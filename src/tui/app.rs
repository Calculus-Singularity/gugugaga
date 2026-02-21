//! Main TUI application

use std::collections::HashMap;
use std::io::{self, Stdout};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use crossterm::{
    event::{self, Event},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode},
};
use ratatui::{
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout, Rect},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph, Wrap},
    Frame, Terminal, TerminalOptions, Viewport,
};
use tokio::sync::{mpsc, RwLock};

use crate::memory::GugugagaNotebook;

/// Convert an absolute file path to a relative path based on cwd.
fn make_relative_path(raw_path: &str, cwd: &str) -> String {
    if let Ok(rel) = std::path::Path::new(raw_path).strip_prefix(cwd) {
        rel.to_string_lossy().to_string()
    } else {
        // Fallback: just use the filename
        std::path::Path::new(raw_path)
            .file_name()
            .map(|f| f.to_string_lossy().to_string())
            .unwrap_or_else(|| raw_path.to_string())
    }
}

fn is_supported_image_path(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| {
            matches!(
                ext.to_ascii_lowercase().as_str(),
                "png" | "jpg" | "jpeg" | "webp" | "gif" | "bmp"
            )
        })
        .unwrap_or(false)
}

/// Parse pasted text and return a local image path if it looks like one.
fn parse_pasted_image_path(pasted: &str) -> Option<PathBuf> {
    let trimmed = pasted.trim();
    if trimmed.is_empty() {
        return None;
    }

    let unquoted = trimmed
        .strip_prefix('"')
        .and_then(|s| s.strip_suffix('"'))
        .or_else(|| {
            trimmed
                .strip_prefix('\'')
                .and_then(|s| s.strip_suffix('\''))
        })
        .unwrap_or(trimmed);

    let candidate = if let Some(rest) = unquoted.strip_prefix("file://") {
        PathBuf::from(rest)
    } else {
        PathBuf::from(unquoted)
    };

    (candidate.exists() && is_supported_image_path(&candidate)).then_some(candidate)
}

fn tool_args_preview(args: &str, max_bytes: usize) -> String {
    let compact = args
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    if compact.is_empty() {
        return String::new();
    }
    let preview = truncate_utf8(&compact, max_bytes);
    if preview.len() < compact.len() {
        format!("{preview}...")
    } else {
        preview.to_string()
    }
}

fn format_tool_args_for_display(args: &str) -> String {
    let trimmed = args.trim();
    if trimmed.is_empty() {
        return String::new();
    }
    match serde_json::from_str::<serde_json::Value>(trimmed) {
        Ok(json) => serde_json::to_string_pretty(&json).unwrap_or_else(|_| trimmed.to_string()),
        Err(_) => trimmed.to_string(),
    }
}

fn format_json_value_for_display(value: &serde_json::Value) -> String {
    serde_json::to_string_pretty(value).unwrap_or_else(|_| value.to_string())
}

fn is_internal_supervision_tool(tool: &str) -> bool {
    matches!(
        tool,
        "update_notebook"
            | "set_activity"
            | "clear_activity"
            | "add_completed"
            | "add_attention"
            | "notebook_mistake"
    )
}

fn supervisor_tool_trace_debug_enabled() -> bool {
    std::env::var("GUGUGAGA_DEBUG_TOOL_TRACE")
        .ok()
        .map(|v| {
            let v = v.trim().to_ascii_lowercase();
            v == "1" || v == "true" || v == "yes" || v == "on"
        })
        .unwrap_or(false)
}

fn notebook_activity_label(value: Option<&str>) -> String {
    value
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| "(none)".to_string())
}

fn format_notebook_diff_for_display(diff: &serde_json::Value) -> Vec<String> {
    let mut lines = Vec::new();
    let activity_before = diff.get("activity_before").and_then(|v| v.as_str());
    let activity_after = diff.get("activity_after").and_then(|v| v.as_str());
    let before_label = notebook_activity_label(activity_before);
    let after_label = notebook_activity_label(activity_after);
    if before_label != after_label {
        lines.push(format!("activity: {} -> {}", before_label, after_label));
    }

    let section = |prefix: &str, key_added: &str, key_before: &str, key_after: &str| {
        let mut section_lines = Vec::new();
        let before = diff.get(key_before).and_then(|v| v.as_u64()).unwrap_or(0);
        let after = diff.get(key_after).and_then(|v| v.as_u64()).unwrap_or(0);
        let added = diff
            .get(key_added)
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();

        if !added.is_empty() {
            section_lines.push(format!(
                "{} +{} ({} -> {})",
                prefix,
                added.len(),
                before,
                after
            ));
            for entry in added.iter().take(3) {
                if let Some(text) = entry.as_str() {
                    section_lines.push(format!("  + {}", text));
                }
            }
            if added.len() > 3 {
                section_lines.push(format!("  ... ({} more)", added.len() - 3));
            }
        } else if before != after {
            section_lines.push(format!("{} {} -> {}", prefix, before, after));
        }

        section_lines
    };

    lines.extend(section(
        "completed",
        "completed_added",
        "completed_before",
        "completed_after",
    ));
    lines.extend(section(
        "attention",
        "attention_added",
        "attention_before",
        "attention_after",
    ));
    lines.extend(section(
        "mistakes",
        "mistakes_added",
        "mistakes_before",
        "mistakes_after",
    ));

    lines
}

use super::ascii_animation::AsciiAnimation;
use super::clipboard_paste::paste_image_to_temp_png;
use super::input::{InputAction, InputState};
use super::picker::{Picker, PickerItem};
use super::slash_commands::{
    parse_command, CodexCommand, GugugagaCommand, ParsedCommand, SlashPopup,
};
use super::theme::Theme;
use super::widgets::{
    render_message_lines, truncate_to_width_str, wrapped_input_cursor_position, HeaderBar,
    InputBox, Message, MessageRole, StatusBar,
};

/// Truncate a string to at most `max_bytes` bytes at a valid UTF-8 char boundary.
/// This avoids panicking on multi-byte characters (e.g. CJK).
fn truncate_utf8(s: &str, max_bytes: usize) -> &str {
    if s.len() <= max_bytes {
        return s;
    }
    // Walk backwards from max_bytes to find a char boundary
    let mut end = max_bytes;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

/// Keep at most the last `max_bytes` bytes, aligned to a UTF-8 char boundary.
fn tail_utf8(s: &str, max_bytes: usize) -> &str {
    if s.len() <= max_bytes {
        return s;
    }
    let mut start = s.len().saturating_sub(max_bytes);
    while start < s.len() && !s.is_char_boundary(start) {
        start += 1;
    }
    &s[start..]
}

/// Application phase — welcome screen or main chat.
#[derive(Debug, Clone, PartialEq, Eq)]
enum AppPhase {
    Welcome,
    Chat,
}

/// Minimum terminal dimensions for showing the animation.
/// Frame is 17 rows × 42 cols; leave room for 3 lines of text below.
const MIN_ANIMATION_WIDTH: u16 = 44;
const MIN_ANIMATION_HEIGHT: u16 = 20;
const QUIT_SHORTCUT_TIMEOUT: Duration = Duration::from_secs(2);
const LARGE_PASTE_CHAR_THRESHOLD: usize = 1000;
const DEFAULT_STATUS_LINE_ITEMS: [&str; 3] =
    ["model-with-reasoning", "context-remaining", "current-dir"];
const STATUS_LINE_AVAILABLE_ITEMS: [(&str, &str, &str, &str); 15] = [
    (
        "model-name",
        "Model Name",
        "Current model name",
        "gpt-5.2-codex",
    ),
    (
        "model-with-reasoning",
        "Model + Reasoning",
        "Current model with reasoning level",
        "gpt-5.2-codex medium",
    ),
    (
        "current-dir",
        "Current Dir",
        "Current working directory",
        "~/project/path",
    ),
    (
        "project-root",
        "Project Root",
        "Project root directory (when available)",
        "~/project",
    ),
    (
        "git-branch",
        "Git Branch",
        "Current git branch (when available)",
        "feat/alignment",
    ),
    (
        "context-remaining",
        "Context Remaining",
        "Remaining context percent (when known)",
        "18% left",
    ),
    (
        "context-used",
        "Context Used",
        "Used context percent (when known)",
        "82% used",
    ),
    (
        "five-hour-limit",
        "5h Limit",
        "5-hour rate limit remaining (when available)",
        "5h 100%",
    ),
    (
        "weekly-limit",
        "Weekly Limit",
        "Weekly rate limit remaining (when available)",
        "weekly 98%",
    ),
    ("codex-version", "Codex Version", "Codex version", "v0.93.0"),
    (
        "context-window-size",
        "Context Window Size",
        "Model context window size",
        "258K window",
    ),
    (
        "used-tokens",
        "Used Tokens",
        "Total tokens used in session",
        "27.3K used",
    ),
    (
        "total-input-tokens",
        "Total Input Tokens",
        "Total input tokens",
        "17,588 in",
    ),
    (
        "total-output-tokens",
        "Total Output Tokens",
        "Total output tokens",
        "265 out",
    ),
    (
        "session-id",
        "Session ID",
        "Current session identifier",
        "019c19bd-ceb6-73b0-adc8-8ec0397b85cf",
    ),
];

/// Current picker mode
#[derive(Debug, Clone, PartialEq, Eq)]
enum PickerMode {
    None,
    Resume,
    ReviewPreset,
    ReviewBranch,
    ReviewCommit,
    FeedbackCategory,
    FeedbackIncludeLogs,
    Model,
    ModelReasoning,         // /model second stage: select reasoning effort
    GugugagaModel,          // //model — select model for gugugaga supervisor
    GugugagaModelReasoning, // //model second stage: select reasoning effort
    SkillsMenu,             // First-level: "List skills" / "Enable/Disable"
    SkillsSelect,           // Second-level: select a skill to insert as $mention
    SkillsManage,           // Second-level: toggle individual skills on/off
    Permissions,
    Personality,
    Collab,
    Agent,
    Statusline,
}

/// Type of pending request
#[derive(Debug, Clone, PartialEq)]
enum PendingRequestType {
    None,
    ThreadList,
    ThreadResume(String),
    #[allow(dead_code)]
    ThreadRead(String),
    RolloutPathLookup(String),
    ThreadCompactStart,
    ThreadBackgroundTerminalsClean,
    ModelList,
    GugugagaModelList,
    SkillsList,
    CollabModeList,
    AgentThreadList,
    McpServerList,
    AppsList,
    ConfigRead,
    DebugConfigRead,
    StatusRead,
    StatuslineConfigRead,
    FeedbackUpload,
    NewThread,
    ForkThread,
    RenameThread,
    Logout,
}

fn take_pending_request_type(
    pending_requests: &mut HashMap<u64, PendingRequestType>,
    request_id: u64,
) -> Option<PendingRequestType> {
    pending_requests.remove(&request_id)
}

fn track_pending_request(
    pending_request_type: &mut PendingRequestType,
    pending_requests: &mut HashMap<u64, PendingRequestType>,
    request_id: u64,
    request_type: PendingRequestType,
) {
    *pending_request_type = request_type.clone();
    pending_requests.insert(request_id, request_type);
}

/// Pending approval request from server
#[derive(Debug, Clone)]
#[allow(dead_code)]
struct PendingApproval {
    request_id: u64,
    approval_type: ApprovalType,
    command: Option<String>,
    cwd: Option<String>,
    reason: Option<String>,
    changes: Vec<String>,
    /// For command exec: proposed execpolicy amendment prefix (e.g. ["echo"])
    /// When present, the user can choose "don't ask again for similar commands"
    proposed_execpolicy_amendment: Option<Vec<String>>,
}

#[derive(Debug, Clone)]
struct PendingUserInput {
    request_id: u64,
    question_ids: Vec<String>,
}

#[derive(Debug, Clone)]
enum ApprovalType {
    CommandExecution,
    FileChange,
}

#[derive(Debug, Clone)]
struct ModelReasoningEffort {
    effort: String,
    description: String,
}

#[derive(Debug, Clone)]
struct ModelInfoEntry {
    id: String,
    display_name: String,
    description: String,
    supported_reasoning_efforts: Vec<ModelReasoningEffort>,
    default_reasoning_effort: Option<String>,
    is_default: bool,
}

#[derive(Debug, Clone, Default)]
struct TokenUsageSnapshot {
    total_tokens: i64,
    input_tokens: i64,
    cached_input_tokens: i64,
    output_tokens: i64,
    reasoning_output_tokens: i64,
    model_context_window: Option<i64>,
    turn_id: Option<String>,
}

#[derive(Debug, Clone, Default)]
struct ActiveTerminal {
    command: String,
    recent_output: String,
}

#[derive(Debug, Clone, Default)]
struct RateLimitWindowSnapshot {
    used_percent: f64,
    window_duration_mins: Option<i64>,
    resets_at: Option<i64>,
}

#[derive(Debug, Clone, Default)]
struct RateLimitSnapshotCache {
    limit_id: Option<String>,
    limit_name: Option<String>,
    plan_type: Option<String>,
    primary: Option<RateLimitWindowSnapshot>,
    secondary: Option<RateLimitWindowSnapshot>,
    credits_has_credits: Option<bool>,
    credits_unlimited: Option<bool>,
    credits_balance: Option<String>,
}

#[derive(Debug, Clone, Default)]
struct StatuslineEditorState {
    enabled_items: Vec<String>,
    selected_item: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum QuitShortcutKey {
    CtrlC,
    CtrlD,
}

impl QuitShortcutKey {
    fn label(self) -> &'static str {
        match self {
            QuitShortcutKey::CtrlC => "Ctrl+C",
            QuitShortcutKey::CtrlD => "Ctrl+D",
        }
    }
}

#[derive(Debug, Clone, Default)]
struct TranscriptOverlayState {
    scroll_offset: usize,
}

/// Application state
pub struct App {
    terminal: Terminal<CrosstermBackend<Stdout>>,
    input: InputState,
    messages: Vec<Message>,
    scroll_offset: usize,
    spinner_frame: usize,
    is_processing: bool,
    project_name: String,
    /// Current working directory (for filtering sessions)
    cwd: String,
    violations_detected: usize,
    current_turn_violations: usize,
    corrections_made: usize,
    auto_replies: usize,
    should_quit: bool,
    input_tx: Option<mpsc::Sender<String>>,
    output_rx: Option<mpsc::Receiver<String>>,
    /// Slash command popup state
    slash_popup: SlashPopup,
    /// Generic picker for resume/model selection
    picker: Picker,
    /// What the picker is currently for
    picker_mode: PickerMode,
    /// Type of pending request
    pending_request_type: PendingRequestType,
    /// Pending request type keyed by request id for deterministic matching.
    pending_requests: HashMap<u64, PendingRequestType>,
    /// Request ID counter
    request_counter: u64,
    /// Cached model catalog from latest model/list response
    available_models: Vec<ModelInfoEntry>,
    /// Model temporarily selected in /model before choosing reasoning effort
    pending_model_for_reasoning: Option<ModelInfoEntry>,
    /// Model temporarily selected in //model before choosing reasoning effort
    pending_gugugaga_model_for_reasoning: Option<ModelInfoEntry>,
    /// Current thread ID (from thread/start response)
    thread_id: Option<String>,
    /// Current turn ID (from turn/started notification, needed for interrupt)
    current_turn_id: Option<String>,
    /// Ctrl+C double-press quit: armed when first Ctrl+C is pressed
    quit_armed: bool,
    /// Which shortcut armed the double-press quit flow.
    quit_armed_key: Option<QuitShortcutKey>,
    /// Timestamp of first Ctrl+C press for double-press timeout
    quit_armed_at: Option<std::time::Instant>,
    /// Pending approval request (waiting for user response)
    pending_approval: Option<PendingApproval>,
    /// Pending request_user_input prompt from app-server.
    pending_user_input: Option<PendingUserInput>,
    /// Waiting for the next user submission to provide thread rename text.
    pending_rename_input: bool,
    /// Waiting for the next user submission to provide custom review instructions.
    pending_review_custom_input: bool,
    /// Waiting for optional feedback note text before upload.
    pending_feedback_note_input: bool,
    /// Feedback classification selected from /feedback picker.
    pending_feedback_classification: Option<String>,
    /// Whether to include logs for the pending feedback upload.
    pending_feedback_include_logs: bool,
    /// Scroll offset for approval overlay content
    approval_scroll: usize,
    /// Gugugaga notebook reference (for TUI display)
    notebook: Option<Arc<RwLock<GugugagaNotebook>>>,
    /// Cached notebook data for rendering (updated periodically)
    notebook_current_activity: Option<String>,
    notebook_completed_count: usize,
    notebook_attention_items: Vec<(String, bool)>, // (content, is_high_priority)
    notebook_mistakes_count: usize,
    /// Gugugaga thinking status (shown in status bar, like Codex's StatusIndicatorWidget)
    gugugaga_status: Option<String>,
    /// Pending session restore data from gugugaga/sessionRestore (arrives before thread/resume)
    pending_session_restore: Option<Vec<serde_json::Value>>,
    /// Latest token usage snapshot from thread/tokenUsage/updated.
    token_usage_snapshot: Option<TokenUsageSnapshot>,
    /// Latest account rate limits snapshot from account/rateLimits/updated.
    rate_limit_snapshot: Option<RateLimitSnapshotCache>,
    /// Latest account auth mode from account/updated (e.g. apikey/chatgpt).
    account_auth_mode: Option<String>,
    /// Current collaboration mode key (e.g. "default", "plan") for UI indicator.
    collaboration_mode: Option<String>,
    /// Whether collaboration modes feature is enabled in config (if known).
    collaboration_modes_feature_enabled: Option<bool>,
    /// Last known collaboration mode options for Shift+Tab cycling.
    collaboration_mode_options: Vec<String>,
    /// Active command execution terminals keyed by item_id.
    active_terminals: HashMap<String, ActiveTerminal>,
    /// Images attached via clipboard paste (Ctrl/Alt+V or pasted file paths).
    attached_images: Vec<PathBuf>,
    /// Large paste placeholders that should be expanded on submit.
    pending_large_pastes: Vec<(String, String)>,
    /// Counter by char-count so repeated large pastes get unique placeholder labels.
    large_paste_counters: HashMap<usize, usize>,
    /// In-progress state for /statusline editor UI.
    statusline_editor: Option<StatuslineEditorState>,
    /// When the current turn started processing (for elapsed time display)
    turn_start_time: Option<std::time::Instant>,
    /// Current application phase (Welcome animation → Chat).
    phase: AppPhase,
    /// ASCII art animation for the welcome screen.
    animation: AsciiAnimation,
    /// Trust onboarding context. `Some` = user still needs to choose.
    trust_ctx: Option<crate::trust::TrustContext>,
    /// The Rect of the inner message area (set each draw frame).
    msg_inner_rect: Rect,
    /// Full transcript overlay state (opened with Ctrl+T).
    transcript_overlay: Option<TranscriptOverlayState>,
    /// Shortcut legend overlay state (toggled by ? when composer is empty).
    shortcuts_overlay_visible: bool,
    /// Esc-Esc backtrack armed state.
    esc_backtrack_armed: bool,
    /// Timestamp for Esc-Esc backtrack arming.
    esc_backtrack_armed_at: Option<std::time::Instant>,
}

impl App {
    /// Create a new App instance.
    ///
    /// If `trust_ctx` is `Some`, the Welcome phase will show the trust
    /// selection UI alongside the animation (matching Codex's onboarding).
    pub fn new(
        project_name: String,
        cwd: String,
        trust_ctx: Option<crate::trust::TrustContext>,
    ) -> io::Result<Self> {
        enable_raw_mode()?;
        let mut stdout = io::stdout();
        execute!(stdout, crossterm::event::EnableBracketedPaste)?;
        let backend = CrosstermBackend::new(stdout);
        let (_, term_height) = crossterm::terminal::size().unwrap_or((0, 24));
        let viewport_height = term_height.saturating_sub(1).max(10);
        let terminal = Terminal::with_options(
            backend,
            TerminalOptions {
                viewport: Viewport::Inline(viewport_height),
            },
        )?;

        Ok(Self {
            terminal,
            input: InputState::new(),
            messages: vec![],
            scroll_offset: 0,
            spinner_frame: 0,
            is_processing: false,
            project_name,
            cwd,
            violations_detected: 0,
            current_turn_violations: 0,
            corrections_made: 0,
            auto_replies: 0,
            should_quit: false,
            input_tx: None,
            output_rx: None,
            slash_popup: SlashPopup::new(),
            picker: Picker::new("Select"),
            picker_mode: PickerMode::None,
            pending_request_type: PendingRequestType::None,
            pending_requests: HashMap::new(),
            request_counter: 100, // Start after other request IDs
            available_models: Vec::new(),
            pending_model_for_reasoning: None,
            pending_gugugaga_model_for_reasoning: None,
            thread_id: None,
            current_turn_id: None,
            quit_armed: false,
            quit_armed_key: None,
            quit_armed_at: None,
            pending_approval: None,
            pending_user_input: None,
            pending_rename_input: false,
            pending_review_custom_input: false,
            pending_feedback_note_input: false,
            pending_feedback_classification: None,
            pending_feedback_include_logs: true,
            approval_scroll: 0,
            notebook: None,
            notebook_current_activity: None,
            notebook_completed_count: 0,
            notebook_attention_items: Vec::new(),
            notebook_mistakes_count: 0,
            gugugaga_status: None,
            pending_session_restore: None,
            token_usage_snapshot: None,
            rate_limit_snapshot: None,
            account_auth_mode: None,
            collaboration_mode: Some("default".to_string()),
            collaboration_modes_feature_enabled: None,
            collaboration_mode_options: vec!["default".to_string(), "plan".to_string()],
            active_terminals: HashMap::new(),
            attached_images: Vec::new(),
            pending_large_pastes: Vec::new(),
            large_paste_counters: HashMap::new(),
            statusline_editor: None,
            turn_start_time: None,
            // Skip the Welcome animation if trust is already established
            phase: if trust_ctx.is_some() {
                AppPhase::Welcome
            } else {
                AppPhase::Chat
            },
            animation: AsciiAnimation::new(),
            trust_ctx,
            msg_inner_rect: Rect::default(),
            transcript_overlay: None,
            shortcuts_overlay_visible: false,
            esc_backtrack_armed: false,
            esc_backtrack_armed_at: None,
        })
    }

    /// Set the notebook reference for TUI display
    pub fn set_notebook(&mut self, notebook: Arc<RwLock<GugugagaNotebook>>) {
        self.notebook = Some(notebook);
    }

    /// Update the cached notebook data for rendering
    pub async fn update_notebook_cache(&mut self) {
        if let Some(notebook) = &self.notebook {
            let nb = notebook.read().await;
            self.notebook_current_activity = nb.current_activity.clone();
            self.notebook_completed_count = nb.completed.len();
            self.notebook_attention_items = nb
                .attention
                .iter()
                .map(|item| {
                    (
                        item.content.clone(),
                        item.priority == crate::memory::Priority::High,
                    )
                })
                .collect();
            self.notebook_mistakes_count = nb.mistakes.len();
        }
    }

    /// Set the communication channels
    pub fn set_channels(
        &mut self,
        input_tx: mpsc::Sender<String>,
        output_rx: mpsc::Receiver<String>,
    ) {
        self.input_tx = Some(input_tx);
        self.output_rx = Some(output_rx);
    }

    /// Run the main event loop
    pub async fn run(&mut self) -> io::Result<()> {
        // Short poll timeout for responsive UI - check for messages frequently
        let poll_timeout = Duration::from_millis(16); // ~60fps responsiveness
        let animation_poll = Duration::from_millis(80); // Match animation frame rate
        let mut last_spinner_update = std::time::Instant::now();
        let spinner_interval = Duration::from_millis(80);

        // Drain any events queued during terminal setup so they don't
        // trigger an immediate transition out of the Welcome screen.
        while event::poll(Duration::from_millis(0))? {
            let _ = event::read()?;
        }

        // Minimum time the welcome screen must be displayed before
        // accepting input (prevents accidental instant skip).
        let welcome_start = std::time::Instant::now();
        let welcome_min_display = Duration::from_millis(600);

        while !self.should_quit {
            match self.phase {
                AppPhase::Welcome => {
                    // ── Welcome screen phase ────────────────────────────
                    self.draw_welcome()?;

                    // Poll with animation frame rate
                    if event::poll(animation_poll)? {
                        if let Event::Key(key) = event::read()? {
                            use crossterm::event::{KeyCode, KeyModifiers};

                            // Ignore input during minimum display period
                            // (except Ctrl+C which always works)
                            let accept_input = welcome_start.elapsed() >= welcome_min_display;

                            // Ctrl+C → quit immediately (always)
                            if key.code == KeyCode::Char('c')
                                && key.modifiers.contains(KeyModifiers::CONTROL)
                            {
                                self.should_quit = true;
                            } else if !accept_input {
                                // Swallow key — too early
                            } else if self.trust_ctx.is_some() {
                                // Trust selection mode: only 1 or 2 advance
                                match key.code {
                                    KeyCode::Char('1') => {
                                        if let Some(ctx) = self.trust_ctx.take() {
                                            let _ = crate::trust::write_trust_decision(&ctx, true);
                                        }
                                        self.phase = AppPhase::Chat;
                                        self.terminal.clear()?;
                                    }
                                    KeyCode::Char('2') => {
                                        if let Some(ctx) = self.trust_ctx.take() {
                                            let _ = crate::trust::write_trust_decision(&ctx, false);
                                        }
                                        self.phase = AppPhase::Chat;
                                        self.terminal.clear()?;
                                    }
                                    _ => {} // ignore other keys
                                }
                            } else {
                                // No trust needed — any key advances
                                self.phase = AppPhase::Chat;
                                self.terminal.clear()?;
                            }
                        }
                    }
                }
                AppPhase::Chat => {
                    // ── Main chat phase ──────────────────────────────────
                    // Check for new messages first (non-blocking)
                    self.check_output().await;

                    // Update notebook cache for display
                    self.update_notebook_cache().await;

                    // Update spinner at fixed interval
                    if last_spinner_update.elapsed() >= spinner_interval {
                        self.spinner_frame = self.spinner_frame.wrapping_add(1);
                        last_spinner_update = std::time::Instant::now();
                    }

                    // Draw UI — wrap error for better diagnostics
                    if let Err(e) = self.draw() {
                        let msg = format!(
                            "draw() error: {e}\nmessage_count: {}\n",
                            self.messages.len()
                        );
                        let _ = std::fs::write("gugugaga-crash.log", &msg);
                        return Err(e);
                    }

                    // Poll for keyboard/mouse events with short timeout
                    if event::poll(poll_timeout)? {
                        match event::read()? {
                            Event::Key(key) => {
                                self.handle_input(key).await;
                            }
                            Event::Paste(pasted) => {
                                // Normalize CR from terminals like iTerm2.
                                let pasted = pasted.replace('\r', "\n");
                                self.handle_paste_event(pasted);
                            }
                            _ => {}
                        }
                    }
                }
            }
        }

        Ok(())
    }

    fn image_label_list(count: usize) -> String {
        (1..=count)
            .map(|i| format!("[Image #{i}]"))
            .collect::<Vec<_>>()
            .join(" ")
    }

    fn attach_local_image(&mut self, path: PathBuf, _dimensions: Option<(u32, u32)>) {
        let image_index = self.attached_images.len() + 1;
        self.attached_images.push(path);
        let placeholder = format!("[Image #{}]", image_index);
        self.input.insert_text(&placeholder);
        self.prune_pending_large_pastes();
        if self.slash_popup.visible {
            self.update_popup_filter();
        }
    }

    fn has_composer_draft(&self) -> bool {
        !self.input.buffer.is_empty()
            || !self.attached_images.is_empty()
            || self
                .pending_large_pastes
                .iter()
                .any(|(placeholder, _)| self.input.buffer.contains(placeholder))
    }

    fn reset_quit_shortcut_if_expired(&mut self) {
        if !self.quit_armed {
            return;
        }
        if let Some(armed_at) = self.quit_armed_at {
            if armed_at.elapsed() > QUIT_SHORTCUT_TIMEOUT {
                self.clear_quit_shortcut();
            }
        }
    }

    fn clear_quit_shortcut(&mut self) {
        self.quit_armed = false;
        self.quit_armed_key = None;
        self.quit_armed_at = None;
    }

    fn quit_shortcut_armed_for(&self, key: QuitShortcutKey) -> bool {
        self.quit_armed && self.quit_armed_key == Some(key)
    }

    fn arm_quit_shortcut(&mut self, key: QuitShortcutKey) {
        let should_announce = !self.quit_armed || self.quit_armed_key != Some(key);
        self.quit_armed = true;
        self.quit_armed_key = Some(key);
        self.quit_armed_at = Some(std::time::Instant::now());
        if should_announce {
            self.messages.push(Message::system(format!(
                "Press {} again to quit.",
                key.label()
            )));
            self.scroll_to_bottom();
        }
    }

    fn clear_draft_for_ctrl_c(&mut self) {
        self.input.clear_current_input();
        self.attached_images.clear();
        self.pending_large_pastes.clear();
        if self.slash_popup.visible {
            self.slash_popup.close();
        }
    }

    fn can_ctrl_d_quit(&self) -> bool {
        self.input.buffer.is_empty()
            && self.attached_images.is_empty()
            && self.pending_large_pastes.is_empty()
            && !self.slash_popup.visible
            && !self.picker.visible
            && self.pending_approval.is_none()
    }

    fn open_transcript_overlay(&mut self) {
        self.transcript_overlay
            .get_or_insert_with(TranscriptOverlayState::default);
        self.shortcuts_overlay_visible = false;
    }

    fn close_transcript_overlay(&mut self) {
        self.transcript_overlay = None;
    }

    fn toggle_shortcuts_overlay(&mut self) {
        self.shortcuts_overlay_visible = !self.shortcuts_overlay_visible;
        if self.shortcuts_overlay_visible {
            self.transcript_overlay = None;
        }
    }

    fn clear_esc_backtrack_shortcut(&mut self) {
        self.esc_backtrack_armed = false;
        self.esc_backtrack_armed_at = None;
    }

    fn reset_esc_backtrack_if_expired(&mut self) {
        if !self.esc_backtrack_armed {
            return;
        }
        if let Some(armed_at) = self.esc_backtrack_armed_at {
            if armed_at.elapsed() > QUIT_SHORTCUT_TIMEOUT {
                self.clear_esc_backtrack_shortcut();
            }
        }
    }

    fn can_esc_backtrack(&self) -> bool {
        !self.is_processing
            && self.gugugaga_status.is_none()
            && self.input.buffer.is_empty()
            && self.attached_images.is_empty()
            && !self.picker.visible
            && !self.slash_popup.visible
            && self.pending_approval.is_none()
            && self.pending_user_input.is_none()
            && !self.pending_rename_input
            && !self.pending_review_custom_input
            && !self.pending_feedback_note_input
            && self.transcript_overlay.is_none()
            && !self.shortcuts_overlay_visible
    }

    fn previous_user_message_content(&self) -> Option<String> {
        self.messages.iter().rev().find_map(|msg| match msg.role {
            MessageRole::User | MessageRole::UserToGugugaga => Some(msg.content.clone()),
            _ => None,
        })
    }

    fn handle_esc_backtrack_shortcut(&mut self) -> bool {
        self.reset_esc_backtrack_if_expired();
        if !self.can_esc_backtrack() {
            return false;
        }

        if self.esc_backtrack_armed {
            self.clear_esc_backtrack_shortcut();
            if let Some(previous) = self.previous_user_message_content() {
                self.input.set_buffer(&previous);
            } else {
                self.messages
                    .push(Message::system("No previous user message to edit."));
                self.scroll_to_bottom();
            }
            return true;
        }

        self.esc_backtrack_armed = true;
        self.esc_backtrack_armed_at = Some(std::time::Instant::now());
        self.messages
            .push(Message::system("Press Esc again to edit previous message."));
        self.scroll_to_bottom();
        true
    }

    fn handle_transcript_overlay_input(&mut self, key: &event::KeyEvent) -> bool {
        let Some(state) = self.transcript_overlay.as_mut() else {
            return false;
        };

        let is_ctrl_t = matches!(key.code, crossterm::event::KeyCode::Char('t'))
            && key
                .modifiers
                .contains(crossterm::event::KeyModifiers::CONTROL);
        match key.code {
            crossterm::event::KeyCode::Esc if key.modifiers.is_empty() => {
                self.close_transcript_overlay();
            }
            _ if is_ctrl_t => {
                self.close_transcript_overlay();
            }
            crossterm::event::KeyCode::Up | crossterm::event::KeyCode::Char('k')
                if key.modifiers.is_empty() =>
            {
                state.scroll_offset = state.scroll_offset.saturating_add(1);
            }
            crossterm::event::KeyCode::Down | crossterm::event::KeyCode::Char('j')
                if key.modifiers.is_empty() =>
            {
                state.scroll_offset = state.scroll_offset.saturating_sub(1);
            }
            crossterm::event::KeyCode::PageUp => {
                state.scroll_offset = state.scroll_offset.saturating_add(8);
            }
            crossterm::event::KeyCode::PageDown => {
                state.scroll_offset = state.scroll_offset.saturating_sub(8);
            }
            crossterm::event::KeyCode::Home => {
                state.scroll_offset = usize::MAX;
            }
            crossterm::event::KeyCode::End => {
                state.scroll_offset = 0;
            }
            _ => {}
        }
        true
    }

    fn handle_shortcuts_overlay_input(&mut self, key: &event::KeyEvent) -> bool {
        if !self.shortcuts_overlay_visible {
            return false;
        }

        let is_ctrl_t = matches!(key.code, crossterm::event::KeyCode::Char('t'))
            && key
                .modifiers
                .contains(crossterm::event::KeyModifiers::CONTROL);
        let is_question =
            matches!(key.code, crossterm::event::KeyCode::Char('?')) && key.modifiers.is_empty();
        let is_escape =
            matches!(key.code, crossterm::event::KeyCode::Esc) && key.modifiers.is_empty();
        if is_escape || is_question {
            self.shortcuts_overlay_visible = false;
            return true;
        }
        if is_ctrl_t {
            self.shortcuts_overlay_visible = false;
            self.open_transcript_overlay();
            return true;
        }
        false
    }

    fn next_large_paste_placeholder(&mut self, char_count: usize) -> String {
        let base = format!("[Pasted Content {char_count} chars]");
        let next_suffix = self.large_paste_counters.entry(char_count).or_insert(0);
        *next_suffix += 1;
        if *next_suffix == 1 {
            base
        } else {
            format!("{base} #{next_suffix}")
        }
    }

    fn prune_pending_large_pastes(&mut self) {
        let text = self.input.buffer.as_str();
        self.pending_large_pastes
            .retain(|(placeholder, _)| text.contains(placeholder));
    }

    fn expand_pending_large_pastes(&mut self, text: &str) -> String {
        if self.pending_large_pastes.is_empty() {
            return text.to_string();
        }

        let mut expanded = text.to_string();
        for (placeholder, payload) in &self.pending_large_pastes {
            if expanded.contains(placeholder) {
                expanded = expanded.replacen(placeholder, payload, 1);
            }
        }
        self.pending_large_pastes.clear();
        expanded
    }

    fn handle_paste_event(&mut self, pasted: String) {
        if pasted.trim().is_empty() {
            return;
        }

        let char_count = pasted.chars().count();
        if char_count > LARGE_PASTE_CHAR_THRESHOLD {
            let placeholder = self.next_large_paste_placeholder(char_count);
            self.input.insert_text(&placeholder);
            self.pending_large_pastes.push((placeholder, pasted));
            if self.slash_popup.visible {
                self.update_popup_filter();
            }
            return;
        }

        if let Some(path) = parse_pasted_image_path(&pasted) {
            self.attach_local_image(path, None);
            self.input.insert_text(" ");
            return;
        }

        self.input.insert_text(&pasted);
        self.prune_pending_large_pastes();
        if self.slash_popup.visible {
            self.update_popup_filter();
        }
    }

    async fn handle_ctrl_c_shortcut(&mut self) {
        self.reset_quit_shortcut_if_expired();

        if self.has_composer_draft() {
            self.clear_draft_for_ctrl_c();
            self.arm_quit_shortcut(QuitShortcutKey::CtrlC);
            return;
        }

        if self.quit_shortcut_armed_for(QuitShortcutKey::CtrlC) {
            self.should_quit = true;
            self.clear_quit_shortcut();
            return;
        }

        self.arm_quit_shortcut(QuitShortcutKey::CtrlC);
        let _ = self.interrupt_active_work().await;
    }

    fn handle_ctrl_d_shortcut(&mut self) {
        self.reset_quit_shortcut_if_expired();
        if !self.can_ctrl_d_quit() {
            return;
        }
        if self.quit_shortcut_armed_for(QuitShortcutKey::CtrlD) {
            self.should_quit = true;
            self.clear_quit_shortcut();
            return;
        }
        self.arm_quit_shortcut(QuitShortcutKey::CtrlD);
    }

    async fn handle_input(&mut self, key: event::KeyEvent) {
        // Handle approval dialog first (highest priority — modal overlay)
        if let Some(approval) = self.pending_approval.take() {
            match key.code {
                // y / Enter — Accept (approve this time)
                crossterm::event::KeyCode::Char('y')
                | crossterm::event::KeyCode::Char('Y')
                | crossterm::event::KeyCode::Enter => {
                    let cmd_display = approval.command.as_deref().unwrap_or("command");
                    self.messages
                        .push(Message::system(format!("✓ Approved: {}", cmd_display)));
                    self.respond_to_approval_decision(&approval, "accept").await;
                    return;
                }
                // p — Accept and don't ask again for similar commands (exec only, needs amendment)
                crossterm::event::KeyCode::Char('p') | crossterm::event::KeyCode::Char('P')
                    if matches!(approval.approval_type, ApprovalType::CommandExecution)
                        && approval.proposed_execpolicy_amendment.is_some() =>
                {
                    let amendment = approval.proposed_execpolicy_amendment.clone().unwrap();
                    let prefix = amendment.join(" ");
                    self.messages.push(Message::system(format!(
                        "✓ Approved (won't ask again for `{}`)",
                        prefix
                    )));
                    self.respond_to_approval_with_amendment(&approval, &amendment)
                        .await;
                    return;
                }
                // a — Accept for session (file changes: don't ask again for these files)
                crossterm::event::KeyCode::Char('a') | crossterm::event::KeyCode::Char('A')
                    if matches!(approval.approval_type, ApprovalType::FileChange) =>
                {
                    self.messages
                        .push(Message::system("✓ Approved (for session)"));
                    self.respond_to_approval_decision(&approval, "acceptForSession")
                        .await;
                    return;
                }
                // n / Esc — Cancel (reject + interrupt turn)
                crossterm::event::KeyCode::Char('n')
                | crossterm::event::KeyCode::Char('N')
                | crossterm::event::KeyCode::Esc => {
                    self.messages.push(Message::system("✗ Cancelled"));
                    self.respond_to_approval_decision(&approval, "cancel").await;
                    return;
                }
                // Ctrl+C during approval — also cancel (same as Codex)
                crossterm::event::KeyCode::Char('c')
                    if key
                        .modifiers
                        .contains(crossterm::event::KeyModifiers::CONTROL) =>
                {
                    self.messages.push(Message::system("✗ Cancelled"));
                    self.respond_to_approval_decision(&approval, "cancel").await;
                    return;
                }
                // Up/Down/PageUp/PageDown — scroll approval content
                crossterm::event::KeyCode::Up => {
                    self.approval_scroll = self.approval_scroll.saturating_sub(1);
                    self.pending_approval = Some(approval);
                    return;
                }
                crossterm::event::KeyCode::Down => {
                    self.approval_scroll = self.approval_scroll.saturating_add(1);
                    self.pending_approval = Some(approval);
                    return;
                }
                crossterm::event::KeyCode::PageUp => {
                    self.approval_scroll = self.approval_scroll.saturating_sub(5);
                    self.pending_approval = Some(approval);
                    return;
                }
                crossterm::event::KeyCode::PageDown => {
                    self.approval_scroll = self.approval_scroll.saturating_add(5);
                    self.pending_approval = Some(approval);
                    return;
                }
                _ => {
                    // Put it back and wait for valid input
                    self.pending_approval = Some(approval);
                    return;
                }
            }
        }

        // Global interruption hotkeys must win over picker/popup handling.
        // Otherwise Esc can get consumed by UI overlays while active work keeps running.
        let is_ctrl_c = matches!(key.code, crossterm::event::KeyCode::Char('c'))
            && key
                .modifiers
                .contains(crossterm::event::KeyModifiers::CONTROL);
        let is_ctrl_d = matches!(key.code, crossterm::event::KeyCode::Char('d'))
            && key
                .modifiers
                .contains(crossterm::event::KeyModifiers::CONTROL);
        let is_ctrl_t = matches!(key.code, crossterm::event::KeyCode::Char('t'))
            && key
                .modifiers
                .contains(crossterm::event::KeyModifiers::CONTROL);
        let is_question =
            matches!(key.code, crossterm::event::KeyCode::Char('?')) && key.modifiers.is_empty();
        let is_escape = matches!(key.code, crossterm::event::KeyCode::Esc);
        if is_ctrl_c {
            self.handle_ctrl_c_shortcut().await;
            return;
        }
        if is_ctrl_d {
            self.handle_ctrl_d_shortcut();
            return;
        }
        if self.handle_transcript_overlay_input(&key) {
            return;
        }
        if self.handle_shortcuts_overlay_input(&key) {
            return;
        }
        if is_ctrl_t && !self.picker.visible {
            self.open_transcript_overlay();
            return;
        }
        if is_question
            && !self.picker.visible
            && !self.slash_popup.visible
            && !self.has_composer_draft()
        {
            self.toggle_shortcuts_overlay();
            return;
        }
        if is_escape && self.interrupt_active_work().await {
            return;
        }
        if is_escape {
            if self.handle_esc_backtrack_shortcut() {
                return;
            }
            self.clear_esc_backtrack_shortcut();
        } else {
            self.clear_esc_backtrack_shortcut();
        }

        let is_shift_tab = matches!(key.code, crossterm::event::KeyCode::BackTab);
        if is_shift_tab && !self.picker.visible && !self.slash_popup.visible {
            if !self.collaboration_modes_enabled() {
                self.messages.push(Message::system(
                    "Collaboration modes are disabled. Enable collaboration modes to use Shift+Tab.",
                ));
                self.scroll_to_bottom();
            } else if self.is_processing || self.gugugaga_status.is_some() {
                self.messages.push(Message::system(
                    "Cannot switch collaboration mode while a task is running.",
                ));
                self.scroll_to_bottom();
            } else {
                self.cycle_collaboration_mode_shortcut().await;
            }
            return;
        }

        // Handle picker keys
        if self.picker.visible {
            match key.code {
                crossterm::event::KeyCode::Up => {
                    self.picker.select_prev();
                    return;
                }
                crossterm::event::KeyCode::Down => {
                    self.picker.select_next();
                    return;
                }
                crossterm::event::KeyCode::Enter => {
                    self.handle_picker_selection().await;
                    return;
                }
                crossterm::event::KeyCode::Esc => {
                    self.picker.close();
                    if matches!(self.picker_mode, PickerMode::Statusline) {
                        self.statusline_editor = None;
                    }
                    self.picker_mode = PickerMode::None;
                    return;
                }
                _ => return, // Ignore other keys when picker is open
            }
        }

        // Handle slash popup keys
        if self.slash_popup.visible {
            match key.code {
                crossterm::event::KeyCode::Up => {
                    self.slash_popup.select_prev();
                    return;
                }
                crossterm::event::KeyCode::Down => {
                    self.slash_popup.select_next();
                    return;
                }
                crossterm::event::KeyCode::PageUp => {
                    self.slash_popup.page_up();
                    return;
                }
                crossterm::event::KeyCode::PageDown => {
                    self.slash_popup.page_down();
                    return;
                }
                crossterm::event::KeyCode::Tab => {
                    // Tab always completes from popup
                    if let Some(completed) = self.slash_popup.complete() {
                        self.input.set_buffer(&completed);
                        self.slash_popup.close();
                    }
                    return;
                }
                crossterm::event::KeyCode::Enter => {
                    // If popup has a match, complete it.
                    // Otherwise close popup and fall through so Enter
                    // submits the text (e.g. "//hello" as a direct chat).
                    if self.slash_popup.total_matches() > 0 {
                        if let Some(completed) = self.slash_popup.complete() {
                            self.input.set_buffer(&completed);
                            self.slash_popup.close();
                        }
                        return;
                    }
                    self.slash_popup.close();
                    // Don't return — let the key fall through to Submit
                }
                crossterm::event::KeyCode::Esc => {
                    self.slash_popup.close();
                    return;
                }
                _ => {}
            }
        }

        // Ctrl/Alt+V: attempt clipboard image paste.
        if let crossterm::event::KeyCode::Char(c) = key.code {
            if key.modifiers.intersects(
                crossterm::event::KeyModifiers::CONTROL | crossterm::event::KeyModifiers::ALT,
            ) && c.eq_ignore_ascii_case(&'v')
            {
                match paste_image_to_temp_png() {
                    Ok((path, info)) => {
                        self.attach_local_image(path, Some((info.width, info.height)));
                    }
                    Err(err) => {
                        self.messages.push(Message::system(format!(
                            "Failed to paste image from clipboard: {}",
                            err
                        )));
                        self.scroll_to_bottom();
                    }
                }
                return;
            }
        }

        // Support "images-only" submission when input text is empty.
        if key.code == crossterm::event::KeyCode::Enter
            && self.pending_user_input.is_none()
            && self.input.buffer.trim().is_empty()
            && !self.attached_images.is_empty()
        {
            if self.is_processing {
                self.messages
                    .push(Message::system("⏳ Please wait for current processing"));
                self.scroll_to_bottom();
                return;
            }

            let local_images = std::mem::take(&mut self.attached_images);
            let preview = Self::image_label_list(local_images.len());
            self.messages.push(Message::user(preview));
            self.scroll_to_bottom();
            self.start_processing();

            let msg = self.create_turn_message("", &local_images);
            if let Some(tx) = &self.input_tx {
                let _ = tx.send(msg).await;
            }
            return;
        }

        match self.input.handle_key(key) {
            InputAction::Quit => {
                self.should_quit = true;
            }
            InputAction::Submit(text) => {
                self.slash_popup.close();
                let expanded_text = self.expand_pending_large_pastes(&text);

                if let Some(pending) = self.pending_user_input.take() {
                    let trimmed = expanded_text.trim();
                    if trimmed.is_empty() {
                        self.messages.push(Message::system(
                            "Input required. Enter an answer, or type /cancel to send an empty response.",
                        ));
                        self.pending_user_input = Some(pending);
                        self.scroll_to_bottom();
                        return;
                    }

                    if trimmed.eq_ignore_ascii_case("/cancel")
                        || trimmed.eq_ignore_ascii_case("cancel")
                    {
                        self.input.commit_submission();
                        self.respond_to_user_input_request(&pending, None).await;
                        self.messages.push(Message::system(
                            "Sent empty response for pending user input request.",
                        ));
                    } else {
                        self.input.commit_submission();
                        self.respond_to_user_input_request(&pending, Some(trimmed))
                            .await;
                        self.messages.push(Message::system(
                            "Sent response for pending user input request.",
                        ));
                    }
                    self.scroll_to_bottom();
                    return;
                }

                if self.pending_rename_input {
                    let trimmed = expanded_text.trim();
                    if trimmed.eq_ignore_ascii_case("/cancel")
                        || trimmed.eq_ignore_ascii_case("cancel")
                    {
                        self.input.commit_submission();
                        self.pending_rename_input = false;
                        self.messages.push(Message::system("Rename cancelled."));
                        self.scroll_to_bottom();
                        return;
                    }
                    if trimmed.is_empty() {
                        self.messages.push(Message::system(
                            "Thread name cannot be empty. Enter a name, or type /cancel.",
                        ));
                        self.scroll_to_bottom();
                        return;
                    }
                    self.input.commit_submission();
                    self.pending_rename_input = false;
                    self.request_rename(trimmed).await;
                    self.scroll_to_bottom();
                    return;
                }

                if self.pending_review_custom_input {
                    let trimmed = expanded_text.trim();
                    if trimmed.eq_ignore_ascii_case("/cancel")
                        || trimmed.eq_ignore_ascii_case("cancel")
                    {
                        self.input.commit_submission();
                        self.pending_review_custom_input = false;
                        self.messages
                            .push(Message::system("Custom review request cancelled."));
                        self.scroll_to_bottom();
                        return;
                    }
                    if trimmed.is_empty() {
                        self.messages.push(Message::system(
                            "Review instructions cannot be empty. Enter text, or type /cancel.",
                        ));
                        self.scroll_to_bottom();
                        return;
                    }
                    self.input.commit_submission();
                    self.pending_review_custom_input = false;
                    self.request_review_custom(trimmed).await;
                    self.scroll_to_bottom();
                    return;
                }

                if self.pending_feedback_note_input {
                    let trimmed = expanded_text.trim();
                    if trimmed.eq_ignore_ascii_case("/cancel")
                        || trimmed.eq_ignore_ascii_case("cancel")
                    {
                        self.input.commit_submission();
                        self.pending_feedback_note_input = false;
                        self.pending_feedback_classification = None;
                        self.pending_feedback_include_logs = true;
                        self.messages.push(Message::system("Feedback cancelled."));
                        self.scroll_to_bottom();
                        return;
                    }

                    let note = if trimmed.eq_ignore_ascii_case("/skip")
                        || trimmed.eq_ignore_ascii_case("skip")
                    {
                        None
                    } else if trimmed.is_empty() {
                        self.messages.push(Message::system(
                            "Type feedback details, /skip to send without a note, or /cancel.",
                        ));
                        self.scroll_to_bottom();
                        return;
                    } else {
                        Some(trimmed)
                    };

                    let classification = self
                        .pending_feedback_classification
                        .take()
                        .unwrap_or_else(|| "other".to_string());
                    let include_logs = self.pending_feedback_include_logs;
                    self.input.commit_submission();
                    self.pending_feedback_note_input = false;
                    self.pending_feedback_include_logs = true;
                    self.request_feedback_upload(&classification, note, include_logs)
                        .await;
                    self.scroll_to_bottom();
                    return;
                }

                // Parse and handle command
                if let Some(parsed) = parse_command(&expanded_text) {
                    match parsed {
                        ParsedCommand::Codex(cmd, args) => {
                            if self.is_processing && !Self::codex_command_available_during_task(cmd)
                            {
                                self.messages.push(Message::system(format!(
                                    "`/{}` is unavailable while a task is running.",
                                    cmd.name()
                                )));
                                self.scroll_to_bottom();
                                return;
                            }
                            self.input.commit_submission();
                            self.forward_codex_command(cmd, args).await;
                        }
                        ParsedCommand::Gugugaga(cmd, args) => {
                            self.input.commit_submission();
                            self.execute_gugugaga_command(cmd, args).await;
                        }
                        ParsedCommand::GugugagaChat(message) => {
                            self.input.commit_submission();
                            self.send_gugugaga_chat(&message).await;
                        }
                        ParsedCommand::Unknown(name) => {
                            self.input.commit_submission();
                            self.messages.push(Message::system(format!(
                                "Unknown command: {}. Use //help for Gugugaga commands.",
                                name
                            )));
                        }
                    }
                } else {
                    // Regular message to Codex - block if already processing
                    if self.is_processing {
                        self.messages
                            .push(Message::system("⏳ Please wait for current processing"));
                        return;
                    }
                    self.input.commit_submission();
                    let local_images = std::mem::take(&mut self.attached_images);
                    let display_text = if local_images.is_empty() {
                        text.clone()
                    } else {
                        let trimmed = text.trim();
                        if trimmed.is_empty() {
                            Self::image_label_list(local_images.len())
                        } else {
                            text.clone()
                        }
                    };

                    self.messages.push(Message::user(display_text));
                    self.scroll_to_bottom();
                    self.start_processing();

                    let msg = self.create_turn_message(&expanded_text, &local_images);
                    if let Some(tx) = &self.input_tx {
                        let _ = tx.send(msg).await;
                    }
                }
            }
            InputAction::ScrollUp => {
                // Do not scroll message pane from composer keys.
            }
            InputAction::ScrollDown => {
                // Do not scroll message pane from composer keys.
            }
            InputAction::HistoryPrev => {
                if self.input.should_handle_history_navigation() {
                    let _ = self.input.navigate_history_prev();
                }
            }
            InputAction::HistoryNext => {
                if self.input.should_handle_history_navigation() {
                    let _ = self.input.navigate_history_next();
                }
            }
            InputAction::Tab => {
                self.handle_tab_completion();
            }
            InputAction::Escape => {
                // Priority: slash_popup > interrupt running work > nothing
                if self.slash_popup.visible {
                    self.slash_popup.close();
                } else if self.is_processing || self.gugugaga_status.is_some() {
                    let _ = self.interrupt_active_work().await;
                }
                // Otherwise ignore
            }
            InputAction::Input('/') => {
                // Check for // (gugugaga) or / (codex)
                if self.input.buffer == "//" {
                    self.slash_popup.open_gugugaga();
                } else if self.input.buffer == "/" {
                    self.slash_popup.open_codex();
                } else if self.slash_popup.visible {
                    self.update_popup_filter();
                }
            }
            InputAction::Input(_)
            | InputAction::Backspace
            | InputAction::DeleteWord
            | InputAction::Delete => {
                self.prune_pending_large_pastes();
                if self.slash_popup.visible {
                    self.update_popup_filter();
                }
            }
            _ => {}
        }
    }

    fn handle_tab_completion(&mut self) {
        let buffer = &self.input.buffer;

        if buffer.starts_with("//") {
            // Gugugaga command
            if !self.slash_popup.visible {
                self.slash_popup.open_gugugaga();
            }
            let prefix = buffer.strip_prefix("//").unwrap_or("");
            let prefix = prefix.split(' ').next().unwrap_or("");
            self.slash_popup.set_filter(prefix);

            // Auto-complete if single match
            if self.slash_popup.gugugaga_matches.len() == 1 {
                if let Some(completed) = self.slash_popup.complete() {
                    self.input.set_buffer(&completed);
                    self.slash_popup.close();
                }
            }
        } else if buffer.starts_with('/') {
            // Codex command
            if !self.slash_popup.visible {
                self.slash_popup.open_codex();
            }
            let prefix = buffer.strip_prefix('/').unwrap_or("");
            let prefix = prefix.split(' ').next().unwrap_or("");
            self.slash_popup.set_filter(prefix);

            // Auto-complete if single match
            if self.slash_popup.codex_matches.len() == 1 {
                if let Some(completed) = self.slash_popup.complete() {
                    self.input.set_buffer(&completed);
                    self.slash_popup.close();
                }
            }
        }
    }

    fn update_popup_filter(&mut self) {
        let buffer = &self.input.buffer;

        if buffer.starts_with("//") {
            if !self.slash_popup.is_gugugaga {
                self.slash_popup.open_gugugaga();
            }
            let prefix = buffer.strip_prefix("//").unwrap_or("");
            let prefix = prefix.split(' ').next().unwrap_or("");
            self.slash_popup.set_filter(prefix);
            // Auto-close when typing free text that matches no command
            if self.slash_popup.total_matches() == 0 && !prefix.is_empty() {
                self.slash_popup.close();
            }
        } else if buffer.starts_with('/') {
            if self.slash_popup.is_gugugaga {
                self.slash_popup.open_codex();
            }
            let prefix = buffer.strip_prefix('/').unwrap_or("");
            let prefix = prefix.split(' ').next().unwrap_or("");
            self.slash_popup.set_filter(prefix);
        } else {
            self.slash_popup.close();
        }
    }

    fn set_active_thread_id(&mut self, thread_id: String) {
        let changed = self.thread_id.as_deref() != Some(thread_id.as_str());
        self.thread_id = Some(thread_id);
        if changed {
            self.active_terminals.clear();
            self.token_usage_snapshot = None;
        }
    }

    fn codex_command_available_during_task(cmd: CodexCommand) -> bool {
        matches!(
            cmd,
            CodexCommand::Diff
                | CodexCommand::Rename
                | CodexCommand::Mention
                | CodexCommand::Skills
                | CodexCommand::Status
                | CodexCommand::DebugConfig
                | CodexCommand::Ps
                | CodexCommand::Clean
                | CodexCommand::Mcp
                | CodexCommand::Apps
                | CodexCommand::Feedback
                | CodexCommand::Quit
                | CodexCommand::Exit
                | CodexCommand::Rollout
                | CodexCommand::Collab
                | CodexCommand::Agent
                | CodexCommand::TestApproval
        )
    }

    fn statusline_item_is_known(item_id: &str) -> bool {
        STATUS_LINE_AVAILABLE_ITEMS
            .iter()
            .any(|(id, _, _, _)| *id == item_id)
    }

    fn default_statusline_items() -> Vec<String> {
        DEFAULT_STATUS_LINE_ITEMS
            .iter()
            .map(|item| (*item).to_string())
            .collect()
    }

    fn normalize_statusline_item_id(item_id: &str) -> Option<String> {
        let normalized = match item_id {
            "5h-limit" => "five-hour-limit",
            other => other,
        };
        Self::statusline_item_is_known(normalized).then(|| normalized.to_string())
    }

    fn parse_statusline_items_from_config(config_result: &serde_json::Value) -> Vec<String> {
        let config = config_result.get("config").unwrap_or(config_result);
        let raw = config
            .get("tui_status_line")
            .or_else(|| config.pointer("/tui/status_line"))
            .or_else(|| config.get("tui").and_then(|tui| tui.get("status_line")));

        let mut dedup = Vec::<String>::new();
        if let Some(items) = raw.and_then(|value| value.as_array()) {
            for value in items {
                if let Some(item) = value.as_str() {
                    if let Some(normalized) = Self::normalize_statusline_item_id(item) {
                        if !dedup.iter().any(|existing| existing == &normalized) {
                            dedup.push(normalized);
                        }
                    }
                }
            }
        }
        if dedup.is_empty() {
            Self::default_statusline_items()
        } else {
            dedup
        }
    }

    async fn forward_codex_command(&mut self, cmd: CodexCommand, args: String) {
        match cmd {
            // === Local commands (handled in TUI) ===
            CodexCommand::Quit | CodexCommand::Exit => {
                self.should_quit = true;
            }
            CodexCommand::Resume => {
                self.open_resume_picker().await;
            }
            CodexCommand::New => {
                self.request_new_thread().await;
            }
            CodexCommand::Status => {
                self.request_status().await;
            }
            CodexCommand::Diff => {
                self.show_git_diff().await;
            }
            CodexCommand::Ps => {
                self.show_background_processes();
            }
            CodexCommand::Rollout => {
                self.show_rollout_path().await;
            }

            // === RPC commands (forward to Codex with correct method) ===
            CodexCommand::Model => {
                self.request_model_list().await;
            }
            CodexCommand::Skills => {
                self.open_skills_menu().await;
            }
            CodexCommand::Review => {
                if args.trim().is_empty() {
                    self.open_review_picker();
                } else {
                    self.request_review_custom(args.trim()).await;
                }
            }
            CodexCommand::Rename => {
                if args.trim().is_empty() {
                    if self.thread_id.is_some() {
                        self.pending_rename_input = true;
                        self.messages.push(Message::system(
                            "Rename thread: type a new name and press Enter (or /cancel).",
                        ));
                    } else {
                        self.messages
                            .push(Message::system("No active thread to rename"));
                    }
                } else {
                    self.request_rename(args.trim()).await;
                }
            }
            CodexCommand::Fork => {
                self.request_fork().await;
            }
            CodexCommand::Logout => {
                self.request_logout().await;
            }
            CodexCommand::Feedback => {
                self.open_feedback_category_picker();
            }
            CodexCommand::Mcp => {
                self.show_mcp_tools().await;
            }
            CodexCommand::Apps => {
                self.request_apps_list().await;
            }

            // === Config-based commands (need picker UI) ===
            CodexCommand::Approvals => {
                self.open_approvals_picker().await;
            }
            CodexCommand::Permissions => {
                self.open_permissions_picker().await;
            }
            CodexCommand::Personality => {
                self.open_personality_picker().await;
            }
            CodexCommand::Experimental => {
                self.open_experimental_picker().await;
            }
            CodexCommand::Collab => {
                self.open_collab_picker().await;
            }
            CodexCommand::Plan => {
                self.set_plan_mode().await;
                let trimmed = args.trim();
                if !trimmed.is_empty() {
                    self.messages.push(Message::user(trimmed.to_string()));
                    self.scroll_to_bottom();
                    self.start_processing();
                    let msg = self.create_turn_message(trimmed, &[]);
                    if let Some(tx) = &self.input_tx {
                        let _ = tx.send(msg).await;
                    }
                }
            }
            CodexCommand::Agent => {
                self.open_agent_picker().await;
            }
            CodexCommand::Compact => {
                self.request_thread_compaction().await;
            }
            CodexCommand::Clean => {
                self.request_background_terminals_clean().await;
            }
            CodexCommand::DebugConfig => {
                self.request_debug_config().await;
            }
            CodexCommand::Statusline => {
                self.open_statusline_picker().await;
            }
            CodexCommand::TestApproval => {
                self.messages.push(Message::system(
                    "/test-approval is a debug command and is not implemented in this TUI.",
                ));
            }
            CodexCommand::MemoryDrop => {
                self.messages.push(Message::system(
                    "/debug-m-drop is a debug command and is not available in this TUI.",
                ));
            }
            CodexCommand::MemoryUpdate => {
                self.messages.push(Message::system(
                    "/debug-m-update is a debug command and is not available in this TUI.",
                ));
            }

            // === Special commands ===
            CodexCommand::Init => {
                self.execute_init().await;
            }
            CodexCommand::Mention => {
                self.input.insert_text("@");
            }
            CodexCommand::SandboxReadRoot => {
                let trimmed = args.trim();
                if trimmed.is_empty() {
                    self.messages.push(Message::system(
                        "Usage: /sandbox-add-read-dir <absolute-directory-path>",
                    ));
                } else if cfg!(target_os = "windows") {
                    self.messages.push(Message::system(
                        "/sandbox-add-read-dir is not yet implemented in this TUI. Use Codex CLI on Windows for now.",
                    ));
                } else {
                    self.messages.push(Message::system(
                        "/sandbox-add-read-dir is only available on Windows.",
                    ));
                }
            }
            CodexCommand::ElevateSandbox => {
                self.messages.push(Message::system(
                    "/setup-default-sandbox is only available on Windows",
                ));
            }
        }
    }

    /// Request a new thread
    async fn request_new_thread(&mut self) {
        if let Some(tx) = &self.input_tx {
            self.request_counter += 1;
            track_pending_request(
                &mut self.pending_request_type,
                &mut self.pending_requests,
                self.request_counter,
                PendingRequestType::NewThread,
            );
            let msg = serde_json::json!({
                "jsonrpc": "2.0",
                "method": "thread/start",
                "id": self.request_counter,
                "params": {
                    "sandbox": "workspace-write",
                    "approvalPolicy": "on-request",
                    "config": {
                        "experimental_use_freeform_apply_patch": true
                    }
                }
            })
            .to_string();
            let _ = tx.send(msg).await;
            self.messages.clear();
            self.messages
                .push(Message::system("Starting new session..."));
        }
    }

    /// Request status details from app-server config and combine with local token usage snapshot.
    async fn request_status(&mut self) {
        if let Some(tx) = &self.input_tx {
            self.request_counter += 1;
            track_pending_request(
                &mut self.pending_request_type,
                &mut self.pending_requests,
                self.request_counter,
                PendingRequestType::StatusRead,
            );
            let msg = serde_json::json!({
                "jsonrpc": "2.0",
                "method": "config/read",
                "id": self.request_counter,
                "params": {
                    "includeLayers": false,
                    "cwd": self.cwd.clone()
                }
            })
            .to_string();
            let _ = tx.send(msg).await;
            self.messages.push(Message::system("Fetching status..."));
        }
    }

    fn render_status_summary(&self, config_result: &serde_json::Value) -> String {
        let config = config_result.get("config").unwrap_or(config_result);
        let model = config
            .get("model")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown");
        let provider = config
            .get("model_provider")
            .and_then(|v| v.as_str())
            .unwrap_or("-");
        let reasoning = config
            .get("model_reasoning_effort")
            .and_then(|v| v.as_str())
            .unwrap_or("default");
        let approval = config
            .get("approval_policy")
            .and_then(|v| v.as_str())
            .unwrap_or("-");
        let sandbox = config
            .get("sandbox_mode")
            .and_then(|v| v.as_str())
            .unwrap_or("-");
        let collab = config
            .get("collaboration_mode")
            .and_then(|v| v.as_str())
            .or(self.collaboration_mode.as_deref())
            .unwrap_or("-");

        let permissions = if approval == "on-request" && sandbox == "workspace-write" {
            "Default".to_string()
        } else if approval == "never" && sandbox == "danger-full-access" {
            "Full Access".to_string()
        } else {
            format!("Custom ({}, {})", sandbox, approval)
        };

        let overview_rows: Vec<(String, String)> = vec![
            (
                "Model".to_string(),
                format!("{} (reasoning {}, provider {})", model, reasoning, provider),
            ),
            ("Directory".to_string(), self.cwd.clone()),
            ("Permissions".to_string(), permissions),
            ("Agents.md".to_string(), self.status_agents_summary()),
            ("Collaboration".to_string(), collab.to_string()),
            (
                "Session ID".to_string(),
                self.thread_id.as_deref().unwrap_or("none").to_string(),
            ),
        ];

        let mut usage_rows: Vec<(String, String)> = Vec::new();
        let prefer_limits_only = self.status_prefers_limits_over_tokens();
        if let Some(usage) = &self.token_usage_snapshot {
            if !prefer_limits_only {
                let non_cached_input = usage.input_tokens.saturating_sub(usage.cached_input_tokens);
                usage_rows.push((
                    "Token usage".to_string(),
                    format!(
                        "{} total  ({} input + {} output)",
                        Self::format_tokens_compact(usage.total_tokens),
                        Self::format_tokens_compact(non_cached_input),
                        Self::format_tokens_compact(usage.output_tokens)
                    ),
                ));
                usage_rows.push((
                    "Cached input".to_string(),
                    Self::format_tokens_compact(usage.cached_input_tokens),
                ));
                usage_rows.push((
                    "Reasoning output".to_string(),
                    Self::format_tokens_compact(usage.reasoning_output_tokens),
                ));
            }
            if let Some(window) = usage.model_context_window {
                let remaining_pct = if window > 0 {
                    (100.0 - ((usage.total_tokens as f64 / window as f64) * 100.0))
                        .clamp(0.0, 100.0)
                } else {
                    100.0
                };
                usage_rows.push((
                    "Context window".to_string(),
                    format!(
                        "{:.1}% left ({} used / {})",
                        remaining_pct,
                        Self::format_tokens_compact(usage.total_tokens),
                        Self::format_tokens_compact(window)
                    ),
                ));
            }
            if !prefer_limits_only {
                if let Some(turn_id) = &usage.turn_id {
                    usage_rows.push(("Last turn".to_string(), turn_id.clone()));
                }
            }
        } else if !prefer_limits_only {
            usage_rows.push((
                "Token usage".to_string(),
                "unavailable (no turn usage update yet)".to_string(),
            ));
        }

        let mut limit_rows: Vec<(String, String)> = Vec::new();
        if let Some(snapshot) = &self.rate_limit_snapshot {
            if let Some(limit_name) = &snapshot.limit_name {
                limit_rows.push(("Limits profile".to_string(), limit_name.clone()));
            } else if let Some(limit_id) = &snapshot.limit_id {
                limit_rows.push(("Limits profile".to_string(), limit_id.clone()));
            }
            let primary_label = snapshot
                .primary
                .as_ref()
                .and_then(|window| window.window_duration_mins)
                .map(Self::format_limit_duration_label)
                .unwrap_or_else(|| "5h".to_string());
            if let Some(window) = &snapshot.primary {
                let remaining = (100.0 - window.used_percent).clamp(0.0, 100.0);
                limit_rows.push((
                    format!("{} limit", primary_label),
                    format!(
                        "{} {}",
                        Self::render_status_limit_progress_bar(remaining),
                        Self::format_limit_window_summary(window)
                    ),
                ));
            }
            if let Some(window) = &snapshot.secondary {
                let remaining = (100.0 - window.used_percent).clamp(0.0, 100.0);
                limit_rows.push((
                    "weekly limit".to_string(),
                    format!(
                        "{} {}",
                        Self::render_status_limit_progress_bar(remaining),
                        Self::format_limit_window_summary(window)
                    ),
                ));
            }

            if snapshot.credits_has_credits == Some(true) {
                if snapshot.credits_unlimited == Some(true) {
                    limit_rows.push(("Credits".to_string(), "unlimited".to_string()));
                } else if let Some(balance) = &snapshot.credits_balance {
                    limit_rows.push(("Credits".to_string(), balance.clone()));
                }
            }

            if let Some(plan_type) = &snapshot.plan_type {
                limit_rows.push(("Plan".to_string(), plan_type.clone()));
            }
        } else if prefer_limits_only {
            limit_rows.push(("Limits".to_string(), "data not available yet".to_string()));
        }

        let supervisor_rows = vec![
            (
                "Violations".to_string(),
                self.violations_detected.to_string(),
            ),
            ("Corrections".to_string(), self.corrections_made.to_string()),
            ("Auto replies".to_string(), self.auto_replies.to_string()),
        ];

        let label_width = overview_rows
            .iter()
            .chain(usage_rows.iter())
            .chain(limit_rows.iter())
            .chain(supervisor_rows.iter())
            .map(|(label, _)| label.chars().count())
            .max()
            .unwrap_or(12);

        let mut content_lines = vec![
            format!(" >_ OpenAI Codex (v{})", env!("CARGO_PKG_VERSION")),
            String::new(),
            "Visit https://chatgpt.com/codex/settings/usage for up-to-date".to_string(),
            "information on rate limits and credits".to_string(),
            String::new(),
        ];

        Self::append_status_rows(&mut content_lines, &overview_rows, label_width);
        if !usage_rows.is_empty() {
            content_lines.push(String::new());
            Self::append_status_rows(&mut content_lines, &usage_rows, label_width);
        }
        if !limit_rows.is_empty() {
            content_lines.push(String::new());
            Self::append_status_rows(&mut content_lines, &limit_rows, label_width);
        }
        content_lines.push(String::new());
        Self::append_status_rows(&mut content_lines, &supervisor_rows, label_width);

        format!("/status\n\n{}", self.render_status_card(&content_lines))
    }

    fn render_gugugaga_stats_summary(&self) -> String {
        let rows = vec![
            (
                "Violations".to_string(),
                self.violations_detected.to_string(),
            ),
            ("Corrections".to_string(), self.corrections_made.to_string()),
            ("Auto replies".to_string(), self.auto_replies.to_string()),
            (
                "Current activity".to_string(),
                self.notebook_current_activity
                    .clone()
                    .unwrap_or_else(|| "-".to_string()),
            ),
        ];

        let label_width = rows
            .iter()
            .map(|(label, _)| label.chars().count())
            .max()
            .unwrap_or(12);

        let mut content_lines = vec![
            " >_ Gugugaga Supervisor".to_string(),
            String::new(),
            "Session monitoring status".to_string(),
            String::new(),
        ];
        Self::append_status_rows(&mut content_lines, &rows, label_width);

        format!("//stats\n\n{}", self.render_status_card(&content_lines))
    }

    fn status_prefers_limits_over_tokens(&self) -> bool {
        match self
            .account_auth_mode
            .as_deref()
            .map(|m| m.to_ascii_lowercase())
        {
            Some(mode) if mode == "chatgpt" || mode == "chatgptauthtokens" => true,
            Some(mode) if mode == "apikey" || mode == "api_key" => false,
            _ => self.rate_limit_snapshot.is_some(),
        }
    }

    fn append_status_rows(lines: &mut Vec<String>, rows: &[(String, String)], label_width: usize) {
        for (label, value) in rows {
            lines.push(format!("  {label:<label_width$}: {value}"));
        }
    }

    fn status_agents_summary(&self) -> String {
        let cwd = Path::new(&self.cwd);
        let mut found = Vec::new();
        for name in ["AGENTS.md", "Agents.md", "agents.md"] {
            if cwd.join(name).exists() {
                found.push(name.to_string());
            }
        }
        if found.is_empty() {
            "<none>".to_string()
        } else {
            found.join(", ")
        }
    }

    fn status_card_width(&self) -> usize {
        let available = usize::from(self.msg_inner_rect.width);
        if available > 8 {
            available.saturating_sub(2).clamp(58, 96)
        } else {
            80
        }
    }

    fn render_status_card(&self, content_lines: &[String]) -> String {
        let width = self.status_card_width();
        let inner_width = width.saturating_sub(2);
        let text_width = inner_width.saturating_sub(2);
        if text_width == 0 {
            return content_lines.join("\n");
        }

        let mut output = Vec::new();
        output.push(format!("╭{}╮", "─".repeat(inner_width)));
        for line in content_lines {
            let wrapped = Self::wrap_status_line(line, text_width);
            for part in wrapped {
                output.push(format!("│ {:<text_width$} │", part));
            }
        }
        output.push(format!("╰{}╯", "─".repeat(inner_width)));
        output.join("\n")
    }

    fn wrap_status_line(line: &str, width: usize) -> Vec<String> {
        if width == 0 {
            return vec![String::new()];
        }
        if line.is_empty() {
            return vec![String::new()];
        }

        let mut out = Vec::new();
        let mut current = String::new();
        let mut current_width = 0usize;

        for ch in line.chars() {
            if current_width >= width {
                out.push(current);
                current = String::new();
                current_width = 0;
            }
            current.push(ch);
            current_width += 1;
        }

        if !current.is_empty() {
            out.push(current);
        }

        if out.is_empty() {
            out.push(String::new());
        }

        out
    }

    fn render_status_limit_progress_bar(percent_remaining: f64) -> String {
        const SEGMENTS: usize = 20;
        let ratio = (percent_remaining / 100.0).clamp(0.0, 1.0);
        let filled = ((ratio * SEGMENTS as f64).round() as usize).min(SEGMENTS);
        let empty = SEGMENTS.saturating_sub(filled);
        format!("[{}{}]", "█".repeat(filled), "░".repeat(empty))
    }

    fn format_tokens_compact(value: i64) -> String {
        let value = value.max(0);
        if value < 1_000 {
            return value.to_string();
        }

        let value_f64 = value as f64;
        let (scaled, suffix) = if value >= 1_000_000_000_000 {
            (value_f64 / 1_000_000_000_000.0, "T")
        } else if value >= 1_000_000_000 {
            (value_f64 / 1_000_000_000.0, "B")
        } else if value >= 1_000_000 {
            (value_f64 / 1_000_000.0, "M")
        } else {
            (value_f64 / 1_000.0, "K")
        };

        let decimals = if scaled < 10.0 {
            2
        } else if scaled < 100.0 {
            1
        } else {
            0
        };

        let mut formatted = format!("{scaled:.decimals$}");
        if formatted.contains('.') {
            while formatted.ends_with('0') {
                formatted.pop();
            }
            if formatted.ends_with('.') {
                formatted.pop();
            }
        }
        format!("{formatted}{suffix}")
    }

    fn format_limit_duration_label(window_mins: i64) -> String {
        if window_mins % (60 * 24 * 7) == 0 {
            "weekly".to_string()
        } else if window_mins % 60 == 0 {
            format!("{}h", window_mins / 60)
        } else {
            format!("{}m", window_mins)
        }
    }

    fn format_limit_window_summary(window: &RateLimitWindowSnapshot) -> String {
        let remaining = (100.0 - window.used_percent).clamp(0.0, 100.0);
        let mut summary = format!("{remaining:.0}% left");
        if let Some(resets_at) = window.resets_at {
            summary.push_str(&Self::format_reset_delta(resets_at));
        }
        summary
    }

    fn format_reset_delta(raw_ts: i64) -> String {
        let timestamp = if raw_ts > 10_000_000_000 {
            raw_ts / 1000
        } else {
            raw_ts
        };
        let now_secs = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(timestamp);
        if timestamp <= now_secs {
            return " (resetting now)".to_string();
        }
        let delta = timestamp - now_secs;
        if delta < 3600 {
            format!(" (resets in {}m)", (delta + 59) / 60)
        } else if delta < 86_400 {
            format!(" (resets in {}h)", (delta + 3599) / 3600)
        } else {
            format!(" (resets in {}d)", (delta + 86_399) / 86_400)
        }
    }

    fn parse_rate_limit_window(value: &serde_json::Value) -> Option<RateLimitWindowSnapshot> {
        if !value.is_object() {
            return None;
        }
        Some(RateLimitWindowSnapshot {
            used_percent: value
                .get("usedPercent")
                .or_else(|| value.get("used_percent"))
                .and_then(|v| v.as_f64())
                .unwrap_or(0.0),
            window_duration_mins: value
                .get("windowDurationMins")
                .or_else(|| value.get("window_duration_mins"))
                .and_then(|v| v.as_i64()),
            resets_at: value
                .get("resetsAt")
                .or_else(|| value.get("resets_at"))
                .and_then(|v| v.as_i64()),
        })
    }

    fn parse_rate_limit_snapshot(value: &serde_json::Value) -> Option<RateLimitSnapshotCache> {
        if !value.is_object() {
            return None;
        }
        let credits = value.get("credits");
        Some(RateLimitSnapshotCache {
            limit_id: value
                .get("limitId")
                .or_else(|| value.get("limit_id"))
                .and_then(|v| v.as_str())
                .map(|s| s.to_string()),
            limit_name: value
                .get("limitName")
                .or_else(|| value.get("limit_name"))
                .and_then(|v| v.as_str())
                .map(|s| s.to_string()),
            plan_type: value
                .get("planType")
                .or_else(|| value.get("plan_type"))
                .and_then(|v| v.as_str())
                .map(|s| s.to_string()),
            primary: value.get("primary").and_then(Self::parse_rate_limit_window),
            secondary: value
                .get("secondary")
                .and_then(Self::parse_rate_limit_window),
            credits_has_credits: credits
                .and_then(|v| v.get("hasCredits").or_else(|| v.get("has_credits")))
                .and_then(|v| v.as_bool()),
            credits_unlimited: credits
                .and_then(|v| v.get("unlimited"))
                .and_then(|v| v.as_bool()),
            credits_balance: credits
                .and_then(|v| v.get("balance"))
                .and_then(|v| v.as_str())
                .map(|s| s.to_string()),
        })
    }

    async fn run_git_capture(&self, args: &[&str]) -> Result<String, String> {
        self.run_git_capture_allowing_diff_exit(args, false).await
    }

    async fn run_git_capture_allowing_diff_exit(
        &self,
        args: &[&str],
        allow_diff_exit_code: bool,
    ) -> Result<String, String> {
        let output = tokio::process::Command::new("git")
            .args(args)
            .current_dir(&self.cwd)
            .output()
            .await
            .map_err(|e| e.to_string())?;

        let is_diff_exit = output.status.code() == Some(1);
        if output.status.success() || (allow_diff_exit_code && is_diff_exit) {
            Ok(String::from_utf8_lossy(&output.stdout).to_string())
        } else {
            Err(String::from_utf8_lossy(&output.stderr).to_string())
        }
    }

    /// Show git diff, including untracked file patches.
    async fn show_git_diff(&mut self) {
        match self
            .run_git_capture(&["rev-parse", "--is-inside-work-tree"])
            .await
        {
            Ok(s) if s.trim() == "true" => {}
            _ => {
                self.messages
                    .push(Message::system("`/diff` — not inside a git repository."));
                return;
            }
        }

        let tracked = self
            .run_git_capture(&["--no-pager", "diff", "--no-ext-diff"])
            .await
            .unwrap_or_default();
        let untracked_files = self
            .run_git_capture(&["ls-files", "--others", "--exclude-standard"])
            .await
            .unwrap_or_default();

        let mut untracked_patches = String::new();
        let null_path = if cfg!(windows) { "NUL" } else { "/dev/null" };
        for file in untracked_files
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
        {
            let args = [
                "--no-pager",
                "diff",
                "--no-ext-diff",
                "--no-index",
                "--",
                null_path,
                file,
            ];
            if let Ok(patch) = self.run_git_capture_allowing_diff_exit(&args, true).await {
                untracked_patches.push_str(&patch);
            }
        }

        let mut body = format!("{}{}", tracked, untracked_patches);
        if body.trim().is_empty() {
            self.messages.push(Message::system("Working tree clean."));
            return;
        }

        const MAX_DIFF_CHARS: usize = 24_000;
        if body.len() > MAX_DIFF_CHARS {
            body = format!(
                "{}\n... (diff truncated)",
                truncate_utf8(&body, MAX_DIFF_CHARS)
            );
        }
        self.messages.push(Message::system(&body));
    }

    /// Show currently active command execution terminals.
    fn show_background_processes(&mut self) {
        let mut lines = vec!["Background terminals".to_string(), String::new()];

        if self.active_terminals.is_empty() {
            lines.push("  • No background terminals running.".to_string());
            self.messages.push(Message::system(lines.join("\n")));
            return;
        }

        let mut entries: Vec<(&String, &ActiveTerminal)> = self.active_terminals.iter().collect();
        entries.sort_by(|(_, a), (_, b)| a.command.cmp(&b.command));
        let max_processes = 16usize;

        for (_, term) in entries.iter().take(max_processes) {
            let first_line = term.command.lines().next().unwrap_or(term.command.as_str());
            let mut command_preview = truncate_utf8(first_line, 80).to_string();
            if term.command.contains('\n') || first_line.len() > command_preview.len() {
                command_preview.push_str(" [...]");
            }
            lines.push(format!("  • {}", command_preview));

            let mut chunks: Vec<String> = term
                .recent_output
                .lines()
                .map(str::trim_end)
                .filter(|line| !line.trim().is_empty())
                .rev()
                .take(2)
                .map(|line| truncate_utf8(line, 120).to_string())
                .collect();
            chunks.reverse();
            if chunks.is_empty() {
                lines.push("    ↳ (no output yet)".to_string());
            } else {
                for (idx, chunk) in chunks.iter().enumerate() {
                    let prefix = if idx == 0 { "    ↳ " } else { "      " };
                    lines.push(format!("{}{}", prefix, chunk));
                }
            }
        }

        let remaining = entries.len().saturating_sub(max_processes);
        if remaining > 0 {
            lines.push(format!("  • ... and {} more running", remaining));
        }

        self.messages.push(Message::system(lines.join("\n")));
    }

    /// Trigger thread compaction via app-server.
    async fn request_thread_compaction(&mut self) {
        let Some(thread_id) = self.thread_id.clone() else {
            self.messages.push(Message::system(
                "No active thread. Start or resume a session first.",
            ));
            return;
        };

        if let Some(tx) = &self.input_tx {
            self.request_counter += 1;
            track_pending_request(
                &mut self.pending_request_type,
                &mut self.pending_requests,
                self.request_counter,
                PendingRequestType::ThreadCompactStart,
            );
            let msg = serde_json::json!({
                "jsonrpc": "2.0",
                "method": "thread/compact/start",
                "id": self.request_counter,
                "params": {
                    "threadId": thread_id
                }
            })
            .to_string();
            let _ = tx.send(msg).await;
            self.messages
                .push(Message::system("Context compaction requested..."));
        }
    }

    /// Stop all background terminals for current thread.
    async fn request_background_terminals_clean(&mut self) {
        let Some(thread_id) = self.thread_id.clone() else {
            self.messages.push(Message::system(
                "No active thread. Start or resume a session first.",
            ));
            return;
        };

        if let Some(tx) = &self.input_tx {
            self.request_counter += 1;
            track_pending_request(
                &mut self.pending_request_type,
                &mut self.pending_requests,
                self.request_counter,
                PendingRequestType::ThreadBackgroundTerminalsClean,
            );
            let msg = serde_json::json!({
                "jsonrpc": "2.0",
                "method": "thread/backgroundTerminals/clean",
                "id": self.request_counter,
                "params": {
                    "threadId": thread_id
                }
            })
            .to_string();
            let _ = tx.send(msg).await;
            self.messages
                .push(Message::system("Stopping background terminals..."));
        }
    }

    /// Request full config with layers for /debug-config.
    async fn request_debug_config(&mut self) {
        if let Some(tx) = &self.input_tx {
            self.request_counter += 1;
            track_pending_request(
                &mut self.pending_request_type,
                &mut self.pending_requests,
                self.request_counter,
                PendingRequestType::DebugConfigRead,
            );
            let msg = serde_json::json!({
                "jsonrpc": "2.0",
                "method": "config/read",
                "id": self.request_counter,
                "params": {
                    "includeLayers": true,
                    "cwd": self.cwd.clone()
                }
            })
            .to_string();
            let _ = tx.send(msg).await;
            self.messages
                .push(Message::system("Fetching config layers..."));
        }
    }

    /// Show rollout path for the current thread by looking it up in thread/list.
    async fn show_rollout_path(&mut self) {
        let Some(thread_id) = self.thread_id.clone() else {
            self.messages.push(Message::system(
                "No active thread. Start or resume a session first.",
            ));
            return;
        };

        if let Some(tx) = &self.input_tx {
            self.request_counter += 1;
            track_pending_request(
                &mut self.pending_request_type,
                &mut self.pending_requests,
                self.request_counter,
                PendingRequestType::RolloutPathLookup(thread_id),
            );
            let msg = serde_json::json!({
                "jsonrpc": "2.0",
                "method": "thread/list",
                "id": self.request_counter,
                "params": {}
            })
            .to_string();
            let _ = tx.send(msg).await;
            self.messages
                .push(Message::system("Fetching rollout path..."));
        }
    }

    /// Request model list
    async fn request_model_list(&mut self) {
        self.picker_mode = PickerMode::Model;
        self.picker.title = "Select Model".to_string();
        self.picker.open_loading();
        self.available_models.clear();
        self.pending_model_for_reasoning = None;
        self.pending_gugugaga_model_for_reasoning = None;

        if let Some(tx) = &self.input_tx {
            self.request_counter += 1;
            track_pending_request(
                &mut self.pending_request_type,
                &mut self.pending_requests,
                self.request_counter,
                PendingRequestType::ModelList,
            );
            let msg = serde_json::json!({
                "jsonrpc": "2.0",
                "method": "model/list",
                "id": self.request_counter,
                "params": {}
            })
            .to_string();
            let _ = tx.send(msg).await;
        }
    }

    /// Request model list for gugugaga model picker (reuses Codex's model/list RPC)
    async fn request_gugugaga_model_list(&mut self) {
        self.picker_mode = PickerMode::GugugagaModel;
        self.picker.title = "Select Model".to_string();
        self.picker.open_loading();
        self.available_models.clear();
        self.pending_model_for_reasoning = None;
        self.pending_gugugaga_model_for_reasoning = None;

        if let Some(tx) = &self.input_tx {
            self.request_counter += 1;
            track_pending_request(
                &mut self.pending_request_type,
                &mut self.pending_requests,
                self.request_counter,
                PendingRequestType::GugugagaModelList,
            );
            let msg = serde_json::json!({
                "jsonrpc": "2.0",
                "method": "model/list",
                "id": self.request_counter,
                "params": {}
            })
            .to_string();
            let _ = tx.send(msg).await;
        }
    }

    /// Directly set gugugaga model by name (//model <name>)
    async fn handle_gugugaga_model_set(&mut self, model: &str) {
        let config_path = Self::codex_home_dir().join("config.toml");
        let effort = Self::read_gugugaga_model_reasoning_effort(&config_path).await;
        self.persist_gugugaga_model_selection(model, effort.as_deref())
            .await;
        let suffix = effort
            .as_deref()
            .map(|e| format!(" (reasoning: {e})"))
            .unwrap_or_default();
        self.messages.push(Message::system(format!(
            "Gugugaga model set to: {}{}\n(Restart gugugaga to apply)",
            model, suffix
        )));
    }

    /// Open skills menu - first level picker
    async fn open_skills_menu(&mut self) {
        self.picker_mode = PickerMode::SkillsMenu;
        self.picker.title = "Skills".to_string();
        let items = vec![
            PickerItem {
                id: "list".to_string(),
                title: "List skills".to_string(),
                subtitle: "Show all available skills".to_string(),
                metadata: None,
            },
            PickerItem {
                id: "manage".to_string(),
                title: "Enable/Disable skills".to_string(),
                subtitle: "Toggle individual skills on/off".to_string(),
                metadata: None,
            },
        ];
        self.picker.open(items);
    }

    /// Fetch skills list from Codex (used by both list and manage flows)
    async fn fetch_skills_list(&mut self) {
        if let Some(tx) = &self.input_tx {
            self.request_counter += 1;
            track_pending_request(
                &mut self.pending_request_type,
                &mut self.pending_requests,
                self.request_counter,
                PendingRequestType::SkillsList,
            );
            let msg = serde_json::json!({
                "jsonrpc": "2.0",
                "method": "skills/list",
                "id": self.request_counter,
                "params": {
                    "cwds": [self.cwd.clone()]
                }
            })
            .to_string();
            let _ = tx.send(msg).await;
        }
    }

    /// Toggle a skill's enabled state via skills/config/write
    /// skill_id is formatted as "enabled:path" or "disabled:path"
    async fn toggle_skill(&mut self, skill_path: &str, currently_enabled: bool) {
        if let Some(tx) = &self.input_tx {
            self.request_counter += 1;
            let msg = serde_json::json!({
                "jsonrpc": "2.0",
                "method": "skills/config/write",
                "id": self.request_counter,
                "params": {
                    "path": skill_path,
                    "enabled": !currently_enabled
                }
            })
            .to_string();
            let _ = tx.send(msg).await;
            let action = if currently_enabled {
                "Disabled"
            } else {
                "Enabled"
            };
            // Extract just the skill name from the path for display
            let display_name = std::path::Path::new(skill_path)
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or(skill_path);
            self.messages.push(Message::system(format!(
                "{} skill: {}",
                action, display_name
            )));
        }
    }

    /// Open review preset picker (Codex-style entry for /review with no args).
    fn open_review_picker(&mut self) {
        if self.thread_id.is_none() {
            self.messages.push(Message::system(
                "No active thread. Start a conversation first.",
            ));
            return;
        }

        self.picker_mode = PickerMode::ReviewPreset;
        self.picker.title = "Select Review Preset".to_string();
        self.picker.open(vec![
            PickerItem {
                id: "base-branch".to_string(),
                title: "Review against a base branch".to_string(),
                subtitle: "PR-style review against a selected branch".to_string(),
                metadata: None,
            },
            PickerItem {
                id: "uncommitted".to_string(),
                title: "Review uncommitted changes".to_string(),
                subtitle: "Review staged, unstaged, and untracked files".to_string(),
                metadata: None,
            },
            PickerItem {
                id: "commit".to_string(),
                title: "Review a commit".to_string(),
                subtitle: "Pick a recent commit to review".to_string(),
                metadata: None,
            },
            PickerItem {
                id: "custom".to_string(),
                title: "Custom review instructions".to_string(),
                subtitle: "Type custom reviewer instructions".to_string(),
                metadata: None,
            },
        ]);
    }

    async fn open_review_branch_picker(&mut self) {
        match self
            .run_git_capture(&["rev-parse", "--is-inside-work-tree"])
            .await
        {
            Ok(s) if s.trim() == "true" => {}
            _ => {
                self.messages
                    .push(Message::system("`/review` — not inside a git repository."));
                return;
            }
        }

        let branches_raw = match self
            .run_git_capture(&["for-each-ref", "--format=%(refname:short)", "refs/heads"])
            .await
        {
            Ok(output) => output,
            Err(err) => {
                self.messages.push(Message::system(format!(
                    "Failed to list branches for review: {}",
                    err.trim()
                )));
                return;
            }
        };

        let current_branch = self
            .run_git_capture(&["branch", "--show-current"])
            .await
            .unwrap_or_default();
        let current_branch = current_branch.trim();

        let mut branches: Vec<String> = branches_raw
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .map(ToOwned::to_owned)
            .collect();
        branches.sort();
        branches.dedup();

        if branches.is_empty() {
            self.messages
                .push(Message::system("No local branches found for review."));
            return;
        }

        let items: Vec<PickerItem> = branches
            .into_iter()
            .map(|branch| {
                let title = if current_branch.is_empty() {
                    branch.clone()
                } else {
                    format!("{current_branch} -> {branch}")
                };
                PickerItem {
                    id: branch,
                    title,
                    subtitle: "Review diff against this branch".to_string(),
                    metadata: None,
                }
            })
            .collect();

        self.picker_mode = PickerMode::ReviewBranch;
        self.picker.title = "Select Base Branch".to_string();
        self.picker.open(items);
    }

    async fn open_review_commit_picker(&mut self) {
        match self
            .run_git_capture(&["rev-parse", "--is-inside-work-tree"])
            .await
        {
            Ok(s) if s.trim() == "true" => {}
            _ => {
                self.messages
                    .push(Message::system("`/review` — not inside a git repository."));
                return;
            }
        }

        let commits_raw = match self
            .run_git_capture(&["--no-pager", "log", "--oneline", "-n", "100"])
            .await
        {
            Ok(output) => output,
            Err(err) => {
                self.messages.push(Message::system(format!(
                    "Failed to list commits for review: {}",
                    err.trim()
                )));
                return;
            }
        };

        let items: Vec<PickerItem> = commits_raw
            .lines()
            .filter_map(|line| {
                let (sha, title) = line.trim().split_once(' ')?;
                if sha.is_empty() || title.is_empty() {
                    return None;
                }
                Some(PickerItem {
                    id: sha.to_string(),
                    title: format!("{} {}", sha, truncate_utf8(title, 72)),
                    subtitle: "Review this commit".to_string(),
                    metadata: Some(title.to_string()),
                })
            })
            .collect();

        if items.is_empty() {
            self.messages.push(Message::system(
                "No recent commits available for commit review.",
            ));
            return;
        }

        self.picker_mode = PickerMode::ReviewCommit;
        self.picker.title = "Review a Commit".to_string();
        self.picker.open(items);
    }

    async fn request_review_start(&mut self, target: serde_json::Value, status_msg: String) {
        if let Some(thread_id) = &self.thread_id {
            if let Some(tx) = &self.input_tx {
                self.request_counter += 1;
                let msg = serde_json::json!({
                    "jsonrpc": "2.0",
                    "method": "review/start",
                    "id": self.request_counter,
                    "params": {
                        "threadId": thread_id,
                        "delivery": "inline",
                        "target": target
                    }
                })
                .to_string();
                let _ = tx.send(msg).await;
                self.messages.push(Message::system(status_msg));
                self.start_processing();
            }
        } else {
            self.messages.push(Message::system(
                "No active thread. Start a conversation first.",
            ));
        }
    }

    async fn request_review_uncommitted(&mut self) {
        self.request_review_start(
            serde_json::json!({
                "type": "uncommittedChanges"
            }),
            "Starting code review (uncommitted changes)...".to_string(),
        )
        .await;
    }

    async fn request_review_base_branch(&mut self, branch: &str) {
        self.request_review_start(
            serde_json::json!({
                "type": "baseBranch",
                "branch": branch
            }),
            format!("Starting code review against base branch `{branch}`..."),
        )
        .await;
    }

    async fn request_review_commit(&mut self, sha: &str, title: Option<&str>) {
        let mut target = serde_json::json!({
            "type": "commit",
            "sha": sha
        });
        if let Some(title) = title.map(str::trim).filter(|s| !s.is_empty()) {
            target["title"] = serde_json::json!(title);
        }

        self.request_review_start(
            target,
            format!("Starting code review for commit `{sha}`..."),
        )
        .await;
    }

    async fn request_review_custom(&mut self, instructions: &str) {
        let trimmed = instructions.trim();
        if trimmed.is_empty() {
            self.messages.push(Message::system(
                "Review instructions cannot be empty. Run /review and choose a preset.",
            ));
            return;
        }

        self.request_review_start(
            serde_json::json!({
                "type": "custom",
                "instructions": trimmed
            }),
            "Starting code review (custom instructions)...".to_string(),
        )
        .await;
    }

    /// Request thread rename
    async fn request_rename(&mut self, name: &str) {
        if let Some(thread_id) = &self.thread_id {
            if let Some(tx) = &self.input_tx {
                self.request_counter += 1;
                track_pending_request(
                    &mut self.pending_request_type,
                    &mut self.pending_requests,
                    self.request_counter,
                    PendingRequestType::RenameThread,
                );
                let msg = serde_json::json!({
                    "jsonrpc": "2.0",
                    "method": "thread/name/set",
                    "id": self.request_counter,
                    "params": {
                        "threadId": thread_id,
                        "name": name
                    }
                })
                .to_string();
                let _ = tx.send(msg).await;
                self.messages
                    .push(Message::system(format!("Renaming to: {}", name)));
            }
        } else {
            self.messages
                .push(Message::system("No active thread to rename"));
        }
    }

    /// Request thread fork
    async fn request_fork(&mut self) {
        if let Some(thread_id) = &self.thread_id {
            if let Some(tx) = &self.input_tx {
                self.request_counter += 1;
                track_pending_request(
                    &mut self.pending_request_type,
                    &mut self.pending_requests,
                    self.request_counter,
                    PendingRequestType::ForkThread,
                );
                let msg = serde_json::json!({
                    "jsonrpc": "2.0",
                    "method": "thread/fork",
                    "id": self.request_counter,
                    "params": {
                        "threadId": thread_id
                    }
                })
                .to_string();
                let _ = tx.send(msg).await;
                self.messages.push(Message::system("Forking session..."));
            }
        } else {
            self.messages
                .push(Message::system("No active thread to fork"));
        }
    }

    /// Request logout
    async fn request_logout(&mut self) {
        if let Some(tx) = &self.input_tx {
            self.request_counter += 1;
            track_pending_request(
                &mut self.pending_request_type,
                &mut self.pending_requests,
                self.request_counter,
                PendingRequestType::Logout,
            );
            let msg = serde_json::json!({
                "jsonrpc": "2.0",
                "method": "account/logout",
                "id": self.request_counter,
                "params": null
            })
            .to_string();
            let _ = tx.send(msg).await;
            self.messages.push(Message::system("Logging out..."));
        }
    }

    fn open_feedback_category_picker(&mut self) {
        if self.thread_id.is_none() {
            self.messages.push(Message::system(
                "No active thread. Start a conversation first.",
            ));
            return;
        }

        self.picker_mode = PickerMode::FeedbackCategory;
        self.picker.title = "How was this?".to_string();
        self.picker.open(vec![
            PickerItem {
                id: "bug".to_string(),
                title: "bug".to_string(),
                subtitle: "Crash, error message, hang, or broken UI/behavior.".to_string(),
                metadata: None,
            },
            PickerItem {
                id: "bad_result".to_string(),
                title: "bad result".to_string(),
                subtitle: "Output was off-target, incorrect, incomplete, or unhelpful.".to_string(),
                metadata: None,
            },
            PickerItem {
                id: "good_result".to_string(),
                title: "good result".to_string(),
                subtitle: "Helpful, correct, high-quality, or delightful result.".to_string(),
                metadata: None,
            },
            PickerItem {
                id: "safety_check".to_string(),
                title: "safety check".to_string(),
                subtitle: "Benign usage blocked due to safety checks or refusals.".to_string(),
                metadata: None,
            },
            PickerItem {
                id: "other".to_string(),
                title: "other".to_string(),
                subtitle: "Slowness, feature request, UX feedback, or anything else.".to_string(),
                metadata: None,
            },
        ]);
    }

    async fn request_feedback_upload(
        &mut self,
        classification: &str,
        reason: Option<&str>,
        include_logs: bool,
    ) {
        if let Some(tx) = &self.input_tx {
            self.request_counter += 1;
            track_pending_request(
                &mut self.pending_request_type,
                &mut self.pending_requests,
                self.request_counter,
                PendingRequestType::FeedbackUpload,
            );
            let msg = serde_json::json!({
                "jsonrpc": "2.0",
                "method": "feedback/upload",
                "id": self.request_counter,
                "params": {
                    "classification": classification,
                    "reason": reason,
                    "threadId": self.thread_id,
                    "includeLogs": include_logs
                }
            })
            .to_string();
            let _ = tx.send(msg).await;
            self.messages.push(Message::system(format!(
                "Uploading feedback (classification: {classification}, include logs: {})...",
                if include_logs { "yes" } else { "no" }
            )));
        }
    }

    /// Show MCP tools - query the server for MCP server statuses
    async fn show_mcp_tools(&mut self) {
        if let Some(tx) = &self.input_tx {
            self.request_counter += 1;
            track_pending_request(
                &mut self.pending_request_type,
                &mut self.pending_requests,
                self.request_counter,
                PendingRequestType::McpServerList,
            );
            let msg = serde_json::json!({
                "jsonrpc": "2.0",
                "method": "mcpServerStatus/list",
                "id": self.request_counter,
                "params": {}
            })
            .to_string();
            let _ = tx.send(msg).await;
            self.messages
                .push(Message::system("Fetching MCP servers..."));
        }
    }

    /// Request apps list
    async fn request_apps_list(&mut self) {
        if let Some(tx) = &self.input_tx {
            self.request_counter += 1;
            track_pending_request(
                &mut self.pending_request_type,
                &mut self.pending_requests,
                self.request_counter,
                PendingRequestType::AppsList,
            );
            let msg = serde_json::json!({
                "jsonrpc": "2.0",
                "method": "app/list",
                "id": self.request_counter,
                "params": {}
            })
            .to_string();
            let _ = tx.send(msg).await;
            self.messages.push(Message::system("Fetching apps..."));
        }
    }

    fn permissions_preset_items() -> Vec<PickerItem> {
        vec![
            PickerItem {
                id: "read-only".to_string(),
                title: "Read Only".to_string(),
                subtitle: "Codex can read files in the current workspace. Approval is required to edit files or access the internet.".to_string(),
                metadata: None,
            },
            PickerItem {
                id: "auto".to_string(),
                title: "Default".to_string(),
                subtitle: "Codex can read and edit files in the workspace. Approval is required to access the internet or edit other files.".to_string(),
                metadata: None,
            },
            PickerItem {
                id: "full-access".to_string(),
                title: "Full Access".to_string(),
                subtitle: "Codex can edit files outside this workspace and access the internet without asking.".to_string(),
                metadata: None,
            },
        ]
    }

    async fn apply_permissions_preset(&mut self, preset_id: &str, label: &str) {
        let (approval, sandbox) = match preset_id {
            "read-only" => ("on-request", "read-only"),
            "auto" => ("on-request", "workspace-write"),
            "full-access" => ("never", "danger-full-access"),
            other => {
                self.messages.push(Message::system(format!(
                    "Unknown permissions preset: {other}.",
                )));
                return;
            }
        };

        self.write_config("approvalPolicy", &serde_json::json!(approval))
            .await;
        self.write_config("sandboxPolicy", &serde_json::json!(sandbox))
            .await;
        self.messages
            .push(Message::system(format!("Permissions updated to {label}.")));
    }

    /// Open approvals picker (alias of /permissions, matching Codex behavior).
    async fn open_approvals_picker(&mut self) {
        self.open_permissions_picker().await;
    }

    /// Open permissions picker - shows approval+sandbox presets.
    async fn open_permissions_picker(&mut self) {
        self.picker_mode = PickerMode::Permissions;
        self.picker.title = "Update Model Permissions".to_string();
        self.picker.open(Self::permissions_preset_items());
    }

    /// Open personality picker
    async fn open_personality_picker(&mut self) {
        self.picker_mode = PickerMode::Personality;
        self.picker.title = "Personality".to_string();
        let items = vec![
            PickerItem {
                id: "friendly".to_string(),
                title: "Friendly".to_string(),
                subtitle: "Warm and encouraging".to_string(),
                metadata: None,
            },
            PickerItem {
                id: "pragmatic".to_string(),
                title: "Pragmatic".to_string(),
                subtitle: "Direct and efficient".to_string(),
                metadata: None,
            },
        ];
        self.picker.open(items);
    }

    /// Open experimental features picker - reads config first
    async fn open_experimental_picker(&mut self) {
        if let Some(tx) = &self.input_tx {
            self.request_counter += 1;
            track_pending_request(
                &mut self.pending_request_type,
                &mut self.pending_requests,
                self.request_counter,
                PendingRequestType::ConfigRead,
            );
            let msg = serde_json::json!({
                "jsonrpc": "2.0",
                "method": "config/read",
                "id": self.request_counter,
                "params": {}
            })
            .to_string();
            let _ = tx.send(msg).await;
            self.messages
                .push(Message::system("Fetching experimental features..."));
        }
    }

    /// Open collaboration mode picker
    async fn open_collab_picker(&mut self) {
        if !self.collaboration_modes_enabled() {
            self.messages.push(Message::system(
                "Collaboration modes are disabled.\nEnable collaboration modes to use /collab.",
            ));
            self.scroll_to_bottom();
            return;
        }

        self.picker_mode = PickerMode::Collab;
        self.picker.title = "Collaboration Mode".to_string();
        self.picker.open_loading();

        if let Some(tx) = &self.input_tx {
            self.request_counter += 1;
            track_pending_request(
                &mut self.pending_request_type,
                &mut self.pending_requests,
                self.request_counter,
                PendingRequestType::CollabModeList,
            );
            let msg = serde_json::json!({
                "jsonrpc": "2.0",
                "method": "collaborationMode/list",
                "id": self.request_counter,
                "params": {}
            })
            .to_string();
            let _ = tx.send(msg).await;
        }
    }

    fn normalize_collaboration_mode_key(mode: &str) -> String {
        let trimmed = mode.trim();
        if trimmed.is_empty() {
            "default".to_string()
        } else {
            trimmed.to_ascii_lowercase()
        }
    }

    fn parse_bool_like(value: &serde_json::Value) -> Option<bool> {
        if let Some(b) = value.as_bool() {
            return Some(b);
        }
        if let Some(s) = value.as_str() {
            let lowered = s.trim().to_ascii_lowercase();
            return match lowered.as_str() {
                "true" | "1" | "yes" | "on" => Some(true),
                "false" | "0" | "no" | "off" => Some(false),
                _ => None,
            };
        }
        value.as_i64().map(|n| n != 0)
    }

    fn parse_collaboration_modes_feature_flag(config_result: &serde_json::Value) -> Option<bool> {
        let config = config_result.get("config").unwrap_or(config_result);
        let features = config.get("features")?;
        features
            .get("collaboration_modes")
            .or_else(|| features.get("collaborationModes"))
            .or_else(|| features.get("collab"))
            .and_then(Self::parse_bool_like)
    }

    fn maybe_update_collaboration_modes_feature_flag(&mut self, config_result: &serde_json::Value) {
        if let Some(enabled) = Self::parse_collaboration_modes_feature_flag(config_result) {
            self.collaboration_modes_feature_enabled = Some(enabled);
        }
    }

    fn collaboration_modes_enabled(&self) -> bool {
        self.collaboration_modes_feature_enabled.unwrap_or(true)
    }

    fn collaboration_mode_title(mode: &str) -> String {
        match mode {
            "plan" => "Plan mode".to_string(),
            "default" => "Default mode".to_string(),
            other => {
                let mut chars = other.chars();
                match chars.next() {
                    Some(first) => {
                        let mut label = first.to_uppercase().to_string();
                        label.push_str(chars.as_str());
                        format!("{label} mode")
                    }
                    None => "Default mode".to_string(),
                }
            }
        }
    }

    fn set_collaboration_mode_options(&mut self, options: Vec<String>) {
        let mut dedup = Vec::<String>::new();
        for item in options {
            let normalized = Self::normalize_collaboration_mode_key(&item);
            if !dedup.iter().any(|existing| existing == &normalized) {
                dedup.push(normalized);
            }
        }
        if dedup.is_empty() {
            dedup.push("default".to_string());
            dedup.push("plan".to_string());
        }
        self.collaboration_mode_options = dedup;
    }

    fn maybe_update_collaboration_mode_from_config_result(&mut self, result: &serde_json::Value) {
        let config = result.get("config").unwrap_or(result);
        let mode = config
            .get("collaboration_mode")
            .or_else(|| config.get("collaborationMode"))
            .and_then(|value| value.as_str())
            .map(Self::normalize_collaboration_mode_key);
        if let Some(mode) = mode {
            self.collaboration_mode = Some(mode.clone());
            if !self
                .collaboration_mode_options
                .iter()
                .any(|item| item == &mode)
            {
                self.collaboration_mode_options.push(mode);
            }
        }
    }

    fn next_collaboration_mode_key(&self) -> String {
        let options = if self.collaboration_mode_options.is_empty() {
            vec!["default".to_string(), "plan".to_string()]
        } else {
            self.collaboration_mode_options.clone()
        };

        let current = self
            .collaboration_mode
            .as_deref()
            .map(Self::normalize_collaboration_mode_key);
        if let Some(current_mode) = current {
            if let Some(idx) = options.iter().position(|item| item == &current_mode) {
                return options[(idx + 1) % options.len()].clone();
            }
        }
        options
            .first()
            .cloned()
            .unwrap_or_else(|| "default".to_string())
    }

    async fn set_collaboration_mode(&mut self, mode: &str, announce: bool) {
        let normalized = Self::normalize_collaboration_mode_key(mode);
        self.collaboration_mode = Some(normalized.clone());
        if !self
            .collaboration_mode_options
            .iter()
            .any(|item| item == &normalized)
        {
            self.collaboration_mode_options.push(normalized.clone());
        }

        self.write_config("collaborationMode", &serde_json::json!(normalized))
            .await;

        if announce {
            self.messages.push(Message::system(format!(
                "Collaboration mode: {}",
                Self::collaboration_mode_title(
                    self.collaboration_mode.as_deref().unwrap_or("default")
                )
            )));
            self.scroll_to_bottom();
        }
    }

    async fn cycle_collaboration_mode_shortcut(&mut self) {
        let next = self.next_collaboration_mode_key();
        self.set_collaboration_mode(&next, true).await;
    }

    /// Set plan mode directly
    async fn set_plan_mode(&mut self) {
        if !self.collaboration_modes_enabled() {
            self.messages.push(Message::system(
                "Collaboration modes are disabled.\nEnable collaboration modes to use /plan.",
            ));
            self.scroll_to_bottom();
            return;
        }
        self.set_collaboration_mode("plan", true).await;
    }

    /// Open agent thread picker
    async fn open_agent_picker(&mut self) {
        self.picker_mode = PickerMode::Agent;
        self.picker.title = "Active Agents".to_string();
        self.picker.open_loading();

        if let Some(tx) = &self.input_tx {
            self.request_counter += 1;
            track_pending_request(
                &mut self.pending_request_type,
                &mut self.pending_requests,
                self.request_counter,
                PendingRequestType::AgentThreadList,
            );
            let msg = serde_json::json!({
                "jsonrpc": "2.0",
                "method": "thread/loaded/list",
                "id": self.request_counter,
                "params": {}
            })
            .to_string();
            let _ = tx.send(msg).await;
        }
    }

    /// Open status line editor by reading current config first.
    async fn open_statusline_picker(&mut self) {
        if let Some(tx) = &self.input_tx {
            self.request_counter += 1;
            track_pending_request(
                &mut self.pending_request_type,
                &mut self.pending_requests,
                self.request_counter,
                PendingRequestType::StatuslineConfigRead,
            );
            let msg = serde_json::json!({
                "jsonrpc": "2.0",
                "method": "config/read",
                "id": self.request_counter,
                "params": {
                    "includeLayers": false,
                    "cwd": self.cwd.clone()
                }
            })
            .to_string();
            let _ = tx.send(msg).await;
        } else {
            self.open_statusline_editor_with_items(Self::default_statusline_items());
        }
    }

    fn open_statusline_editor_with_items(&mut self, enabled_items: Vec<String>) {
        let selected_item = enabled_items.first().cloned();
        self.statusline_editor = Some(StatuslineEditorState {
            enabled_items,
            selected_item,
        });
        self.picker_mode = PickerMode::Statusline;
        self.picker.title = "Status Line".to_string();
        self.refresh_statusline_editor_picker();
    }

    fn refresh_statusline_editor_picker(&mut self) {
        let Some(editor) = self.statusline_editor.as_ref() else {
            self.picker.close();
            self.picker_mode = PickerMode::None;
            return;
        };

        let selected_item = editor.selected_item.clone();
        let selected_idx = selected_item
            .as_ref()
            .and_then(|id| editor.enabled_items.iter().position(|it| it == id));

        let mut items = Vec::new();
        items.push(PickerItem {
            id: "__statusline:save".to_string(),
            title: "Save changes".to_string(),
            subtitle: "Persist current selection and order".to_string(),
            metadata: None,
        });
        items.push(PickerItem {
            id: "__statusline:cancel".to_string(),
            title: "Cancel".to_string(),
            subtitle: "Discard unsaved status line edits".to_string(),
            metadata: None,
        });
        items.push(PickerItem {
            id: "__statusline:reset".to_string(),
            title: "Reset to default".to_string(),
            subtitle: "model-with-reasoning, context-remaining, current-dir".to_string(),
            metadata: None,
        });

        let up_hint = match selected_idx {
            Some(0) => "Selected item is already first".to_string(),
            Some(_) => "Move selected enabled item earlier".to_string(),
            None => "Select an enabled item first".to_string(),
        };
        items.push(PickerItem {
            id: "__statusline:up".to_string(),
            title: "Move selected up".to_string(),
            subtitle: up_hint,
            metadata: None,
        });

        let down_hint = match selected_idx {
            Some(idx) if idx + 1 >= editor.enabled_items.len() => {
                "Selected item is already last".to_string()
            }
            Some(_) => "Move selected enabled item later".to_string(),
            None => "Select an enabled item first".to_string(),
        };
        items.push(PickerItem {
            id: "__statusline:down".to_string(),
            title: "Move selected down".to_string(),
            subtitle: down_hint,
            metadata: None,
        });

        for (id, title, description, example) in STATUS_LINE_AVAILABLE_ITEMS {
            let enabled_idx = editor.enabled_items.iter().position(|item| item == id);
            let enabled = enabled_idx.is_some();
            let selected = selected_item.as_deref() == Some(id);
            let order = enabled_idx
                .map(|idx| format!("{:>2}. ", idx + 1))
                .unwrap_or_else(|| " -- ".to_string());
            let selected_marker = if selected { "  <- selected" } else { "" };
            let row_title = format!(
                "[{}] {}{}{}",
                if enabled { "x" } else { " " },
                order,
                title,
                selected_marker
            );
            let row_subtitle = format!("{} (example: {})", description, example);
            items.push(PickerItem {
                id: format!("item:{}", id),
                title: row_title,
                subtitle: row_subtitle,
                metadata: None,
            });
        }

        if self.picker.visible {
            self.picker.set_items(items);
        } else {
            self.picker.open(items);
        }
    }

    /// Execute /init command - creates AGENTS.md
    async fn execute_init(&mut self) {
        let init_target = std::path::Path::new(&self.cwd).join("AGENTS.md");
        if init_target.exists() {
            self.messages.push(Message::system(
                "AGENTS.md already exists. Skipping /init to avoid overwriting it.",
            ));
            return;
        }

        // Send a prompt to Codex to create the AGENTS.md file
        const INIT_PROMPT: &str = r#"Create an AGENTS.md file in the current directory that describes this project and provides guidance for AI agents working on it. Include:

1. Project overview and purpose
2. Key technologies and dependencies
3. Important directories and files
4. Coding conventions and style guidelines
5. Common tasks and how to perform them
6. Testing and build instructions

Make it comprehensive but concise."#;

        let msg = self.create_turn_message(INIT_PROMPT, &[]);
        if let Some(tx) = &self.input_tx {
            let _ = tx.send(msg).await;
            self.messages.push(Message::user("/init"));
            self.messages.push(Message::system("Creating AGENTS.md..."));
            self.start_processing();
        }
    }

    /// Open the resume picker by requesting thread list
    async fn open_resume_picker(&mut self) {
        self.picker_mode = PickerMode::Resume;
        self.picker.title = "Resume Session".to_string();
        self.picker.open_loading();

        // Request thread list from Codex
        if let Some(tx) = &self.input_tx {
            self.request_counter += 1;
            let request_id = self.request_counter;
            track_pending_request(
                &mut self.pending_request_type,
                &mut self.pending_requests,
                self.request_counter,
                PendingRequestType::ThreadList,
            );

            let msg = serde_json::json!({
                "jsonrpc": "2.0",
                "method": "thread/list",
                "id": request_id,
                "params": {}
            })
            .to_string();
            let _ = tx.send(msg).await;
        }
    }

    /// Handle picker selection
    async fn handle_picker_selection(&mut self) {
        if let Some(item) = self.picker.selected_item() {
            let item_id = item.id.clone();
            let item_title = item.title.clone();
            let item_metadata = item.metadata.clone();
            let mut keep_picker_open = false;

            match self.picker_mode {
                PickerMode::Resume => {
                    // Clear stale restore payload from any previous resume attempt.
                    self.pending_session_restore = None;
                    self.messages
                        .push(Message::system(format!("Resuming session: {}", item_title)));
                    if let Some(tx) = &self.input_tx {
                        self.request_counter += 1;
                        track_pending_request(
                            &mut self.pending_request_type,
                            &mut self.pending_requests,
                            self.request_counter,
                            PendingRequestType::ThreadResume(item_id.clone()),
                        );
                        // Build params with both threadId and path (if available).
                        // The path takes precedence in the app-server, bypassing
                        // the potentially unreliable UUID-based file search.
                        let mut params = serde_json::json!({
                            "threadId": item_id,
                            "sandbox": "workspace-write",
                            "approvalPolicy": "on-request",
                            "config": {
                                "experimental_use_freeform_apply_patch": true
                            }
                        });
                        if let Some(path) = &item_metadata {
                            params["path"] = serde_json::json!(path);
                        }
                        let msg = serde_json::json!({
                            "jsonrpc": "2.0",
                            "method": "thread/resume",
                            "id": self.request_counter,
                            "params": params
                        })
                        .to_string();
                        let _ = tx.send(msg).await;
                    }
                }
                PickerMode::ReviewPreset => match item_id.as_str() {
                    "base-branch" => {
                        self.open_review_branch_picker().await;
                        return;
                    }
                    "uncommitted" => {
                        self.request_review_uncommitted().await;
                    }
                    "commit" => {
                        self.open_review_commit_picker().await;
                        return;
                    }
                    "custom" => {
                        self.pending_review_custom_input = true;
                        self.messages.push(Message::system(
                            "Custom review: type instructions and press Enter (or /cancel).",
                        ));
                    }
                    _ => {}
                },
                PickerMode::ReviewBranch => {
                    self.request_review_base_branch(&item_id).await;
                }
                PickerMode::ReviewCommit => {
                    self.request_review_commit(&item_id, item_metadata.as_deref())
                        .await;
                }
                PickerMode::FeedbackCategory => {
                    self.pending_feedback_classification = Some(item_id);
                    self.picker_mode = PickerMode::FeedbackIncludeLogs;
                    self.picker.title = "Upload logs?".to_string();
                    self.picker.open(vec![
                        PickerItem {
                            id: "include-logs".to_string(),
                            title: "Yes, include logs".to_string(),
                            subtitle: "Include debug logs and rollout metadata".to_string(),
                            metadata: None,
                        },
                        PickerItem {
                            id: "no-logs".to_string(),
                            title: "No, send feedback only".to_string(),
                            subtitle: "Send only classification and optional note".to_string(),
                            metadata: None,
                        },
                    ]);
                    return;
                }
                PickerMode::FeedbackIncludeLogs => {
                    self.pending_feedback_include_logs = item_id == "include-logs";
                    self.pending_feedback_note_input = true;
                    self.messages.push(Message::system(
                        "Feedback note (optional): type details and press Enter. Use /skip or /cancel.",
                    ));
                }
                PickerMode::Model => {
                    if let Some(model) = self
                        .available_models
                        .iter()
                        .find(|m| m.id == item_id)
                        .cloned()
                    {
                        let default_effort = model.default_reasoning_effort.clone();
                        let supported_count = model.supported_reasoning_efforts.len();

                        if supported_count > 1 {
                            self.pending_model_for_reasoning = Some(model.clone());
                            self.picker_mode = PickerMode::ModelReasoning;
                            self.picker.title =
                                format!("Reasoning Effort ({})", model.display_name);

                            let mut items: Vec<PickerItem> = model
                                .supported_reasoning_efforts
                                .iter()
                                .map(|opt| {
                                    let is_default = default_effort
                                        .as_deref()
                                        .map(|d| d == opt.effort)
                                        .unwrap_or(false);
                                    let title = if is_default {
                                        format!("{} (default)", opt.effort)
                                    } else {
                                        opt.effort.clone()
                                    };
                                    PickerItem {
                                        id: opt.effort.clone(),
                                        title,
                                        subtitle: if opt.description.is_empty() {
                                            "Reasoning effort".to_string()
                                        } else {
                                            opt.description.clone()
                                        },
                                        metadata: None,
                                    }
                                })
                                .collect();

                            if items.is_empty() {
                                items.push(PickerItem {
                                    id: "default".to_string(),
                                    title: "default".to_string(),
                                    subtitle: "Use model default reasoning effort".to_string(),
                                    metadata: None,
                                });
                            }

                            self.picker.open(items);
                            self.messages.push(Message::system(format!(
                                "Model selected: {}. Choose reasoning effort.",
                                model.display_name
                            )));
                            return;
                        }

                        // No second stage needed (single effort or no effort metadata).
                        let selected_effort = model
                            .supported_reasoning_efforts
                            .first()
                            .map(|e| e.effort.clone())
                            .or(default_effort);

                        self.set_default_model(&item_id, selected_effort.as_deref())
                            .await;
                        let suffix = selected_effort
                            .as_deref()
                            .map(|e| format!(" (reasoning: {e})"))
                            .unwrap_or_default();
                        self.messages.push(Message::system(format!(
                            "Model set to: {}{}",
                            item_title, suffix
                        )));
                    } else {
                        // Fallback if model cache is missing for any reason.
                        self.set_default_model(&item_id, None).await;
                        self.messages
                            .push(Message::system(format!("Model set to: {}", item_title)));
                    }
                }
                PickerMode::ModelReasoning => {
                    if let Some(model) = self.pending_model_for_reasoning.take() {
                        let effort = if item_id == "default" {
                            model.default_reasoning_effort.as_deref()
                        } else {
                            Some(item_id.as_str())
                        };
                        self.set_default_model(&model.id, effort).await;
                        let suffix = effort
                            .map(|e| format!(" (reasoning: {e})"))
                            .unwrap_or_default();
                        self.messages.push(Message::system(format!(
                            "Model set to: {}{}",
                            model.display_name, suffix
                        )));
                    } else {
                        self.messages.push(Message::system(
                            "No model selected for reasoning effort. Please run /model again.",
                        ));
                    }
                }
                PickerMode::GugugagaModel => {
                    if let Some(model) = self
                        .available_models
                        .iter()
                        .find(|m| m.id == item_id)
                        .cloned()
                    {
                        let default_effort = model.default_reasoning_effort.clone();
                        let supported_count = model.supported_reasoning_efforts.len();

                        if supported_count > 1 {
                            self.pending_gugugaga_model_for_reasoning = Some(model.clone());
                            self.picker_mode = PickerMode::GugugagaModelReasoning;
                            self.picker.title =
                                format!("Reasoning Effort ({})", model.display_name);

                            let mut items: Vec<PickerItem> = model
                                .supported_reasoning_efforts
                                .iter()
                                .map(|opt| {
                                    let is_default = default_effort
                                        .as_deref()
                                        .map(|d| d == opt.effort)
                                        .unwrap_or(false);
                                    let title = if is_default {
                                        format!("{} (default)", opt.effort)
                                    } else {
                                        opt.effort.clone()
                                    };
                                    PickerItem {
                                        id: opt.effort.clone(),
                                        title,
                                        subtitle: if opt.description.is_empty() {
                                            "Reasoning effort".to_string()
                                        } else {
                                            opt.description.clone()
                                        },
                                        metadata: None,
                                    }
                                })
                                .collect();

                            if items.is_empty() {
                                items.push(PickerItem {
                                    id: "default".to_string(),
                                    title: "default".to_string(),
                                    subtitle: "Use model default reasoning effort".to_string(),
                                    metadata: None,
                                });
                            }

                            self.picker.open(items);
                            self.messages.push(Message::system(format!(
                                "Gugugaga model selected: {}. Choose reasoning effort.",
                                model.display_name
                            )));
                            return;
                        }

                        let selected_effort = model
                            .supported_reasoning_efforts
                            .first()
                            .map(|e| e.effort.clone())
                            .or(default_effort);

                        self.persist_gugugaga_model_selection(&item_id, selected_effort.as_deref())
                            .await;
                        let suffix = selected_effort
                            .as_deref()
                            .map(|e| format!(" (reasoning: {e})"))
                            .unwrap_or_default();
                        self.messages.push(Message::system(format!(
                            "Gugugaga model set to: {}{}\n(Restart gugugaga to apply)",
                            item_title, suffix
                        )));
                    } else {
                        self.persist_gugugaga_model_selection(&item_id, None).await;
                        self.messages.push(Message::system(format!(
                            "Gugugaga model set to: {}\n(Restart gugugaga to apply)",
                            item_title
                        )));
                    }
                }
                PickerMode::GugugagaModelReasoning => {
                    if let Some(model) = self.pending_gugugaga_model_for_reasoning.take() {
                        let effort = if item_id == "default" {
                            model.default_reasoning_effort.as_deref()
                        } else {
                            Some(item_id.as_str())
                        };
                        self.persist_gugugaga_model_selection(&model.id, effort)
                            .await;
                        let suffix = effort
                            .map(|e| format!(" (reasoning: {e})"))
                            .unwrap_or_default();
                        self.messages.push(Message::system(format!(
                            "Gugugaga model set to: {}{}\n(Restart gugugaga to apply)",
                            model.display_name, suffix
                        )));
                    } else {
                        self.messages.push(Message::system(
                            "No Gugugaga model selected for reasoning effort. Please run //model again.",
                        ));
                    }
                }
                PickerMode::SkillsMenu => {
                    match item_id.as_str() {
                        "list" => {
                            // Switch to skill select picker
                            self.picker_mode = PickerMode::SkillsSelect;
                            self.picker.title = "Select Skill".to_string();
                            self.picker.open_loading();
                            self.fetch_skills_list().await;
                            return; // Don't close picker
                        }
                        "manage" => {
                            // Switch to manage mode, open loading picker
                            self.picker_mode = PickerMode::SkillsManage;
                            self.picker.title = "Enable/Disable Skills".to_string();
                            self.picker.open_loading();
                            self.fetch_skills_list().await;
                            return; // Don't close picker
                        }
                        _ => {}
                    }
                }
                PickerMode::SkillsSelect => {
                    // Insert $skillname into input buffer
                    let mention = format!("${}", item_id);
                    self.input.buffer.push_str(&mention);
                    self.input.cursor += mention.len();
                }
                PickerMode::SkillsManage => {
                    // Toggle the selected skill
                    // The id is formatted as "enabled:skill_name" or "disabled:skill_name"
                    let currently_enabled = item_id.starts_with("enabled:");
                    let skill_name = item_id
                        .strip_prefix("enabled:")
                        .or_else(|| item_id.strip_prefix("disabled:"))
                        .unwrap_or(&item_id);
                    self.toggle_skill(skill_name, currently_enabled).await;
                }
                PickerMode::Permissions => {
                    self.apply_permissions_preset(&item_id, &item_title).await;
                }
                PickerMode::Personality => {
                    self.messages
                        .push(Message::system(format!("Personality: {}", item_title)));
                    self.write_config("personality", &serde_json::json!(item_id))
                        .await;
                }
                PickerMode::Collab => {
                    self.set_collaboration_mode(&item_id, false).await;
                    self.messages.push(Message::system(format!(
                        "Collaboration mode: {}",
                        if item_title.is_empty() {
                            Self::collaboration_mode_title(&item_id)
                        } else {
                            item_title
                        }
                    )));
                }
                PickerMode::Agent => {
                    self.messages.push(Message::system(format!(
                        "Switched to agent: {}",
                        item_title
                    )));
                    self.set_active_thread_id(item_id);
                }
                PickerMode::Statusline => {
                    if item_id == "__statusline:cancel" {
                        self.statusline_editor = None;
                        self.messages
                            .push(Message::system("Status line edit cancelled."));
                    } else if item_id == "__statusline:save" {
                        if let Some(editor) = &self.statusline_editor {
                            let saved_items = editor.enabled_items.clone();
                            let value = serde_json::Value::Array(
                                saved_items
                                    .iter()
                                    .map(|item| serde_json::Value::String(item.clone()))
                                    .collect(),
                            );
                            self.write_config("tui.status_line", &value).await;
                            let summary = if saved_items.is_empty() {
                                "disabled".to_string()
                            } else {
                                saved_items.join(", ")
                            };
                            self.messages
                                .push(Message::system(format!("Status line saved: {}", summary)));
                        }
                        self.statusline_editor = None;
                    } else if let Some(editor) = self.statusline_editor.as_mut() {
                        keep_picker_open = true;
                        match item_id.as_str() {
                            "__statusline:reset" => {
                                editor.enabled_items = Self::default_statusline_items();
                                editor.selected_item = editor.enabled_items.first().cloned();
                            }
                            "__statusline:up" => {
                                if let Some(selected_id) = editor.selected_item.clone() {
                                    if let Some(idx) = editor
                                        .enabled_items
                                        .iter()
                                        .position(|item| item == &selected_id)
                                    {
                                        if idx > 0 {
                                            editor.enabled_items.swap(idx, idx - 1);
                                        }
                                    }
                                }
                            }
                            "__statusline:down" => {
                                if let Some(selected_id) = editor.selected_item.clone() {
                                    if let Some(idx) = editor
                                        .enabled_items
                                        .iter()
                                        .position(|item| item == &selected_id)
                                    {
                                        if idx + 1 < editor.enabled_items.len() {
                                            editor.enabled_items.swap(idx, idx + 1);
                                        }
                                    }
                                }
                            }
                            _ => {
                                if let Some(raw_id) = item_id.strip_prefix("item:") {
                                    if let Some(normalized_id) =
                                        Self::normalize_statusline_item_id(raw_id)
                                    {
                                        if let Some(idx) = editor
                                            .enabled_items
                                            .iter()
                                            .position(|item| item == &normalized_id)
                                        {
                                            editor.enabled_items.remove(idx);
                                            if editor.selected_item.as_deref()
                                                == Some(normalized_id.as_str())
                                            {
                                                editor.selected_item =
                                                    editor.enabled_items.first().cloned();
                                            }
                                        } else {
                                            editor.enabled_items.push(normalized_id.clone());
                                            editor.selected_item = Some(normalized_id);
                                        }
                                    }
                                }
                            }
                        }
                        self.refresh_statusline_editor_picker();
                    } else {
                        self.messages.push(Message::system(
                            "Status line editor state missing. Run /statusline again.",
                        ));
                        self.picker_mode = PickerMode::None;
                        self.picker.close();
                        return;
                    }
                }
                PickerMode::None => {}
            }

            if keep_picker_open {
                self.scroll_to_bottom();
                return;
            }
        }

        self.picker.close();
        if matches!(self.picker_mode, PickerMode::Statusline) {
            self.statusline_editor = None;
        }
        self.picker_mode = PickerMode::None;
        self.scroll_to_bottom();
    }

    /// Helper: write a config value to Codex via config/value/write
    async fn write_config(&mut self, key_path: &str, value: &serde_json::Value) {
        if let Some(tx) = &self.input_tx {
            self.request_counter += 1;
            let msg = serde_json::json!({
                            "jsonrpc": "2.0",
                "method": "config/value/write",
                "id": self.request_counter,
                "params": {
                    "keyPath": key_path,
                    "value": value,
                    "mergeStrategy": "replace"
                }
            })
            .to_string();
            let _ = tx.send(msg).await;
        }
    }

    /// Set the default model via Codex's setDefaultModel RPC
    async fn set_default_model(&mut self, model_id: &str, reasoning_effort: Option<&str>) {
        if let Some(tx) = &self.input_tx {
            self.request_counter += 1;
            let mut params = serde_json::json!({
                "model": model_id
            });
            params["reasoningEffort"] = reasoning_effort
                .map(|e| serde_json::Value::String(e.to_string()))
                .unwrap_or(serde_json::Value::Null);
            let msg = serde_json::json!({
                "jsonrpc": "2.0",
                "method": "setDefaultModel",
                "id": self.request_counter,
                "params": params
            })
            .to_string();
            let _ = tx.send(msg).await;
        }
    }

    /// Send a simple approval decision (accept / acceptForSession / cancel)
    async fn respond_to_approval_decision(&mut self, approval: &PendingApproval, decision: &str) {
        if let Some(tx) = &self.input_tx {
            let response = serde_json::json!({
                        "jsonrpc": "2.0",
                "id": approval.request_id,
                "result": {
                    "decision": decision
                }
            });
            let _ = tx.send(response.to_string()).await;
        }
    }

    /// Send acceptWithExecpolicyAmendment decision (command exec only)
    async fn respond_to_approval_with_amendment(
        &mut self,
        approval: &PendingApproval,
        amendment: &[String],
    ) {
        if let Some(tx) = &self.input_tx {
            let response = serde_json::json!({
                "jsonrpc": "2.0",
                "id": approval.request_id,
                "result": {
                    "decision": {
                        "acceptWithExecpolicyAmendment": {
                            "execpolicy_amendment": amendment
                        }
                    }
                }
            });
            let _ = tx.send(response.to_string()).await;
        }
    }

    /// Send response for item/tool/requestUserInput
    async fn respond_to_user_input_request(
        &mut self,
        pending: &PendingUserInput,
        answer: Option<&str>,
    ) {
        if let Some(tx) = &self.input_tx {
            let normalized_answer = answer.map(str::trim).filter(|s| !s.is_empty());
            let mut answers = serde_json::Map::new();
            for qid in &pending.question_ids {
                let answer_value = match normalized_answer {
                    Some(text) => serde_json::json!({ "answers": [text] }),
                    None => serde_json::json!({ "answers": [] }),
                };
                answers.insert(qid.clone(), answer_value);
            }

            let response = serde_json::json!({
                "jsonrpc": "2.0",
                "id": pending.request_id,
                "result": {
                    "answers": serde_json::Value::Object(answers)
                }
            });
            let _ = tx.send(response.to_string()).await;
        }
    }

    /// Send a direct chat message to Gugugaga via the interceptor
    async fn send_gugugaga_chat(&mut self, message: &str) {
        // Show user's message in chat (with distinct magenta style for Gugugaga)
        self.messages.push(Message::user_to_gugugaga(message));
        self.scroll_to_bottom();

        // Send to interceptor which will call GugugagaAgent::chat()
        if let Some(tx) = &self.input_tx {
            let msg = serde_json::json!({
                "jsonrpc": "2.0",
                "method": "gugugaga/chat",
                "params": { "message": message }
            })
            .to_string();
            let _ = tx.send(msg).await;
        }

        // Mark as processing so status bar shows thinking
        self.gugugaga_status = Some("Thinking...".to_string());
    }

    async fn execute_gugugaga_command(&mut self, cmd: GugugagaCommand, args: String) {
        match cmd {
            GugugagaCommand::Help => {
                self.messages
                    .push(Message::system("Gugugaga commands (//):"));
                for c in GugugagaCommand::all() {
                    self.messages.push(Message::system(format!(
                        "  //{:<12} - {}",
                        c.name(),
                        c.description()
                    )));
                }
                self.messages.push(Message::system("\nCodex commands (/):"));
                self.messages.push(Message::system(
                    "  /model, /resume, /new, /status, /diff, etc.",
                ));
                self.messages
                    .push(Message::system("  Type / and press Tab for full list."));
            }
            GugugagaCommand::Clear => {
                self.messages.clear();
                self.messages.push(Message::system("Chat history cleared."));
            }
            GugugagaCommand::Stats => {
                let summary = self.render_gugugaga_stats_summary();
                self.messages.push(Message::system(&summary));
            }
            GugugagaCommand::Model => {
                let arg = args.trim();
                if arg.is_empty() {
                    // Open picker with model list (same as /model but writes to gugugaga_model)
                    self.request_gugugaga_model_list().await;
                } else {
                    // Direct set: //model <name>
                    self.handle_gugugaga_model_set(arg).await;
                }
            }
            GugugagaCommand::Notebook => {
                if let Some(notebook) = &self.notebook {
                    let nb = notebook.read().await;
                    let mut lines = Vec::new();
                    lines.push("📓 Gugugaga Notebook".to_string());
                    lines.push("─".repeat(40));

                    // Current activity
                    if let Some(activity) = &nb.current_activity {
                        lines.push(format!("\n▸ Current Activity: {}", activity));
                    } else {
                        lines.push("\n▸ Current Activity: (none)".to_string());
                    }

                    // Completed items
                    lines.push(format!("\n▸ Completed ({}):", nb.completed.len()));
                    if nb.completed.is_empty() {
                        lines.push("  (none)".to_string());
                    } else {
                        for (i, item) in nb.completed.iter().rev().take(10).enumerate() {
                            lines.push(format!(
                                "  {}. {} — {}",
                                i + 1,
                                item.what,
                                item.significance
                            ));
                        }
                        if nb.completed.len() > 10 {
                            lines.push(format!("  ... and {} more", nb.completed.len() - 10));
                        }
                    }

                    // Attention items
                    lines.push(format!("\n▸ Attention ({}):", nb.attention.len()));
                    if nb.attention.is_empty() {
                        lines.push("  (none)".to_string());
                    } else {
                        for item in &nb.attention {
                            let priority = if item.priority == crate::memory::Priority::High {
                                "⚠️"
                            } else {
                                "  "
                            };
                            lines.push(format!("{} [{}] {}", priority, item.source, item.content));
                        }
                    }

                    // Mistakes
                    lines.push(format!("\n▸ Mistakes & Lessons ({}):", nb.mistakes.len()));
                    if nb.mistakes.is_empty() {
                        lines.push("  (none)".to_string());
                    } else {
                        for (i, m) in nb.mistakes.iter().rev().take(5).enumerate() {
                            lines.push(format!("  {}. {} → {}", i + 1, m.what_happened, m.lesson));
                        }
                        if nb.mistakes.len() > 5 {
                            lines.push(format!("  ... and {} more", nb.mistakes.len() - 5));
                        }
                    }

                    self.messages.push(Message::system(lines.join("\n")));
                } else {
                    self.messages
                        .push(Message::system("No notebook available (not initialized)."));
                }
            }
        }
        self.scroll_to_bottom();
    }

    async fn persist_gugugaga_model_selection(
        &mut self,
        model: &str,
        reasoning_effort: Option<&str>,
    ) {
        let config_path = Self::codex_home_dir().join("config.toml");
        if let Err(e) =
            Self::write_gugugaga_model_selection(&config_path, model, reasoning_effort).await
        {
            self.messages.push(Message::system(format!(
                "Failed to set Gugugaga model: {}",
                e
            )));
        }
    }

    fn codex_home_dir() -> std::path::PathBuf {
        std::env::var("CODEX_HOME")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|_| {
                dirs::home_dir()
                    .unwrap_or_else(|| std::path::PathBuf::from("."))
                    .join(".codex")
            })
    }

    async fn read_gugugaga_model_reasoning_effort(config_path: &std::path::Path) -> Option<String> {
        let content = tokio::fs::read_to_string(config_path).await.ok()?;
        let doc = content.parse::<toml_edit::DocumentMut>().ok()?;
        doc.get("gugugaga_model_reasoning_effort")
            .and_then(|v| v.as_str())
            .map(ToOwned::to_owned)
    }

    /// Write gugugaga model selection to config.toml (preserving other fields)
    async fn write_gugugaga_model_selection(
        config_path: &std::path::Path,
        model: &str,
        reasoning_effort: Option<&str>,
    ) -> std::result::Result<(), String> {
        let mut doc = if config_path.exists() {
            let content = tokio::fs::read_to_string(config_path)
                .await
                .map_err(|e| format!("read config.toml: {}", e))?;
            if content.trim().is_empty() {
                toml_edit::DocumentMut::new()
            } else {
                content
                    .parse::<toml_edit::DocumentMut>()
                    .map_err(|e| format!("parse config.toml: {}", e))?
            }
        } else {
            toml_edit::DocumentMut::new()
        };

        doc["gugugaga_model"] = toml_edit::value(model.to_string());
        match reasoning_effort {
            Some(effort) => {
                doc["gugugaga_model_reasoning_effort"] = toml_edit::value(effort.to_string());
            }
            None => {
                doc.as_table_mut().remove("gugugaga_model_reasoning_effort");
            }
        }

        let output = doc.to_string();

        // Ensure parent dir exists
        if let Some(parent) = config_path.parent() {
            let _ = tokio::fs::create_dir_all(parent).await;
        }

        tokio::fs::write(config_path, output)
            .await
            .map_err(|e| format!("write config.toml: {}", e))?;

        Ok(())
    }

    async fn check_output(&mut self) {
        let messages: Vec<String> = if let Some(rx) = &mut self.output_rx {
            let mut msgs = Vec::new();
            while let Ok(msg) = rx.try_recv() {
                msgs.push(msg);
            }
            msgs
        } else {
            Vec::new()
        };

        for msg in messages {
            self.handle_output_message(&msg).await;
        }
    }

    async fn handle_output_message(&mut self, msg: &str) {
        // Debug: show raw message method for troubleshooting
        if let Ok(json) = serde_json::from_str::<serde_json::Value>(msg) {
            if let Some(method) = json.get("method").and_then(|m| m.as_str()) {
                // Show important events
                if method.contains("error") || method.contains("Error") {
                    let preview = truncate_utf8(msg, 200);
                    self.messages
                        .push(Message::system(format!("[{}] {}", method, preview)));
                }
            }
        }

        if let Ok(json) = serde_json::from_str::<serde_json::Value>(msg) {
            // Check if this is a server-initiated REQUEST (has both "id" and "method")
            // These are approval requests that need our response
            if let (Some(id), Some(method)) = (
                json.get("id").and_then(|i| i.as_u64()),
                json.get("method").and_then(|m| m.as_str()),
            ) {
                match method {
                    "item/tool/requestUserInput" => {
                        let params = json.get("params").cloned().unwrap_or_default();
                        let questions = crate::protocol::extract_user_input_questions(&params)
                            .unwrap_or_default();
                        let question_ids: Vec<String> = questions
                            .iter()
                            .filter_map(|q| {
                                q.get("id").and_then(|v| v.as_str()).map(ToOwned::to_owned)
                            })
                            .collect();

                        if question_ids.is_empty() {
                            let empty_pending = PendingUserInput {
                                request_id: id,
                                question_ids: Vec::new(),
                            };
                            self.respond_to_user_input_request(&empty_pending, None)
                                .await;
                            self.messages.push(Message::system(
                                "Received a tool user-input request without questions; sent empty response.",
                            ));
                            self.scroll_to_bottom();
                            return;
                        }

                        self.pending_user_input = Some(PendingUserInput {
                            request_id: id,
                            question_ids: question_ids.clone(),
                        });

                        let mut lines = Vec::new();
                        lines.push("Codex requested user input:".to_string());
                        for (idx, q) in questions.iter().enumerate() {
                            let header = q
                                .get("header")
                                .and_then(|v| v.as_str())
                                .unwrap_or("Question");
                            let question = q.get("question").and_then(|v| v.as_str()).unwrap_or("");
                            lines.push(format!("{}. {}: {}", idx + 1, header, question));

                            if let Some(options) = q.get("options").and_then(|v| v.as_array()) {
                                let labels: Vec<String> = options
                                    .iter()
                                    .filter_map(|opt| {
                                        opt.get("label")
                                            .and_then(|v| v.as_str())
                                            .map(ToOwned::to_owned)
                                    })
                                    .collect();
                                if !labels.is_empty() {
                                    lines.push(format!("   options: {}", labels.join(" / ")));
                                }
                            }
                        }
                        lines.push(
                            "Type your answer and press Enter. It will be used for all questions in this request. Type /cancel to send an empty response.".to_string(),
                        );
                        self.messages.push(Message::system(lines.join("\n")));
                        self.scroll_to_bottom();
                        return;
                    }
                    "item/commandExecution/requestApproval" => {
                        // Command execution approval request
                        let params = json.get("params").cloned().unwrap_or_default();
                        let command = params
                            .get("command")
                            .and_then(|c| c.as_str())
                            .map(String::from);
                        let cwd = params.get("cwd").and_then(|c| c.as_str()).map(String::from);
                        let reason = params
                            .get("reason")
                            .and_then(|r| r.as_str())
                            .map(String::from);
                        // Extract proposed execpolicy amendment (array of command prefix strings)
                        let proposed_amendment = params
                            .get("proposedExecpolicyAmendment")
                            .and_then(|v| v.as_array())
                            .map(|arr| {
                                arr.iter()
                                    .filter_map(|s| s.as_str().map(String::from))
                                    .collect::<Vec<_>>()
                            });

                        self.pending_approval = Some(PendingApproval {
                            request_id: id,
                            approval_type: ApprovalType::CommandExecution,
                            command: command.clone(),
                            cwd: cwd.clone(),
                            reason: reason.clone(),
                            changes: vec![],
                            proposed_execpolicy_amendment: proposed_amendment,
                        });
                        self.approval_scroll = 0;

                        // No chat message needed — the overlay will show everything
                        self.scroll_to_bottom();
                        return;
                    }
                    "item/fileChange/requestApproval" => {
                        // File change approval request
                        let params = json.get("params").cloned().unwrap_or_default();
                        let reason = params
                            .get("reason")
                            .and_then(|r| r.as_str())
                            .map(String::from);

                        self.pending_approval = Some(PendingApproval {
                            request_id: id,
                            approval_type: ApprovalType::FileChange,
                            command: None,
                            cwd: None,
                            reason: reason.clone(),
                            changes: vec![],
                            proposed_execpolicy_amendment: None,
                        });
                        self.approval_scroll = 0;

                        // No chat message needed — the overlay will show everything
                        self.scroll_to_bottom();
                        return;
                    }
                    _ => {} // Not an approval request, continue processing
                }
            }

            // Always capture thread_id from any response that contains result.thread.id
            // This handles initialization thread/start, thread/resume, thread/fork etc.
            if let Some(tid) = json
                .get("result")
                .and_then(|r| r.get("thread"))
                .and_then(|t| t.get("id"))
                .and_then(|i| i.as_str())
            {
                self.set_active_thread_id(tid.to_string());
            }

            // Check if this is a JSON-RPC response (has "id" and "result" or "error")
            if let Some(id) = json.get("id") {
                // This is a response to a request we made
                if let Some(req_id) = id.as_u64() {
                    if let Some(request_type) =
                        take_pending_request_type(&mut self.pending_requests, req_id)
                    {
                        self.pending_request_type = request_type;
                        self.handle_rpc_response(&json);
                        return;
                    }
                }
                // Error responses for non-tracked requests (e.g. turn/start failures)
                if let Some(error) = json.get("error") {
                    let error_msg = error
                        .get("message")
                        .and_then(|m| m.as_str())
                        .unwrap_or("Unknown error");

                    self.messages
                        .push(Message::system(format!("Error: {}", error_msg)));
                    if self.is_processing {
                        self.stop_processing();
                    }
                    return;
                }
            }

            let method = json.get("method").and_then(|m| m.as_str()).unwrap_or("");

            match method {
                "item/agentMessage/delta" => {
                    // Agent message delta - params.delta contains the text
                    if let Some(delta) = json
                        .get("params")
                        .and_then(|p| p.get("delta"))
                        .and_then(|t| t.as_str())
                    {
                        self.start_processing();
                        if let Some(last) = self.messages.last_mut() {
                            if last.role == MessageRole::Codex {
                                last.content.push_str(delta);
                                self.scroll_to_bottom();
                                return;
                            }
                        }
                        self.messages.push(Message::codex(delta));
                        self.scroll_to_bottom();
                    }
                }
                "item/agentReasoning/delta" | "item/agentReasoning/summaryDelta" => {
                    // Reasoning/thinking delta - show as thinking message
                    if let Some(delta) = json
                        .get("params")
                        .and_then(|p| p.get("delta"))
                        .and_then(|t| t.as_str())
                    {
                        self.start_processing();
                        // Append to existing thinking message or create new one
                        if let Some(last) = self.messages.last_mut() {
                            if last.role == MessageRole::Thinking {
                                last.content.push_str(delta);
                                return;
                            }
                        }
                        self.messages.push(Message::thinking(delta));
                    }
                }
                "item/agentReasoning/rawContentDelta" => {
                    // Raw reasoning content
                    if let Some(delta) = json
                        .get("params")
                        .and_then(|p| p.get("delta"))
                        .and_then(|t| t.as_str())
                    {
                        self.start_processing();
                        if let Some(last) = self.messages.last_mut() {
                            if last.role == MessageRole::Thinking {
                                last.content.push_str(delta);
                                return;
                            }
                        }
                        self.messages.push(Message::thinking(delta));
                    }
                }
                "item/reasoning/summaryTextDelta" => {
                    // Reasoning summary delta
                    if let Some(delta) = json
                        .get("params")
                        .and_then(|p| p.get("delta"))
                        .and_then(|t| t.as_str())
                    {
                        self.start_processing();
                        if let Some(last) = self.messages.last_mut() {
                            if last.role == MessageRole::Thinking {
                                last.content.push_str(delta);
                                return;
                            }
                        }
                        self.messages.push(Message::thinking(delta));
                    }
                }
                "item/reasoning/textDelta" => {
                    // Raw reasoning text delta (for open source models)
                    if let Some(delta) = json
                        .get("params")
                        .and_then(|p| p.get("delta"))
                        .and_then(|t| t.as_str())
                    {
                        self.start_processing();
                        if let Some(last) = self.messages.last_mut() {
                            if last.role == MessageRole::Thinking {
                                last.content.push_str(delta);
                                return;
                            }
                        }
                        self.messages.push(Message::thinking(delta));
                    }
                }
                "item/commandExecution/outputDelta" => {
                    // Command execution output streaming
                    let item_id = json
                        .get("params")
                        .and_then(|p| p.get("itemId").or_else(|| p.get("item_id")))
                        .and_then(|id| id.as_str())
                        .map(|s| s.to_string());
                    if let Some(delta) = json
                        .get("params")
                        .and_then(|p| p.get("delta"))
                        .and_then(|t| t.as_str())
                    {
                        if let Some(item_id) = &item_id {
                            if let Some(active) = self.active_terminals.get_mut(item_id) {
                                active.recent_output.push_str(delta);
                                if active.recent_output.len() > 2000 {
                                    active.recent_output =
                                        tail_utf8(&active.recent_output, 2000).to_string();
                                }
                            }
                        }

                        self.start_processing();
                        const MAX_OUTPUT_LINES: usize = 50;
                        const MAX_OUTPUT_CHARS: usize = 4000;

                        if let Some(last) = self.messages.last_mut() {
                            if last.role == MessageRole::CommandExec {
                                // Check if already truncated
                                if last.content.ends_with("... (output truncated)") {
                                    return;
                                }

                                last.content.push_str(delta);

                                // Truncate if too long
                                let lines: Vec<&str> = last.content.lines().collect();
                                if lines.len() > MAX_OUTPUT_LINES
                                    || last.content.len() > MAX_OUTPUT_CHARS
                                {
                                    let truncated: String = lines
                                        .iter()
                                        .take(MAX_OUTPUT_LINES)
                                        .copied()
                                        .collect::<Vec<_>>()
                                        .join("\n");
                                    let truncated = if truncated.len() > MAX_OUTPUT_CHARS {
                                        format!(
                                            "{}...",
                                            truncate_utf8(&truncated, MAX_OUTPUT_CHARS)
                                        )
                                    } else {
                                        truncated
                                    };
                                    last.content = format!("{}\n... (output truncated)", truncated);
                                }
                                return;
                            }
                        }
                        self.messages.push(Message::command_exec(delta));
                    }
                }
                "item/fileChange/outputDelta" => {
                    // File change output (diff)
                    if let Some(delta) = json
                        .get("params")
                        .and_then(|p| p.get("delta"))
                        .and_then(|t| t.as_str())
                    {
                        self.start_processing();
                        if let Some(last) = self.messages.last_mut() {
                            if last.role == MessageRole::FileChange {
                                last.content.push_str(delta);
                                return;
                            }
                        }
                        self.messages.push(Message::file_change(delta));
                    }
                }
                "turn/diff/updated" => {
                    // Turn-level unified diff — the aggregated diff of all file
                    // changes so far in this turn (same as what Codex CLI shows).
                    if let Some(diff) = json
                        .get("params")
                        .and_then(|p| p.get("diff"))
                        .and_then(|d| d.as_str())
                    {
                        if !diff.trim().is_empty() {
                            // Replace any existing turn diff message, or create new one.
                            // We look for the last FileChange message that starts with
                            // our marker so we replace instead of appending on each update.
                            let marker = "[turn diff]";
                            let new_content = format!("{}\n{}", marker, diff);
                            let replaced = self.messages.iter_mut().rev().any(|m| {
                                if m.role == MessageRole::FileChange
                                    && m.content.starts_with(marker)
                                {
                                    m.content = new_content.clone();
                                    true
                                } else {
                                    false
                                }
                            });
                            if !replaced {
                                self.messages.push(Message::file_change(&new_content));
                            }
                            self.scroll_to_bottom();
                        }
                    }
                }
                "item/started" => {
                    // Item lifecycle start - show what's happening
                    if let Some(item) = json.get("params").and_then(|p| p.get("item")) {
                        let item_type = item
                            .get("type")
                            .and_then(|t| t.as_str())
                            .unwrap_or("unknown");
                        match item_type {
                            "commandExecution" => {
                                let item_id = item.get("id").and_then(|i| i.as_str()).unwrap_or("");
                                let cmd = item
                                    .get("command")
                                    .and_then(|c| c.as_str())
                                    .unwrap_or("command");
                                if !item_id.is_empty() {
                                    self.active_terminals.insert(
                                        item_id.to_string(),
                                        ActiveTerminal {
                                            command: cmd.to_string(),
                                            recent_output: String::new(),
                                        },
                                    );
                                }
                                self.messages
                                    .push(Message::command_exec(format!("$ {}", cmd)));
                            }
                            "fileChange" => {
                                if let Some(changes) =
                                    item.get("changes").and_then(|c| c.as_array())
                                {
                                    let item_id =
                                        item.get("id").and_then(|i| i.as_str()).unwrap_or("");
                                    let mut full_diff = String::new();
                                    for change in changes {
                                        let raw_path = change
                                            .get("path")
                                            .and_then(|p| p.as_str())
                                            .unwrap_or("file");
                                        let path = make_relative_path(raw_path, &self.cwd);
                                        let kind = change.get("kind");
                                        let verb = if let Some(k) = kind.and_then(|k| k.as_str()) {
                                            match k {
                                                "add" => "Added",
                                                "delete" => "Deleted",
                                                _ => "Edited",
                                            }
                                        } else {
                                            "Edited"
                                        };
                                        full_diff.push_str(&format!("• {} {}\n", verb, path));
                                        if let Some(diff) =
                                            change.get("diff").and_then(|d| d.as_str())
                                        {
                                            if !diff.trim().is_empty() {
                                                full_diff.push_str(diff);
                                                if !diff.ends_with('\n') {
                                                    full_diff.push('\n');
                                                }
                                            }
                                        }
                                    }
                                    if !full_diff.is_empty() {
                                        // Tag with item_id so item/completed can replace it.
                                        let marker = format!("[fc:{}]", item_id);
                                        let content =
                                            format!("{}\n{}", marker, full_diff.trim_end());
                                        self.messages.push(Message::file_change(&content));
                                    }
                                }
                            }
                            "contextCompaction" => {
                                self.messages
                                    .push(Message::system("Context compaction in progress..."));
                            }
                            "webSearch" => {
                                let query =
                                    item.get("query").and_then(|q| q.as_str()).unwrap_or("...");
                                self.messages
                                    .push(Message::system(format!("🔍 Searching: {}", query)));
                            }
                            "enteredReviewMode" => {
                                let review = item
                                    .get("review")
                                    .and_then(|r| r.as_str())
                                    .unwrap_or("changes");
                                self.messages
                                    .push(Message::system(format!("📋 Reviewing: {}", review)));
                            }
                            "collabAgentToolCall" => {
                                let tool = item
                                    .get("details")
                                    .and_then(|d| d.get("tool"))
                                    .and_then(|t| t.as_str())
                                    .unwrap_or("unknown");
                                let sender = item
                                    .get("details")
                                    .and_then(|d| d.get("senderThreadId"))
                                    .and_then(|s| s.as_str())
                                    .unwrap_or("?");
                                match tool {
                                    "spawnAgent" => {
                                        let prompt = item
                                            .get("details")
                                            .and_then(|d| d.get("prompt"))
                                            .and_then(|p| p.as_str())
                                            .unwrap_or("");
                                        let preview = if prompt.len() > 80 {
                                            format!("{}...", truncate_utf8(prompt, 80))
                                        } else {
                                            prompt.to_string()
                                        };
                                        self.messages.push(Message::system(format!(
                                            "🔀 Spawning sub-agent: {}",
                                            preview
                                        )));
                                    }
                                    "sendInput" => {
                                        self.messages.push(Message::system(format!(
                                            "📨 Sending input to agent (from {})",
                                            sender
                                        )));
                                    }
                                    "wait" => {
                                        self.messages
                                            .push(Message::system("⏳ Waiting for sub-agent..."));
                                    }
                                    "closeAgent" => {
                                        self.messages.push(Message::system("🔚 Closing sub-agent"));
                                    }
                                    _ => {
                                        self.messages.push(Message::system(format!(
                                            "🤖 Collab: {} (from {})",
                                            tool, sender
                                        )));
                                    }
                                }
                            }
                            _ => {}
                        }
                        self.start_processing();
                    }
                }
                "item/completed" => {
                    // Item lifecycle complete
                    if let Some(item) = json.get("params").and_then(|p| p.get("item")) {
                        let item_type = item
                            .get("type")
                            .and_then(|t| t.as_str())
                            .unwrap_or("unknown");
                        match item_type {
                            "commandExecution" => {
                                if let Some(item_id) = item.get("id").and_then(|i| i.as_str()) {
                                    self.active_terminals.remove(item_id);
                                }
                                let exit_code = item.get("exitCode").and_then(|e| e.as_i64());
                                let duration = item.get("durationMs").and_then(|d| d.as_i64());

                                let dur_str =
                                    duration.map(|d| format!(" • {}ms", d)).unwrap_or_default();
                                let status_line = if exit_code.unwrap_or(0) == 0 {
                                    format!("\n\u{2713}{}", dur_str)
                                } else {
                                    format!("\n\u{2717} ({}){}", exit_code.unwrap_or(-1), dur_str)
                                };

                                // Update the last in-progress CommandExec message
                                let updated = self
                                    .messages
                                    .iter_mut()
                                    .rev()
                                    .find(|m| {
                                        m.role == MessageRole::CommandExec
                                            && !m.content.contains('\u{2713}')
                                            && !m.content.contains('\u{2717}')
                                    })
                                    .map(|m| {
                                        m.content.push_str(&status_line);
                                        true
                                    })
                                    .unwrap_or(false);

                                if !updated {
                                    // Fallback: separate system message
                                    let code_str = exit_code
                                        .map(|c| format!(" (exit {})", c))
                                        .unwrap_or_default();
                                    let dur = duration
                                        .map(|d| format!(" in {}ms", d))
                                        .unwrap_or_default();
                                    self.messages.push(Message::system(format!(
                                        "Command completed{}{}",
                                        code_str, dur
                                    )));
                                }
                            }
                            "fileChange" => {
                                let item_id = item.get("id").and_then(|i| i.as_str()).unwrap_or("");
                                let status = item
                                    .get("status")
                                    .and_then(|s| s.as_str())
                                    .unwrap_or("completed");
                                // Build the final diff from completed changes.
                                if let Some(changes) =
                                    item.get("changes").and_then(|c| c.as_array())
                                {
                                    let mut full_diff = String::new();
                                    for change in changes {
                                        let raw_path = change
                                            .get("path")
                                            .and_then(|p| p.as_str())
                                            .unwrap_or("file");
                                        let path = make_relative_path(raw_path, &self.cwd);
                                        let kind = change.get("kind");
                                        let verb = if let Some(k) = kind.and_then(|k| k.as_str()) {
                                            match k {
                                                "add" => "Added",
                                                "delete" => "Deleted",
                                                _ => "Edited",
                                            }
                                        } else {
                                            "Edited"
                                        };
                                        full_diff.push_str(&format!("• {} {}\n", verb, path));
                                        if let Some(diff) =
                                            change.get("diff").and_then(|d| d.as_str())
                                        {
                                            if !diff.trim().is_empty() {
                                                full_diff.push_str(diff);
                                                if !diff.ends_with('\n') {
                                                    full_diff.push('\n');
                                                }
                                            }
                                        }
                                    }
                                    if !full_diff.is_empty() {
                                        let marker = format!("[fc:{}]", item_id);
                                        let new_content =
                                            format!("{}\n{}", marker, full_diff.trim_end());
                                        // Replace the in-progress message if it exists.
                                        let replaced = self.messages.iter_mut().rev().any(|m| {
                                            if m.role == MessageRole::FileChange
                                                && m.content.starts_with(&marker)
                                            {
                                                m.content = new_content.clone();
                                                true
                                            } else {
                                                false
                                            }
                                        });
                                        if !replaced {
                                            self.messages.push(Message::file_change(&new_content));
                                        }
                                    }
                                }
                                let icon = if status == "completed" { "✓" } else { "✘" };
                                self.messages.push(Message::system(format!(
                                    "{} File change {}",
                                    icon, status
                                )));
                            }
                            "exitedReviewMode" => {
                                if let Some(review) = item.get("review").and_then(|r| r.as_str()) {
                                    self.messages.push(Message::codex(review));
                                }
                            }
                            "contextCompaction" => {
                                self.messages.push(Message::system("Context compacted."));
                            }
                            "collabAgentToolCall" => {
                                let tool = item
                                    .get("details")
                                    .and_then(|d| d.get("tool"))
                                    .and_then(|t| t.as_str())
                                    .unwrap_or("unknown");
                                let status = item
                                    .get("details")
                                    .and_then(|d| d.get("status"))
                                    .and_then(|s| s.as_str())
                                    .unwrap_or("completed");
                                // Show agent states if available
                                if let Some(agents) = item
                                    .get("details")
                                    .and_then(|d| d.get("agentsStates"))
                                    .and_then(|a| a.as_object())
                                {
                                    for (agent_id, state) in agents {
                                        let agent_status = state
                                            .get("status")
                                            .and_then(|s| s.as_str())
                                            .unwrap_or("unknown");
                                        let msg = state.get("message").and_then(|m| m.as_str());
                                        let icon = match agent_status {
                                            "completed" => "✅",
                                            "running" => "🔄",
                                            "errored" => "❌",
                                            "shutdown" => "⏹️",
                                            _ => "🤖",
                                        };
                                        let short_id = if agent_id.len() > 8 {
                                            truncate_utf8(agent_id, 8)
                                        } else {
                                            agent_id
                                        };
                                        if let Some(msg) = msg {
                                            let preview = if msg.len() > 100 {
                                                format!("{}...", truncate_utf8(msg, 100))
                                            } else {
                                                msg.to_string()
                                            };
                                            self.messages.push(Message::system(format!(
                                                "{} Agent {}.. {}: {}",
                                                icon, short_id, agent_status, preview
                                            )));
                                        } else {
                                            self.messages.push(Message::system(format!(
                                                "{} Agent {}.. {}",
                                                icon, short_id, agent_status
                                            )));
                                        }
                                    }
                                } else {
                                    self.messages.push(Message::system(format!(
                                        "🤖 Collab {} {}",
                                        tool, status
                                    )));
                                }
                            }
                            _ => {}
                        }
                    }
                }
                "turn/plan/updated" => {
                    // Plan update notification
                    if let Some(explanation) = json
                        .get("params")
                        .and_then(|p| p.get("explanation"))
                        .and_then(|e| e.as_str())
                    {
                        self.messages
                            .push(Message::system(format!("📋 Plan: {}", explanation)));
                    }
                    if let Some(plan) = json
                        .get("params")
                        .and_then(|p| p.get("plan"))
                        .and_then(|p| p.as_array())
                    {
                        for (i, step) in plan.iter().enumerate() {
                            let step_text =
                                step.get("step").and_then(|s| s.as_str()).unwrap_or("step");
                            let status = step
                                .get("status")
                                .and_then(|s| s.as_str())
                                .unwrap_or("pending");
                            let icon = match status {
                                "completed" => "✓",
                                "inProgress" => "→",
                                _ => "○",
                            };
                            self.messages.push(Message::system(format!(
                                "  {} {}. {}",
                                icon,
                                i + 1,
                                step_text
                            )));
                        }
                    }
                }
                // "turn/diff/updated" handled above (near item/fileChange/outputDelta)
                "turn/started" => {
                    self.start_processing();
                    self.current_turn_violations = 0;
                    // Capture turn_id for interrupt support
                    if let Some(turn_id) = json
                        .get("params")
                        .and_then(|p| p.get("turn"))
                        .and_then(|t| t.get("id"))
                        .and_then(|i| i.as_str())
                    {
                        self.current_turn_id = Some(turn_id.to_string());
                    }
                }
                "turn/completed" => {
                    let turn_status = json
                        .get("params")
                        .and_then(|p| p.get("turn"))
                        .and_then(|t| t.get("status"))
                        .and_then(|s| s.as_str())
                        .unwrap_or("completed");

                    match turn_status {
                        "interrupted" => {
                            self.messages.push(Message::system(
                                "Turn interrupted — tell the model what to do differently.",
                            ));
                        }
                        "failed" => {
                            let error_msg = json
                                .get("params")
                                .and_then(|p| p.get("turn"))
                                .and_then(|t| t.get("error"))
                                .and_then(|e| e.get("message"))
                                .and_then(|m| m.as_str())
                                .unwrap_or("Unknown error");
                            self.messages
                                .push(Message::system(format!("Turn failed: {}", error_msg)));
                        }
                        _ => {} // "completed" — normal
                    }

                    self.stop_processing();
                    self.current_turn_id = None;
                    self.scroll_to_bottom();
                }
                "thread/tokenUsage/updated" => {
                    if let Some(params) = json.get("params") {
                        let token_usage = params
                            .get("tokenUsage")
                            .or_else(|| params.get("token_usage"));
                        let total = token_usage
                            .and_then(|usage| usage.get("total"))
                            .cloned()
                            .unwrap_or_default();
                        let turn_id = params
                            .get("turnId")
                            .or_else(|| params.get("turn_id"))
                            .and_then(|v| v.as_str())
                            .map(|s| s.to_string());
                        let model_context_window = token_usage
                            .and_then(|usage| {
                                usage
                                    .get("modelContextWindow")
                                    .or_else(|| usage.get("model_context_window"))
                            })
                            .and_then(|v| v.as_i64());

                        self.token_usage_snapshot = Some(TokenUsageSnapshot {
                            total_tokens: total
                                .get("totalTokens")
                                .or_else(|| total.get("total_tokens"))
                                .and_then(|v| v.as_i64())
                                .unwrap_or(0),
                            input_tokens: total
                                .get("inputTokens")
                                .or_else(|| total.get("input_tokens"))
                                .and_then(|v| v.as_i64())
                                .unwrap_or(0),
                            cached_input_tokens: total
                                .get("cachedInputTokens")
                                .or_else(|| total.get("cached_input_tokens"))
                                .and_then(|v| v.as_i64())
                                .unwrap_or(0),
                            output_tokens: total
                                .get("outputTokens")
                                .or_else(|| total.get("output_tokens"))
                                .and_then(|v| v.as_i64())
                                .unwrap_or(0),
                            reasoning_output_tokens: total
                                .get("reasoningOutputTokens")
                                .or_else(|| total.get("reasoning_output_tokens"))
                                .and_then(|v| v.as_i64())
                                .unwrap_or(0),
                            model_context_window,
                            turn_id,
                        });
                    }
                }
                "account/rateLimits/updated" => {
                    if let Some(params) = json.get("params") {
                        let snapshot = params
                            .get("rateLimits")
                            .or_else(|| params.get("rate_limits"))
                            .and_then(Self::parse_rate_limit_snapshot);
                        if snapshot.is_some() {
                            self.rate_limit_snapshot = snapshot;
                            if self.account_auth_mode.is_none() {
                                self.account_auth_mode = Some("chatgpt".to_string());
                            }
                        }
                    }
                }
                "account/updated" => {
                    if let Some(params) = json.get("params") {
                        self.account_auth_mode = params
                            .get("authMode")
                            .or_else(|| params.get("auth_mode"))
                            .and_then(|v| v.as_str())
                            .map(ToOwned::to_owned);
                    }
                }
                "thread/started" => {
                    // Extract thread ID from notification
                    if let Some(thread) = json.get("params").and_then(|p| p.get("thread")) {
                        if let Some(id) = thread.get("id").and_then(|i| i.as_str()) {
                            self.set_active_thread_id(id.to_string());
                            // Session ready — no startup message needed
                        }
                    }
                }
                "gugugaga/correction" => {
                    if let Some(text) = json
                        .get("params")
                        .and_then(|p| p.get("message"))
                        .and_then(|t| t.as_str())
                    {
                        self.messages.push(Message::correction(text));
                        self.corrections_made += 1;
                        self.scroll_to_bottom();
                    }
                }
                "gugugaga/violation" => {
                    self.violations_detected += 1;
                    self.current_turn_violations += 1;
                    // Show violation to user
                    if let Some(text) = json
                        .get("params")
                        .and_then(|p| p.get("message"))
                        .and_then(|t| t.as_str())
                    {
                        self.messages
                            .push(Message::system(format!("⚠️ violations: {}", text)));
                        self.scroll_to_bottom();
                    }
                }
                "gugugaga/thinking" => {
                    let params = json.get("params");
                    let status = params
                        .and_then(|p| p.get("status"))
                        .and_then(|s| s.as_str())
                        .unwrap_or("");
                    let message = params
                        .and_then(|p| p.get("message"))
                        .and_then(|m| m.as_str())
                        .unwrap_or("");

                    match status {
                        "thinking" => {
                            // Update status bar only (like Codex's reasoning → status indicator)
                            self.gugugaga_status = Some(message.to_string());
                        }
                        "thought" => {
                            // Show thinking content in chat (dimmed, like reasoning output)
                            let duration_ms = params
                                .and_then(|p| p.get("duration_ms"))
                                .and_then(|d| d.as_u64())
                                .unwrap_or(0);
                            let duration_str = if duration_ms >= 1000 {
                                format!("{:.1}s", duration_ms as f64 / 1000.0)
                            } else {
                                format!("{}ms", duration_ms)
                            };
                            // Show first meaningful line as status, full content in chat
                            let first_line = message.lines().next().unwrap_or(message);
                            let display = if first_line.len() > 80 {
                                format!("{}...", &first_line[..77])
                            } else {
                                first_line.to_string()
                            };
                            self.gugugaga_status =
                                Some(format!("Thought ({}): {}", duration_str, display));
                        }
                        _ => {
                            if !message.is_empty() {
                                self.gugugaga_status = Some(message.to_string());
                            }
                        }
                    }
                }
                "gugugaga/toolCall" => {
                    let params = json.get("params");
                    let status = params
                        .and_then(|p| p.get("status"))
                        .and_then(|s| s.as_str())
                        .unwrap_or("");
                    let tool = params
                        .and_then(|p| p.get("tool"))
                        .and_then(|t| t.as_str())
                        .unwrap_or("?");
                    let call_id = params
                        .and_then(|p| p.get("call_id"))
                        .and_then(|c| c.as_str())
                        .unwrap_or("");
                    let args = params
                        .and_then(|p| p.get("args"))
                        .and_then(|a| a.as_str())
                        .unwrap_or("");
                    let normalized_args = params
                        .and_then(|p| p.get("normalized_args"))
                        .and_then(|a| a.as_str());
                    let normalized_error = params
                        .and_then(|p| p.get("normalized_error"))
                        .and_then(|a| a.as_str());
                    let raw_item = params.and_then(|p| p.get("raw_item"));
                    let notebook_diff = params.and_then(|p| p.get("notebook_diff"));
                    let duplicate = params
                        .and_then(|p| p.get("duplicate"))
                        .and_then(|b| b.as_bool())
                        .unwrap_or(false);
                    let guarded = params
                        .and_then(|p| p.get("guarded"))
                        .and_then(|b| b.as_bool())
                        .unwrap_or(false);
                    let debug_tool_trace = supervisor_tool_trace_debug_enabled();
                    let internal_tool = is_internal_supervision_tool(tool);
                    let args_preview = tool_args_preview(args, 120);
                    let args_display = format_tool_args_for_display(args);

                    match status {
                        "started" => {
                            // Show tool call start in status bar
                            if internal_tool && !debug_tool_trace {
                                self.gugugaga_status = Some("Supervising...".to_string());
                            } else if call_id.is_empty() {
                                self.gugugaga_status =
                                    Some(format!("$ {}({})", tool, args_preview));
                            } else {
                                self.gugugaga_status =
                                    Some(format!("$ {}#{}({})", tool, call_id, args_preview));
                            }
                        }
                        "completed" => {
                            let output = params
                                .and_then(|p| p.get("output"))
                                .and_then(|o| o.as_str())
                                .unwrap_or("");
                            let duration_ms = params
                                .and_then(|p| p.get("duration_ms"))
                                .and_then(|d| d.as_u64())
                                .unwrap_or(0);
                            let success = params
                                .and_then(|p| p.get("success"))
                                .and_then(|s| s.as_bool())
                                .unwrap_or(true);

                            let duration_str = if duration_ms >= 1000 {
                                format!("{:.1}s", duration_ms as f64 / 1000.0)
                            } else {
                                format!("{}ms", duration_ms)
                            };

                            let icon = if success { "✓" } else { "✗" };

                            let mut content = if internal_tool && !debug_tool_trace {
                                format!("$ {}\n", tool)
                            } else {
                                format!("$ {}({})\n", tool, args_display)
                            };
                            let mut notebook_diff_rendered = false;
                            if internal_tool && !debug_tool_trace {
                                if let Some(diff) = notebook_diff {
                                    let diff_lines = format_notebook_diff_for_display(diff);
                                    if !diff_lines.is_empty() {
                                        content.push_str(&diff_lines.join("\n"));
                                        content.push('\n');
                                        notebook_diff_rendered = true;
                                    }
                                }
                            }

                            if debug_tool_trace {
                                if !call_id.is_empty() {
                                    content.push_str(&format!("call_id: {}\n", call_id));
                                }
                                if let Some(normalized) = normalized_args {
                                    let pretty = format_tool_args_for_display(normalized);
                                    if !pretty.trim().is_empty()
                                        && pretty.trim() != args_display.trim()
                                    {
                                        content.push_str("normalized args:\n");
                                        content.push_str(&pretty);
                                        content.push('\n');
                                    }
                                }
                                if let Some(err) = normalized_error {
                                    if !err.trim().is_empty() {
                                        content.push_str(&format!("argument error: {}\n", err));
                                    }
                                }
                                if let Some(item) = raw_item {
                                    let payload = format_json_value_for_display(item);
                                    let capped_payload = truncate_utf8(&payload, 4_000);
                                    content.push_str("raw payload:\n");
                                    content.push_str(capped_payload);
                                    if capped_payload.len() < payload.len() {
                                        content.push_str("\n... (payload truncated)");
                                    }
                                    content.push('\n');
                                }
                            }
                            if !(output.is_empty()
                                || (internal_tool && !debug_tool_trace && notebook_diff_rendered))
                            {
                                let (max_bytes, max_lines) = if internal_tool && !debug_tool_trace {
                                    (2_000usize, 8usize)
                                } else {
                                    (10_000usize, 40usize)
                                };
                                let capped_output = truncate_utf8(output, max_bytes);
                                let output_lines: Vec<&str> =
                                    capped_output.lines().take(max_lines).collect();
                                content.push_str(&output_lines.join("\n"));
                                let total_lines = capped_output.lines().count();
                                let bytes_truncated = capped_output.len() < output.len();
                                if total_lines > max_lines {
                                    content.push_str(&format!(
                                        "\n... ({} more lines)",
                                        total_lines - max_lines
                                    ));
                                }
                                if bytes_truncated {
                                    content.push_str("\n... (output truncated by size)");
                                }
                                content.push('\n');
                            }
                            if guarded {
                                if duplicate {
                                    content.push_str("guard: duplicate call skipped\n");
                                } else {
                                    content.push_str("guard: tool-call limit reached\n");
                                }
                            }
                            content.push_str(&format!("{} {}", icon, duration_str));

                            self.messages.push(Message::gugugaga(&content));
                            self.scroll_to_bottom();

                            // Update status bar
                            let mut status_line = if internal_tool && !debug_tool_trace {
                                format!("{} {} {}", icon, tool, duration_str)
                            } else {
                                format!("{} {}({}) {}", icon, tool, args_preview, duration_str)
                            };
                            if guarded && duplicate {
                                status_line.push_str(" duplicate-skip");
                            } else if guarded {
                                status_line.push_str(" guarded");
                            }
                            self.gugugaga_status = Some(status_line);
                        }
                        _ => {}
                    }
                }
                "gugugaga/sessionRestore" => {
                    // Full ordered conversation history for session restore.
                    // Store it; the thread/resume handler will use it instead of
                    // display_turns() to show messages in correct interleaved order.
                    if let Some(turns) = json
                        .get("params")
                        .and_then(|p| p.get("turns"))
                        .and_then(|t| t.as_array())
                    {
                        self.pending_session_restore = Some(turns.clone());
                    }
                }
                "gugugaga/chatReply" => {
                    // Direct chat response from Gugugaga
                    self.gugugaga_status = None;
                    if let Some(msg) = json
                        .get("params")
                        .and_then(|p| p.get("message"))
                        .and_then(|m| m.as_str())
                    {
                        if !msg.is_empty() {
                            self.messages.push(Message::gugugaga(msg));
                            self.scroll_to_bottom();
                        }
                    }
                }
                "gugugaga/check" => {
                    // Clear gugugaga status (thinking is done)
                    self.gugugaga_status = None;

                    let thinking = json
                        .get("params")
                        .and_then(|p| p.get("thinking"))
                        .and_then(|t| t.as_str())
                        .unwrap_or("");
                    let status = json
                        .get("params")
                        .and_then(|p| p.get("status"))
                        .and_then(|s| s.as_str())
                        .unwrap_or("ok");

                    let msg = json
                        .get("params")
                        .and_then(|p| p.get("message"))
                        .and_then(|m| m.as_str())
                        .unwrap_or("");

                    if !thinking.trim().is_empty() {
                        self.messages.push(Message::thinking(thinking.trim()));
                    }

                    if !msg.is_empty() {
                        match status {
                            "ok" => {
                                // Avoid double-prefixing if message already has emoji
                                if msg.starts_with("🛡️") {
                                    self.messages.push(Message::gugugaga(msg));
                                } else {
                                    self.messages.push(Message::gugugaga(format!("🛡️ {}", msg)));
                                }
                            }
                            "violation" => {
                                self.violations_detected += 1;
                                self.current_turn_violations += 1;
                                self.messages.push(Message::gugugaga(msg));
                            }
                            "error" => self.messages.push(Message::gugugaga(msg)),
                            _ => self.messages.push(Message::gugugaga(msg)),
                        }
                        self.scroll_to_bottom();
                    }
                }
                "gugugaga/status" => {
                    // Silently acknowledge gugugaga status — no message shown
                }
                "gugugaga/auto_reply" => {
                    self.auto_replies += 1;
                }
                "error" => {
                    // Error notification
                    if let Some(msg) = json
                        .get("params")
                        .and_then(|p| p.get("message"))
                        .and_then(|m| m.as_str())
                    {
                        self.messages
                            .push(Message::system(format!("Error: {}", msg)));
                        self.stop_processing();
                    }
                }
                _ => {
                    // Log unknown methods in debug mode
                    // For now, just ignore
                }
            }
        }
    }

    fn handle_rpc_response(&mut self, json: &serde_json::Value) {
        let request_type =
            std::mem::replace(&mut self.pending_request_type, PendingRequestType::None);

        match request_type {
            PendingRequestType::ThreadList => {
                // Parse thread list response for picker
                if let Some(result) = json.get("result") {
                    if let Some(data) = result.get("data").and_then(|d| d.as_array()) {
                        let current_cwd = &self.cwd;
                        let items: Vec<PickerItem> = data
                            .iter()
                            .filter_map(|thread| {
                                let id = thread.get("id")?.as_str()?.to_string();

                                // Extract rollout file path for direct resume
                                let rollout_path = thread
                                    .get("path")
                                    .and_then(|p| p.as_str())
                                    .map(|s| s.to_string());

                                // Filter by cwd - only show sessions from current directory
                                let thread_cwd =
                                    thread.get("cwd").and_then(|c| c.as_str()).unwrap_or("");
                                if !thread_cwd.is_empty() && thread_cwd != current_cwd {
                                    return None;
                                }

                                // Use preview as title (first user message), fallback to id
                                let preview =
                                    thread.get("preview").and_then(|p| p.as_str()).unwrap_or("");
                                let title = if preview.is_empty() {
                                    format!("Session {}", &id[..8.min(id.len())])
                                } else {
                                    // Truncate long previews (char-safe for UTF-8)
                                    let max_chars = 40;
                                    let truncated: String =
                                        preview.chars().take(max_chars).collect();
                                    if truncated.len() < preview.len() {
                                        format!("{}...", truncated)
                                    } else {
                                        preview.to_string()
                                    }
                                };

                                // createdAt is Unix timestamp (i64), convert to date
                                let created_at = thread
                                    .get("createdAt")
                                    .and_then(|c| c.as_i64())
                                    .unwrap_or(0);
                                let date = if created_at > 0 {
                                    use std::time::{Duration, UNIX_EPOCH};
                                    let datetime =
                                        UNIX_EPOCH + Duration::from_secs(created_at as u64);
                                    let secs_ago = std::time::SystemTime::now()
                                        .duration_since(datetime)
                                        .map(|d| d.as_secs())
                                        .unwrap_or(0);
                                    if secs_ago < 60 {
                                        "just now".to_string()
                                    } else if secs_ago < 3600 {
                                        format!("{}min ago", secs_ago / 60)
                                    } else if secs_ago < 86400 {
                                        format!("{}hours ago", secs_ago / 3600)
                                    } else {
                                        format!("{}days ago", secs_ago / 86400)
                                    }
                                } else {
                                    "Unknown".to_string()
                                };

                                Some(PickerItem {
                                    id,
                                    title,
                                    subtitle: date,
                                    metadata: rollout_path,
                                })
                            })
                            .collect();

                        if items.is_empty() {
                            self.picker.close();
                            self.picker_mode = PickerMode::None;
                            self.messages.push(Message::system(
                                "No saved sessions found for this directory.",
                            ));
                        } else {
                            self.picker.set_items(items);
                        }
                    }
                } else if let Some(error) = json.get("error") {
                    self.picker.close();
                    self.picker_mode = PickerMode::None;
                    let error_msg = error
                        .get("message")
                        .and_then(|m| m.as_str())
                        .unwrap_or("Unknown error");
                    self.messages.push(Message::system(format!(
                        "Failed to load sessions: {}",
                        error_msg
                    )));
                }
            }
            PendingRequestType::RolloutPathLookup(thread_id) => {
                if let Some(result) = json.get("result") {
                    let path = result
                        .get("data")
                        .and_then(|d| d.as_array())
                        .and_then(|threads| {
                            threads.iter().find_map(|thread| {
                                let id = thread.get("id").and_then(|v| v.as_str())?;
                                if id != thread_id {
                                    return None;
                                }
                                thread.get("path").and_then(|p| p.as_str())
                            })
                        });

                    match path {
                        Some(p) if !p.is_empty() => self
                            .messages
                            .push(Message::system(format!("Current rollout path: {}", p))),
                        _ => self.messages.push(Message::system(
                            "Rollout path is not available for the current thread.",
                        )),
                    }
                } else {
                    self.handle_rpc_error(json, "Failed to fetch rollout path");
                }
            }
            PendingRequestType::ThreadResume(_thread_id) => {
                // Handle thread/resume response
                if let Some(result) = json.get("result") {
                    // Extract thread ID from response and update our state
                    if let Some(thread) = result.get("thread") {
                        if let Some(id) = thread.get("id").and_then(|i| i.as_str()) {
                            self.set_active_thread_id(id.to_string());
                            // Switching to another thread should replace the current transcript
                            // instead of appending onto it.
                            self.messages.clear();
                            let mut rendered_history = false;

                            // If we have a pending session restore (from gugugaga/sessionRestore),
                            // use it to display ALL turns (User, Codex, Gugugaga) in correct
                            // chronological order. Otherwise fall back to display_turns().
                            if let Some(restore_turns) = self.pending_session_restore.take() {
                                if !restore_turns.is_empty() {
                                    self.display_session_restore(&restore_turns);
                                    rendered_history = true;
                                }
                            }

                            if !rendered_history {
                                if let Some(turns) = thread.get("turns").and_then(|t| t.as_array())
                                {
                                    if !turns.is_empty() {
                                        self.display_turns(turns);
                                        rendered_history = true;
                                    }
                                }
                            }

                            if !rendered_history {
                                self.messages.push(Message::system("Session resumed."));
                            }
                            self.scroll_to_bottom();
                        }
                    }
                } else if let Some(error) = json.get("error") {
                    self.pending_session_restore = None;
                    let error_msg = error
                        .get("message")
                        .and_then(|m| m.as_str())
                        .unwrap_or("Unknown error");
                    self.messages.push(Message::system(format!(
                        "Failed to resume session: {}",
                        error_msg
                    )));
                }
            }
            PendingRequestType::ThreadRead(_thread_id) => {
                // Handle thread/read response - display history
                if let Some(result) = json.get("result") {
                    if let Some(thread) = result.get("thread") {
                        if let Some(turns) = thread.get("turns").and_then(|t| t.as_array()) {
                            self.display_turns(turns);
                            self.scroll_to_bottom();
                        }
                    }
                }
            }
            PendingRequestType::ThreadCompactStart => {
                if json.get("result").is_some() {
                    self.messages
                        .push(Message::system("Context compaction started."));
                } else {
                    self.handle_rpc_error(json, "Failed to start context compaction");
                }
            }
            PendingRequestType::ThreadBackgroundTerminalsClean => {
                if json.get("result").is_some() {
                    self.active_terminals.clear();
                    self.messages
                        .push(Message::system("Background terminals cleaned."));
                } else {
                    self.handle_rpc_error(json, "Failed to clean background terminals");
                }
            }
            PendingRequestType::ModelList => {
                if let Some(result) = json.get("result") {
                    let parsed_models = Self::parse_model_entries(result);
                    self.available_models = parsed_models.clone();
                    let items: Vec<PickerItem> = parsed_models
                        .iter()
                        .map(|model| {
                            let title = if model.is_default {
                                format!("{} (default)", model.display_name)
                            } else {
                                model.display_name.clone()
                            };
                            PickerItem {
                                id: model.id.clone(),
                                title,
                                subtitle: Self::model_subtitle(model),
                                metadata: None,
                            }
                        })
                        .collect();

                    if items.is_empty() {
                        self.picker.close();
                        self.picker_mode = PickerMode::None;
                        self.messages.push(Message::system("No models available."));
                    } else {
                        self.picker.set_items(items);
                    }
                } else {
                    self.handle_rpc_error(json, "Failed to load models");
                }
            }
            PendingRequestType::GugugagaModelList => {
                // Same parsing as ModelList, but for gugugaga model picker
                if let Some(result) = json.get("result") {
                    let parsed_models = Self::parse_model_entries(result);
                    self.available_models = parsed_models.clone();
                    let items: Vec<PickerItem> = parsed_models
                        .iter()
                        .map(|model| {
                            let title = if model.is_default {
                                format!("{} (default)", model.display_name)
                            } else {
                                model.display_name.clone()
                            };
                            PickerItem {
                                id: model.id.clone(),
                                title,
                                subtitle: Self::model_subtitle(model),
                                metadata: None,
                            }
                        })
                        .collect();

                    if items.is_empty() {
                        self.picker.close();
                        self.picker_mode = PickerMode::None;
                        self.messages.push(Message::system("No models available."));
                    } else {
                        self.picker.set_items(items);
                    }
                } else {
                    self.handle_rpc_error(json, "Failed to load models");
                }
            }
            PendingRequestType::SkillsList => {
                if let Some(result) = json.get("result") {
                    // Collect all skills from all cwds: (name, desc, enabled, path)
                    let mut all_skills: Vec<(String, String, bool, String)> = Vec::new();
                    let mut errors: Vec<String> = Vec::new();

                    if let Some(data) = result.get("data").and_then(|d| d.as_array()) {
                        for entry in data {
                            if let Some(skills) = entry.get("skills").and_then(|s| s.as_array()) {
                                for skill in skills {
                                    let name = skill
                                        .get("name")
                                        .and_then(|n| n.as_str())
                                        .unwrap_or("unnamed")
                                        .to_string();
                                    let desc = skill
                                        .get("description")
                                        .and_then(|d| d.as_str())
                                        .unwrap_or("")
                                        .to_string();
                                    let enabled = skill
                                        .get("enabled")
                                        .and_then(|e| e.as_bool())
                                        .unwrap_or(false);
                                    let path = skill
                                        .get("path")
                                        .and_then(|p| p.as_str())
                                        .unwrap_or("")
                                        .to_string();
                                    all_skills.push((name, desc, enabled, path));
                                }
                            }
                            if let Some(errs) = entry.get("errors").and_then(|e| e.as_array()) {
                                for err in errs {
                                    // SkillErrorInfo has path and message fields
                                    let path =
                                        err.get("path").and_then(|p| p.as_str()).unwrap_or("");
                                    let msg = err
                                        .get("message")
                                        .and_then(|m| m.as_str())
                                        .unwrap_or_else(|| err.as_str().unwrap_or("unknown error"));
                                    if path.is_empty() {
                                        errors.push(msg.to_string());
                                    } else {
                                        errors.push(format!("{}: {}", path, msg));
                                    }
                                }
                            }
                        }
                    }

                    match self.picker_mode {
                        PickerMode::SkillsSelect => {
                            // Show enabled skills in picker for selection/mention
                            let enabled_skills: Vec<&(String, String, bool, String)> = all_skills
                                .iter()
                                .filter(|(_, _, enabled, _)| *enabled)
                                .collect();

                            if enabled_skills.is_empty() {
                                self.picker.close();
                                self.picker_mode = PickerMode::None;
                                self.messages
                                    .push(Message::system("No enabled skills found."));
                            } else {
                                let items: Vec<PickerItem> = enabled_skills
                                    .iter()
                                    .map(|(name, desc, _, _)| PickerItem {
                                        id: name.clone(),
                                        title: name.clone(),
                                        subtitle: desc.clone(),
                                        metadata: None,
                                    })
                                    .collect();
                                self.picker.set_items(items);
                            }
                        }
                        PickerMode::SkillsManage => {
                            // Show all skills in picker for toggling
                            if all_skills.is_empty() {
                                self.picker.close();
                                self.picker_mode = PickerMode::None;
                                self.messages
                                    .push(Message::system("No skills found to manage."));
                            } else {
                                let items: Vec<PickerItem> = all_skills
                                    .iter()
                                    .map(|(name, desc, enabled, path)| {
                                        let status = if *enabled { "✓" } else { "✗" };
                                        let prefix = if *enabled { "enabled" } else { "disabled" };
                                        PickerItem {
                                            // Use path as ID since skills/config/write requires path
                                            id: format!("{}:{}", prefix, path),
                                            title: format!("{} {}", status, name),
                                            subtitle: desc.clone(),
                                            metadata: None,
                                        }
                                    })
                                    .collect();
                                self.picker.set_items(items);
                            }
                        }
                        _ => {
                            // Fallback: display as text
                            self.picker.close();
                            self.picker_mode = PickerMode::None;

                            if all_skills.is_empty() && errors.is_empty() {
                                self.messages.push(Message::system(
                                    "No skills found. Add skills via AGENTS.md or ~/.codex/skills/",
                                ));
                            } else {
                                let mut lines = Vec::new();
                                for (name, desc, enabled, _path) in &all_skills {
                                    let status = if *enabled { "✓" } else { "✗" };
                                    lines.push(format!("  {} {} — {}", status, name, desc));
                                }
                                for err in &errors {
                                    lines.push(format!("  ⚠ {}", err));
                                }
                                self.messages.push(Message::system(format!(
                                    "Skills:\n{}",
                                    lines.join("\n")
                                )));
                            }
                        }
                    }
                } else {
                    self.picker.close();
                    self.picker_mode = PickerMode::None;
                    self.handle_rpc_error(json, "Failed to load skills");
                }
            }
            PendingRequestType::CollabModeList => {
                if let Some(result) = json.get("result") {
                    if let Some(modes) = result.get("data").and_then(|m| m.as_array()) {
                        let options: Vec<String> = modes
                            .iter()
                            .filter_map(|m| m.get("name").and_then(|n| n.as_str()))
                            .map(Self::normalize_collaboration_mode_key)
                            .collect();
                        self.set_collaboration_mode_options(options);

                        let items: Vec<PickerItem> = modes
                            .iter()
                            .filter_map(|m| {
                                let name = m.get("name").and_then(|n| n.as_str())?.to_string();
                                let mode = m.get("mode").and_then(|d| d.as_str()).unwrap_or("");
                                let model = m.get("model").and_then(|d| d.as_str()).unwrap_or("");
                                let subtitle = if !model.is_empty() {
                                    format!("{} ({})", mode, model)
                                } else {
                                    mode.to_string()
                                };
                                Some(PickerItem {
                                    id: name.clone(),
                                    title: name,
                                    subtitle,
                                    metadata: None,
                                })
                            })
                            .collect();

                        if items.is_empty() {
                            self.picker.close();
                            self.picker_mode = PickerMode::None;
                            self.messages
                                .push(Message::system("No collaboration modes available."));
                        } else {
                            self.picker.set_items(items);
                        }
                    } else {
                        self.picker.close();
                        self.picker_mode = PickerMode::None;
                        let text = serde_json::to_string_pretty(result).unwrap_or_default();
                        self.messages
                            .push(Message::system(format!("Collaboration modes:\n{}", text)));
                    }
                } else {
                    self.picker.close();
                    self.picker_mode = PickerMode::None;
                    self.handle_rpc_error(json, "Failed to load collaboration modes");
                }
            }
            PendingRequestType::AgentThreadList => {
                if let Some(result) = json.get("result") {
                    // data is Vec<String> (thread IDs), not Vec<objects>
                    if let Some(thread_ids) = result.get("data").and_then(|t| t.as_array()) {
                        let items: Vec<PickerItem> = thread_ids
                            .iter()
                            .filter_map(|t| {
                                let id = t.as_str()?.to_string();
                                let short_id = if id.len() > 12 {
                                    truncate_utf8(&id, 12)
                                } else {
                                    &id
                                };
                                Some(PickerItem {
                                    id: id.clone(),
                                    title: format!("Thread {}", short_id),
                                    subtitle: id.clone(),
                                    metadata: None,
                                })
                            })
                            .collect();

                        if items.is_empty() {
                            self.picker.close();
                            self.picker_mode = PickerMode::None;
                            self.messages
                                .push(Message::system("No active agent threads."));
                        } else {
                            self.picker.set_items(items);
                        }
                    } else {
                        self.picker.close();
                        self.picker_mode = PickerMode::None;
                        let text = serde_json::to_string_pretty(result).unwrap_or_default();
                        self.messages
                            .push(Message::system(format!("Agent threads:\n{}", text)));
                    }
                } else {
                    self.picker.close();
                    self.picker_mode = PickerMode::None;
                    self.handle_rpc_error(json, "Failed to load agent threads");
                }
            }
            PendingRequestType::McpServerList => {
                if let Some(result) = json.get("result") {
                    let mut lines = Vec::new();

                    if let Some(servers) = result.get("data").and_then(|s| s.as_array()) {
                        for server in servers {
                            let name = server
                                .get("name")
                                .and_then(|n| n.as_str())
                                .unwrap_or("unnamed");
                            let auth_status = server
                                .get("authStatus")
                                .and_then(|s| s.as_str())
                                .unwrap_or("unknown");
                            // tools is a HashMap<String, McpTool>, not an array
                            let tool_count = server
                                .get("tools")
                                .and_then(|t| t.as_object())
                                .map(|o| o.len())
                                .unwrap_or(0);
                            lines.push(format!(
                                "  {} [{}] - {} tools",
                                name, auth_status, tool_count
                            ));

                            if let Some(tools) = server.get("tools").and_then(|t| t.as_object()) {
                                for (tool_name, _tool_info) in tools {
                                    lines.push(format!("    - {}", tool_name));
                                }
                            }
                        }
                    }

                    if lines.is_empty() {
                        self.messages
                            .push(Message::system("MCP: No servers configured."));
                    } else {
                        self.messages.push(Message::system(format!(
                            "MCP Servers:\n{}",
                            lines.join("\n")
                        )));
                    }
                } else {
                    self.handle_rpc_error(json, "Failed to load MCP servers");
                }
            }
            PendingRequestType::AppsList => {
                if let Some(result) = json.get("result") {
                    let mut lines = Vec::new();

                    if let Some(apps) = result.get("data").and_then(|a| a.as_array()) {
                        for app in apps {
                            let name = app
                                .get("name")
                                .and_then(|n| n.as_str())
                                .unwrap_or("unnamed");
                            let desc = app
                                .get("description")
                                .and_then(|d| d.as_str())
                                .unwrap_or("");
                            let accessible = app
                                .get("isAccessible")
                                .and_then(|a| a.as_bool())
                                .unwrap_or(false);
                            let status = if accessible { "✓" } else { "✗" };
                            lines.push(format!("  {} {} — {}", status, name, desc));
                        }
                    }

                    if lines.is_empty() {
                        self.messages.push(Message::system("No apps configured."));
                    } else {
                        self.messages
                            .push(Message::system(format!("Apps:\n{}", lines.join("\n"))));
                    }
                } else {
                    self.handle_rpc_error(json, "Failed to load apps");
                }
            }
            PendingRequestType::ConfigRead => {
                if let Some(result) = json.get("result") {
                    self.maybe_update_collaboration_modes_feature_flag(result);
                    self.maybe_update_collaboration_mode_from_config_result(result);
                    let text = serde_json::to_string_pretty(result).unwrap_or_default();
                    self.messages
                        .push(Message::system(format!("Current config:\n{}", text)));
                } else {
                    self.handle_rpc_error(json, "Failed to read config");
                }
            }
            PendingRequestType::DebugConfigRead => {
                if let Some(result) = json.get("result") {
                    let text = serde_json::to_string_pretty(result).unwrap_or_default();
                    self.messages
                        .push(Message::system(format!("Debug config:\n{}", text)));
                } else {
                    self.handle_rpc_error(json, "Failed to read debug config");
                }
            }
            PendingRequestType::StatusRead => {
                if let Some(result) = json.get("result") {
                    self.maybe_update_collaboration_modes_feature_flag(result);
                    self.maybe_update_collaboration_mode_from_config_result(result);
                    let summary = self.render_status_summary(result);
                    self.messages.push(Message::system(&summary));
                } else {
                    self.handle_rpc_error(json, "Failed to load status");
                }
            }
            PendingRequestType::StatuslineConfigRead => {
                if let Some(result) = json.get("result") {
                    self.maybe_update_collaboration_modes_feature_flag(result);
                    let items = Self::parse_statusline_items_from_config(result);
                    self.open_statusline_editor_with_items(items);
                } else {
                    self.handle_rpc_error(json, "Failed to open status line editor");
                }
            }
            PendingRequestType::FeedbackUpload => {
                if json.get("result").is_some() {
                    self.messages.push(Message::system(
                        "Feedback uploaded successfully. Thank you!",
                    ));
                } else {
                    self.handle_rpc_error(json, "Failed to upload feedback");
                }
            }
            PendingRequestType::NewThread => {
                if let Some(result) = json.get("result") {
                    if let Some(thread_id) = result
                        .get("thread")
                        .and_then(|t| t.get("id"))
                        .and_then(|i| i.as_str())
                    {
                        self.set_active_thread_id(thread_id.to_string());
                        self.messages.push(Message::system("New session ready."));
                    }
                } else {
                    self.handle_rpc_error(json, "Failed to start new session");
                }
            }
            PendingRequestType::ForkThread => {
                if let Some(result) = json.get("result") {
                    if let Some(thread_id) = result
                        .get("thread")
                        .and_then(|t| t.get("id"))
                        .and_then(|i| i.as_str())
                    {
                        self.set_active_thread_id(thread_id.to_string());
                        self.messages
                            .push(Message::system("Session forked successfully."));
                    } else {
                        self.messages.push(Message::system("Session forked."));
                    }
                } else {
                    self.handle_rpc_error(json, "Failed to fork session");
                }
            }
            PendingRequestType::RenameThread => {
                if json.get("result").is_some() {
                    self.messages.push(Message::system("Thread renamed."));
                } else {
                    self.handle_rpc_error(json, "Failed to rename thread");
                }
            }
            PendingRequestType::Logout => {
                if json.get("result").is_some() {
                    self.messages
                        .push(Message::system("Logged out. Exiting..."));
                    self.should_quit = true;
                } else {
                    self.handle_rpc_error(json, "Failed to logout");
                }
            }
            PendingRequestType::None => {
                // Unexpected response - ignore
            }
        }
    }

    fn parse_model_entries(result: &serde_json::Value) -> Vec<ModelInfoEntry> {
        let Some(models) = result.get("data").and_then(|m| m.as_array()) else {
            return Vec::new();
        };

        models
            .iter()
            .filter_map(|m| {
                let id = m
                    .get("id")
                    .and_then(|i| i.as_str())
                    .or_else(|| m.get("model").and_then(|i| i.as_str()))?
                    .to_string();
                let display_name = m
                    .get("displayName")
                    .and_then(|n| n.as_str())
                    .or_else(|| m.get("model").and_then(|n| n.as_str()))
                    .unwrap_or(&id)
                    .to_string();
                let description = m
                    .get("description")
                    .and_then(|d| d.as_str())
                    .unwrap_or("")
                    .to_string();
                let default_reasoning_effort = m
                    .get("defaultReasoningEffort")
                    .and_then(|e| e.as_str())
                    .map(|s| s.to_string());
                let supported_reasoning_efforts = m
                    .get("supportedReasoningEfforts")
                    .and_then(|arr| arr.as_array())
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|opt| {
                                let effort = opt
                                    .get("reasoningEffort")
                                    .and_then(|e| e.as_str())?
                                    .to_string();
                                let description = opt
                                    .get("description")
                                    .and_then(|d| d.as_str())
                                    .unwrap_or("")
                                    .to_string();
                                Some(ModelReasoningEffort {
                                    effort,
                                    description,
                                })
                            })
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default();
                let is_default = m
                    .get("isDefault")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);

                Some(ModelInfoEntry {
                    id,
                    display_name,
                    description,
                    supported_reasoning_efforts,
                    default_reasoning_effort,
                    is_default,
                })
            })
            .collect()
    }

    fn model_subtitle(model: &ModelInfoEntry) -> String {
        let mut efforts: Vec<String> = model
            .supported_reasoning_efforts
            .iter()
            .map(|e| e.effort.clone())
            .collect();
        if let Some(default_effort) = &model.default_reasoning_effort {
            if !efforts.iter().any(|e| e == default_effort) {
                efforts.push(default_effort.clone());
            }
        }

        let effort_part = if efforts.is_empty() {
            "effort: unknown".to_string()
        } else if let Some(default_effort) = &model.default_reasoning_effort {
            format!("effort: {} (default {})", efforts.join("/"), default_effort)
        } else {
            format!("effort: {}", efforts.join("/"))
        };

        if model.description.is_empty() {
            effort_part
        } else {
            format!("{}\n{}", effort_part, model.description)
        }
    }

    /// Helper to display RPC error messages
    fn handle_rpc_error(&mut self, json: &serde_json::Value, fallback: &str) {
        if let Some(error) = json.get("error") {
            let error_msg = error
                .get("message")
                .and_then(|m| m.as_str())
                .unwrap_or(fallback);
            self.messages
                .push(Message::system(format!("Error: {}", error_msg)));
        } else {
            self.messages.push(Message::system(fallback));
        }
    }

    #[allow(dead_code)]
    fn request_thread_read(&mut self, thread_id: String) {
        if let Some(tx) = &self.input_tx {
            self.request_counter += 1;
            track_pending_request(
                &mut self.pending_request_type,
                &mut self.pending_requests,
                self.request_counter,
                PendingRequestType::ThreadRead(thread_id.clone()),
            );

            let msg = serde_json::json!({
                "jsonrpc": "2.0",
                "method": "thread/read",
                "id": self.request_counter,
                "params": {
                    "threadId": thread_id,
                    "includeTurns": true
                }
            })
            .to_string();

            // Use try_send since we're not in async context
            let tx = tx.clone();
            tokio::spawn(async move {
                let _ = tx.send(msg).await;
            });
        }
    }

    /// Display turns (from thread/resume or thread/read responses) as chat messages.
    fn display_turns(&mut self, turns: &[serde_json::Value]) {
        for turn in turns {
            if let Some(items) = turn.get("items").and_then(|i| i.as_array()) {
                for item in items {
                    let item_type = item.get("type").and_then(|t| t.as_str()).unwrap_or("");
                    match item_type {
                        "userMessage" => {
                            if let Some(content) = item.get("content").and_then(|c| c.as_array()) {
                                for c in content {
                                    if let Some(text) = c.get("text").and_then(|t| t.as_str()) {
                                        self.messages.push(Message::user(text));
                                    }
                                }
                            }
                        }
                        "agentMessage" => {
                            if let Some(text) = item.get("text").and_then(|t| t.as_str()) {
                                self.messages.push(Message::codex(text));
                            }
                        }
                        "reasoning" => {
                            // Show thinking/reasoning — prefer summary, fallback to content
                            let mut reasoning_text = String::new();
                            if let Some(summary) = item.get("summary").and_then(|s| s.as_array()) {
                                for s in summary {
                                    if let Some(t) = s.as_str() {
                                        if !reasoning_text.is_empty() {
                                            reasoning_text.push('\n');
                                        }
                                        reasoning_text.push_str(t);
                                    }
                                }
                            }
                            if reasoning_text.is_empty() {
                                if let Some(content) =
                                    item.get("content").and_then(|c| c.as_array())
                                {
                                    for c in content {
                                        if let Some(t) = c.as_str() {
                                            if !reasoning_text.is_empty() {
                                                reasoning_text.push('\n');
                                            }
                                            reasoning_text.push_str(t);
                                        }
                                    }
                                }
                            }
                            if !reasoning_text.is_empty() {
                                self.messages.push(Message::thinking(&reasoning_text));
                            }
                        }
                        "commandExecution" => {
                            let cmd = item.get("command").and_then(|c| c.as_str()).unwrap_or("?");
                            let exit_code = item.get("exitCode").and_then(|e| e.as_i64());
                            let duration = item.get("durationMs").and_then(|d| d.as_i64());
                            let output = item
                                .get("aggregatedOutput")
                                .and_then(|o| o.as_str())
                                .unwrap_or("");

                            let mut msg = format!("$ {}", cmd);

                            if !output.is_empty() {
                                let max_output = 2000;
                                if output.len() > max_output {
                                    msg.push_str(&format!(
                                        "\n{}...\n... (output truncated)",
                                        &output[..max_output]
                                    ));
                                } else {
                                    msg.push_str(&format!("\n{}", output));
                                }
                            }

                            // Status line matching new renderer format
                            let dur_str = duration
                                .map(|d| format!(" \u{2022} {}ms", d))
                                .unwrap_or_default();
                            if exit_code.unwrap_or(0) == 0 {
                                msg.push_str(&format!("\n\u{2713}{}", dur_str));
                            } else if let Some(code) = exit_code {
                                msg.push_str(&format!("\n\u{2717} ({}){}", code, dur_str));
                            }

                            self.messages.push(Message::command_exec(msg));
                        }
                        "fileChange" => {
                            if let Some(changes) = item.get("changes").and_then(|c| c.as_array()) {
                                let mut full_diff = String::new();
                                for change in changes {
                                    let raw_path = change
                                        .get("path")
                                        .and_then(|p| p.as_str())
                                        .unwrap_or("unknown");
                                    let path = make_relative_path(raw_path, &self.cwd);
                                    let kind = change
                                        .get("kind")
                                        .and_then(|k| k.get("type"))
                                        .and_then(|t| t.as_str())
                                        .unwrap_or("update");
                                    let verb = match kind {
                                        "add" => "Added",
                                        "delete" => "Deleted",
                                        _ => "Edited",
                                    };
                                    full_diff.push_str(&format!("\u{2022} {} {}\n", verb, path));
                                    if let Some(diff) = change.get("diff").and_then(|d| d.as_str())
                                    {
                                        if !diff.trim().is_empty() {
                                            full_diff.push_str(diff);
                                            if !diff.ends_with('\n') {
                                                full_diff.push('\n');
                                            }
                                        }
                                    }
                                }
                                if !full_diff.is_empty() {
                                    self.messages
                                        .push(Message::file_change(full_diff.trim_end()));
                                }
                            }
                        }
                        "plan" => {
                            if let Some(text) = item.get("text").and_then(|t| t.as_str()) {
                                if !text.is_empty() {
                                    self.messages
                                        .push(Message::system(format!("Plan: {}", text)));
                                }
                            }
                        }
                        _ => {}
                    }
                }
            }
        }
    }

    /// Display session restore turns that include User, Codex, AND Gugugaga messages
    /// in their original chronological order.
    fn display_session_restore(&mut self, restore_turns: &[serde_json::Value]) {
        for turn in restore_turns {
            let role = turn
                .get("role")
                .and_then(|r| r.as_str())
                .unwrap_or("unknown");
            let raw_content = turn.get("content").and_then(|c| c.as_str()).unwrap_or("");
            if raw_content.is_empty() {
                continue;
            }

            match role {
                "user" => {
                    self.messages.push(Message::user(raw_content));
                }
                "user_to_gugugaga" => {
                    self.messages.push(Message::user_to_gugugaga(raw_content));
                }
                "codex" | "assistant" => {
                    // Strip internal annotations that were added for Gugugaga's
                    // context (e.g. [EXECUTED COMMAND], [COMMAND EXIT], [FILE CHANGES]).
                    // These are not meant for display.
                    let cleaned = Self::strip_internal_annotations(raw_content);
                    if !cleaned.is_empty() {
                        self.messages.push(Message::codex(&cleaned));
                    }
                }
                "gugugaga" => {
                    self.messages.push(Message::gugugaga(raw_content));
                }
                _ => {
                    self.messages.push(Message::system(raw_content));
                }
            }
        }
    }

    /// Remove internal annotations ([EXECUTED COMMAND], [COMMAND EXIT], [FILE CHANGES])
    /// from stored Codex turns so they don't leak into the display.
    fn strip_internal_annotations(content: &str) -> String {
        content
            .lines()
            .filter(|line| {
                let trimmed = line.trim();
                !trimmed.starts_with("[EXECUTED COMMAND]")
                    && !trimmed.starts_with("[COMMAND EXIT")
                    && !trimmed.starts_with("[FILE CHANGES]")
                    && !trimmed.starts_with("[FILE CHANGE ")
            })
            .collect::<Vec<_>>()
            .join("\n")
            .trim()
            .to_string()
    }

    fn create_turn_message(&mut self, text: &str, local_images: &[PathBuf]) -> String {
        self.request_counter += 1;
        let thread_id = self.thread_id.as_deref().unwrap_or("main");
        let mut input_items = Vec::<serde_json::Value>::new();
        if !text.trim().is_empty() {
            input_items.push(serde_json::json!({
                "type": "text",
                "text": text,
                "textElements": []
            }));
        }
        for image_path in local_images {
            input_items.push(serde_json::json!({
                "type": "localImage",
                "path": image_path.to_string_lossy().to_string()
            }));
        }
        serde_json::json!({
            "jsonrpc": "2.0",
            "method": "turn/start",
            "id": self.request_counter,
            "params": {
                "threadId": thread_id,
                "input": input_items
            }
        })
        .to_string()
    }

    /// Send turn/interrupt RPC to cancel the current turn
    async fn send_turn_interrupt(&mut self) {
        if let Some(ref tx) = self.input_tx {
            let thread_id = self.thread_id.as_deref().unwrap_or("main");
            self.request_counter += 1;
            let mut params = serde_json::json!({
                "threadId": thread_id
            });
            if let Some(turn_id) = self.current_turn_id.as_ref() {
                params["turnId"] = serde_json::json!(turn_id);
            }
            let msg = serde_json::json!({
                "jsonrpc": "2.0",
                "method": "turn/interrupt",
                "id": self.request_counter,
                "params": params
            })
            .to_string();
            let _ = tx.send(msg).await;
        }
    }

    /// Send gugugaga/interrupt RPC to cancel the local supervision task.
    async fn send_gugugaga_interrupt(&mut self) {
        if let Some(ref tx) = self.input_tx {
            self.request_counter += 1;
            let msg = serde_json::json!({
                "jsonrpc": "2.0",
                "method": "gugugaga/interrupt",
                "id": self.request_counter,
                "params": {}
            })
            .to_string();
            let _ = tx.send(msg).await;
        }
    }

    /// Interrupt whatever is currently running (Codex turn and/or Gugugaga supervision).
    async fn interrupt_active_work(&mut self) -> bool {
        let should_interrupt_turn = self.is_processing;
        // Supervision can be active before status text arrives (or if status events are dropped),
        // so while a turn is active we always send the local supervision interrupt as well.
        let should_interrupt_supervision = should_interrupt_turn || self.gugugaga_status.is_some();
        if !should_interrupt_turn && !should_interrupt_supervision {
            return false;
        }

        if should_interrupt_turn {
            self.send_turn_interrupt().await;
        }
        if should_interrupt_supervision {
            self.send_gugugaga_interrupt().await;
            // Clear status immediately; the interceptor will also send a final check update.
            self.gugugaga_status = None;
        }
        true
    }

    /// Mark the start of processing (sets timer if not already running).
    fn start_processing(&mut self) {
        if !self.is_processing {
            self.turn_start_time = Some(std::time::Instant::now());
        }
        self.is_processing = true;
    }

    /// Mark the end of processing (clears timer).
    fn stop_processing(&mut self) {
        self.is_processing = false;
        self.turn_start_time = None;
    }

    fn scroll_to_bottom(&mut self) {
        self.scroll_offset = 0;
    }

    /// Render the animated welcome screen with optional trust selection.
    ///
    /// Bypasses ratatui's double-buffered diff rendering and writes directly
    /// to the terminal via crossterm so that every animation frame is fully
    /// flushed.
    fn draw_welcome(&mut self) -> io::Result<()> {
        use crossterm::{cursor, terminal as ct, QueueableCommand};
        use std::io::Write as _;

        let frame_text = self.animation.current_frame();
        let mut stdout = io::stdout();

        stdout.queue(cursor::MoveTo(0, 0))?;
        stdout.queue(ct::Clear(ct::ClearType::All))?;

        let (term_w, term_h) = ct::size()?;
        let show_animation = term_w >= MIN_ANIMATION_WIDTH && term_h >= MIN_ANIMATION_HEIGHT;

        // ── Animation ──
        if show_animation {
            for line in frame_text.lines() {
                write!(stdout, "{}\r\n", line)?;
            }
            write!(stdout, "\r\n")?;
        }

        // ── Welcome text ──
        write!(
            stdout,
            "  Welcome to \x1b[1;36mGugugaga\x1b[0m, Codex Supervisor Agent\r\n"
        )?;
        write!(stdout, "\r\n")?;

        // ── Trust selection (if needed) or "press any key" ──
        if let Some(ctx) = &self.trust_ctx {
            write!(
                stdout,
                "  You are running Gugugaga in \x1b[1m{}\x1b[0m\r\n",
                ctx.display_path
            )?;
            write!(stdout, "\r\n")?;

            if ctx.is_git {
                write!(
                    stdout,
                    "  Since this folder is version controlled, you may wish to allow\r\n"
                )?;
                write!(
                    stdout,
                    "  Codex to work in this folder without asking for approval.\r\n"
                )?;
            } else {
                write!(
                    stdout,
                    "  Since this folder is \x1b[33mnot version controlled\x1b[0m, we recommend\r\n"
                )?;
                write!(
                    stdout,
                    "  requiring approval of all edits and commands.\r\n"
                )?;
            }

            write!(stdout, "\r\n")?;
            write!(
                stdout,
                "  \x1b[1;32m[1]\x1b[0m Allow Codex to work without asking for approval\r\n"
            )?;
            write!(
                stdout,
                "  \x1b[1;33m[2]\x1b[0m Require approval of edits and commands\r\n"
            )?;
            write!(stdout, "\r\n")?;
            write!(
                stdout,
                "\x1b[90m  Press 1 or 2 \u{2022} Ctrl+C to quit\x1b[0m"
            )?;
        } else {
            write!(
                stdout,
                "\x1b[90m  Press any key to continue \u{2022} Ctrl+C to quit\x1b[0m"
            )?;
        }

        stdout.flush()?;
        Ok(())
    }

    fn draw(&mut self) -> io::Result<()> {
        let messages = &self.messages;
        let input = &self.input;
        let project_name = &self.project_name;
        let is_processing = self.is_processing;
        let spinner_frame = self.spinner_frame;
        let scroll_offset = self.scroll_offset;
        let slash_popup = &self.slash_popup;
        let picker = &self.picker;
        let pending_approval = &self.pending_approval;
        let approval_scroll = self.approval_scroll;
        let transcript_overlay = self.transcript_overlay.clone();
        let shortcuts_overlay_visible = self.shortcuts_overlay_visible;
        let collaboration_modes_enabled = self.collaboration_modes_enabled();
        let gugugaga_status = &self.gugugaga_status;
        let elapsed_secs = self.turn_start_time.map(|t| t.elapsed().as_secs_f64());

        // These will be filled by the draw closure and written back after
        let mut captured_rect = Rect::default();

        self.terminal.draw(|f| {
            let size = f.area();

            // Main layout: header, content, status, input
            let main_chunks = Layout::default()
                .direction(Direction::Vertical)
                .constraints([
                    Constraint::Length(1), // Header
                    Constraint::Min(8),    // Content
                    Constraint::Length(1), // Status (above input)
                    Constraint::Length(4), // Input
                ])
                .split(size);

            // Header
            let header = HeaderBar {
                title: "Gugugaga",
                project: project_name,
                is_processing,
                spinner_frame,
            };
            f.render_widget(header, main_chunks[0]);

            // Status bar (above input box)
            let status = StatusBar {
                is_processing: is_processing || gugugaga_status.is_some(),
                spinner_frame,
                status_text: if let Some(gs) = gugugaga_status {
                    format!("Supervising: {} (Esc to interrupt)", gs)
                } else if is_processing {
                    "Thinking (Esc to interrupt)".to_string()
                } else {
                    String::new()
                },
                elapsed_secs: if is_processing { elapsed_secs } else { None },
            };
            f.render_widget(status, main_chunks[2]);

            // Messages (full width, no side panel)
            captured_rect = Self::render_messages(f, main_chunks[1], messages, scroll_offset);

            // Input box
            let input_box = InputBox {
                content: &input.buffer,
                cursor: input.cursor,
                focused: true,
            };
            f.render_widget(input_box, main_chunks[3]);

            // Render slash command popup if visible
            if slash_popup.visible {
                Self::render_slash_popup(f, main_chunks[3], slash_popup);
            }

            // Keep cursor aligned with wrapped input rendering (including wide chars/newlines).
            let inner_w = main_chunks[3].width.saturating_sub(2) as usize;
            let inner_h = main_chunks[3].height.saturating_sub(2) as usize;
            let (cursor_row, cursor_col) =
                wrapped_input_cursor_position(&input.buffer, input.cursor, inner_w);
            let scroll_start = cursor_row.saturating_sub(inner_h.saturating_sub(1));
            let visible_row = cursor_row
                .saturating_sub(scroll_start)
                .min(inner_h.saturating_sub(1));
            let visible_col = cursor_col.min(inner_w.saturating_sub(1));
            let overlay_active = pending_approval.is_some()
                || picker.visible
                || transcript_overlay.is_some()
                || shortcuts_overlay_visible;
            if !overlay_active {
                let cursor_x = main_chunks[3].x + 1 + visible_col as u16;
                let cursor_y = main_chunks[3].y + 1 + visible_row as u16;
                f.set_cursor_position((
                    cursor_x.min(main_chunks[3].x + main_chunks[3].width - 2),
                    cursor_y.min(main_chunks[3].y + main_chunks[3].height - 2),
                ));
            }

            // Render picker overlay (on top of everything)
            if picker.visible {
                picker.render(size, f.buffer_mut());
            }

            // Render approval overlay (on top of everything, highest z-order)
            if let Some(approval) = pending_approval {
                Self::render_approval_overlay(f, size, approval, approval_scroll);
            }

            // Render transcript overlay (Codex-style Ctrl+T) above normal UI.
            if let Some(overlay) = transcript_overlay.as_ref() {
                Self::render_transcript_overlay(f, size, messages, overlay.scroll_offset);
            }

            // Render shortcuts overlay (toggled by ?) on top.
            if shortcuts_overlay_visible {
                Self::render_shortcuts_overlay(f, size, collaboration_modes_enabled);
            }
        })?;

        // Write back the captured data from the draw closure
        self.msg_inner_rect = captured_rect;

        Ok(())
    }

    fn render_slash_popup(f: &mut Frame, input_area: Rect, popup: &SlashPopup) {
        let items = popup.display_items();
        let popup_width = input_area.width.saturating_sub(2);
        if popup_width < 4 {
            return;
        }
        let body_rows = items.len().max(1) as u16;
        let popup_height = (body_rows + 2).min(10);

        // Position popup above the input box
        let popup_area = Rect {
            x: input_area.x + 1,
            y: input_area.y.saturating_sub(popup_height),
            width: popup_width,
            height: popup_height,
        };

        // Clear the area
        f.render_widget(Clear, popup_area);

        // Build popup content
        let inner_width = popup_width.saturating_sub(2) as usize;
        let lines: Vec<Line> = if items.is_empty() {
            vec![Line::from(Span::styled(
                "  no matching commands",
                Theme::muted(),
            ))]
        } else {
            items
                .iter()
                .map(|(cmd, desc, selected)| {
                    let prefix = if *selected { "▸ " } else { "  " };
                    let raw = format!("{prefix}{cmd} - {desc}");
                    let line = truncate_to_width_str(&raw, inner_width);
                    let style = if *selected {
                        Theme::accent()
                    } else {
                        Theme::text()
                    };
                    Line::from(Span::styled(line, style))
                })
                .collect()
        };

        let kind = if popup.is_gugugaga {
            "Gugugaga Commands"
        } else {
            "Codex Commands"
        };
        let title = if let Some((page, total_pages)) = popup.page_progress() {
            format!(" {kind} (Tab, PgUp/PgDn) {page}/{total_pages} ")
        } else {
            format!(" {kind} (Tab) ")
        };

        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Theme::accent())
            .title_top(Line::styled(title, Theme::title()));

        let paragraph = Paragraph::new(lines).block(block);
        f.render_widget(paragraph, popup_area);
    }

    fn render_transcript_overlay(
        f: &mut Frame,
        area: Rect,
        messages: &[Message],
        scroll_offset: usize,
    ) {
        use ratatui::style::{Color, Style};

        if area.width < 12 || area.height < 8 {
            return;
        }
        let overlay_area = Rect {
            x: area.x + 1,
            y: area.y + 1,
            width: area.width.saturating_sub(2),
            height: area.height.saturating_sub(2),
        };
        f.render_widget(Clear, overlay_area);

        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Theme::accent())
            .title_top(Line::styled(
                " Transcript (Ctrl+T / Esc to close) ",
                Theme::title(),
            ));
        let inner = block.inner(overlay_area);
        f.render_widget(block, overlay_area);
        if inner.width == 0 || inner.height == 0 {
            return;
        }

        let mut all_lines: Vec<Line> = Vec::new();
        for msg in messages {
            all_lines.extend(render_message_lines(msg, inner.width as usize));
        }
        if all_lines.is_empty() {
            all_lines.push(Line::from(Span::styled(
                "  No transcript yet.",
                Theme::muted(),
            )));
        }

        let total_lines = all_lines.len();
        let visible_height = inner.height as usize;
        let max_scroll = total_lines.saturating_sub(visible_height);
        let actual_scroll = scroll_offset.min(max_scroll);
        let start = total_lines
            .saturating_sub(visible_height)
            .saturating_sub(actual_scroll);
        let visible: Vec<Line> = all_lines
            .into_iter()
            .skip(start)
            .take(visible_height)
            .collect();
        f.render_widget(Paragraph::new(visible), inner);

        if total_lines > visible_height {
            let indicator = if actual_scroll > 0 && actual_scroll < max_scroll {
                format!("↑↓ {}/{}", actual_scroll + 1, total_lines)
            } else if actual_scroll > 0 {
                format!("↑ {}/{}", actual_scroll + 1, total_lines)
            } else {
                format!("↓ {}/{}", actual_scroll + 1, total_lines)
            };
            let ind_len = indicator.len() as u16;
            if inner.width > ind_len + 1 {
                let ind_area = Rect {
                    x: inner.x + inner.width - ind_len - 1,
                    y: inner.y,
                    width: ind_len,
                    height: 1,
                };
                f.render_widget(
                    Paragraph::new(Span::styled(
                        indicator,
                        Style::default().fg(Color::DarkGray),
                    )),
                    ind_area,
                );
            }
        }
    }

    fn render_shortcuts_overlay(f: &mut Frame, area: Rect, collaboration_modes_enabled: bool) {
        let mut lines = vec![
            Line::from("  /  for Codex commands"),
            Line::from("  // for Gugugaga commands"),
            Line::from("  Tab  complete command"),
            Line::from("  Enter  send message"),
            Line::from("  Ctrl+V / Alt+V  paste image"),
            Line::from("  Esc  interrupt active work"),
            Line::from("  Ctrl+C  clear draft, interrupt, then quit"),
            Line::from("  Ctrl+D  quit when composer is empty"),
            Line::from("  Ctrl+T  open transcript view"),
        ];
        if collaboration_modes_enabled {
            lines.push(Line::from("  Shift+Tab  change collaboration mode"));
        }
        lines.push(Line::from(""));
        lines.push(Line::from("  ? / Esc  close this panel"));

        let content_h = lines.len() as u16 + 2;
        let overlay_width = area.width.saturating_sub(8).min(72);
        let overlay_height = content_h.min(area.height.saturating_sub(4));
        if overlay_width < 12 || overlay_height < 6 {
            return;
        }
        let x = area.x + (area.width.saturating_sub(overlay_width)) / 2;
        let y = area.y + (area.height.saturating_sub(overlay_height)) / 2;
        let overlay_area = Rect {
            x,
            y,
            width: overlay_width,
            height: overlay_height,
        };
        f.render_widget(Clear, overlay_area);

        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Theme::accent())
            .title_top(Line::styled(
                " Shortcuts (? / Esc to close) ",
                Theme::title(),
            ));
        let inner = block.inner(overlay_area);
        f.render_widget(block, overlay_area);
        if inner.width == 0 || inner.height == 0 {
            return;
        }

        let visible: Vec<Line> = lines.into_iter().take(inner.height as usize).collect();
        f.render_widget(Paragraph::new(visible), inner);
    }

    fn render_approval_overlay(
        f: &mut Frame,
        area: Rect,
        approval: &PendingApproval,
        scroll: usize,
    ) {
        use ratatui::style::{Color, Modifier, Style};
        use ratatui::text::{Line, Span};

        let opt_style = Style::default()
            .fg(Color::Yellow)
            .add_modifier(Modifier::BOLD);
        let desc_style = Style::default().fg(Color::White);

        // --- Build footer lines (options) — these are ALWAYS visible ---
        let mut footer_lines: Vec<Line> = Vec::new();
        footer_lines.push(Line::from(vec![
            Span::styled("  [y] ", opt_style),
            Span::styled("Yes, proceed", desc_style),
        ]));

        match approval.approval_type {
            ApprovalType::CommandExecution => {
                if let Some(ref amendment) = approval.proposed_execpolicy_amendment {
                    let prefix = amendment.join(" ");
                    footer_lines.push(Line::from(vec![
                        Span::styled("  [p] ", opt_style),
                        Span::styled(format!("Yes, don't ask again for `{}`", prefix), desc_style),
                    ]));
                }
            }
            ApprovalType::FileChange => {
                footer_lines.push(Line::from(vec![
                    Span::styled("  [a] ", opt_style),
                    Span::styled("Yes, don't ask again for these files", desc_style),
                ]));
            }
        }

        footer_lines.push(Line::from(vec![
            Span::styled("  [n] ", opt_style),
            Span::styled("No, cancel  ", desc_style),
            Span::styled("Esc", Style::default().fg(Color::DarkGray)),
            Span::styled(" also cancels", Style::default().fg(Color::DarkGray)),
        ]));

        let footer_height = footer_lines.len() as u16;

        // --- Build content lines (title + reason + command) ---
        let mut content_lines: Vec<Line> = Vec::new();

        let title_text = match approval.approval_type {
            ApprovalType::CommandExecution => "Would you like to run the following command?",
            ApprovalType::FileChange => "Would you like to make the following edits?",
        };
        content_lines.push(Line::from(Span::styled(
            title_text,
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        )));
        content_lines.push(Line::from(""));

        if let Some(ref reason) = approval.reason {
            content_lines.push(Line::from(vec![
                Span::styled("Reason: ", Style::default().fg(Color::Gray)),
                Span::styled(
                    reason.as_str(),
                    Style::default()
                        .fg(Color::White)
                        .add_modifier(Modifier::ITALIC),
                ),
            ]));
            content_lines.push(Line::from(""));
        }

        match approval.approval_type {
            ApprovalType::CommandExecution => {
                let cmd = approval.command.as_deref().unwrap_or("(unknown)");
                content_lines.push(Line::from(vec![
                    Span::styled(
                        "  $ ",
                        Style::default()
                            .fg(Color::Green)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(cmd.to_string(), Style::default().fg(Color::White)),
                ]));
                if let Some(ref cwd) = approval.cwd {
                    content_lines.push(Line::from(Span::styled(
                        format!("    in {}", cwd),
                        Style::default().fg(Color::DarkGray),
                    )));
                }
            }
            ApprovalType::FileChange => {
                content_lines.push(Line::from(Span::styled(
                    "  (file modifications pending)",
                    Style::default().fg(Color::Cyan),
                )));
            }
        }

        // --- Layout: calculate overlay size ---
        // border(2) + content + separator(1) + footer
        let ideal_height = 2 + content_lines.len() as u16 + 1 + footer_height;
        let overlay_height = ideal_height.min(area.height.saturating_sub(4));
        let overlay_width = 64.min(area.width.saturating_sub(4));

        let x = area.x + (area.width.saturating_sub(overlay_width)) / 2;
        let y = area.y + (area.height.saturating_sub(overlay_height)) / 2;
        let overlay_area = Rect {
            x,
            y,
            width: overlay_width,
            height: overlay_height,
        };

        f.render_widget(Clear, overlay_area);

        // Outer border
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Yellow))
            .title_top(Line::styled(
                " ⚡ APPROVAL REQUIRED ",
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            ));
        let inner = block.inner(overlay_area);
        f.render_widget(block, overlay_area);

        if inner.height == 0 || inner.width == 0 {
            return;
        }

        // Split inner into: content area (flexible) + separator (1 line) + footer (fixed)
        let sep_and_footer = 1 + footer_height;
        let content_height = inner.height.saturating_sub(sep_and_footer);

        let content_area = Rect {
            x: inner.x,
            y: inner.y,
            width: inner.width,
            height: content_height,
        };
        let sep_area = Rect {
            x: inner.x,
            y: inner.y + content_height,
            width: inner.width,
            height: 1.min(inner.height.saturating_sub(content_height)),
        };
        let footer_area = Rect {
            x: inner.x,
            y: inner.y + content_height + sep_area.height,
            width: inner.width,
            height: footer_height.min(
                inner
                    .height
                    .saturating_sub(content_height + sep_area.height),
            ),
        };

        // Render content with scroll support
        let content_h = content_area.height as usize;
        let total_content = content_lines.len();
        let max_scroll = total_content.saturating_sub(content_h);
        let actual_scroll = scroll.min(max_scroll);

        let visible_content: Vec<Line> = content_lines
            .into_iter()
            .skip(actual_scroll)
            .take(content_h)
            .collect();

        let content_para = Paragraph::new(visible_content).wrap(Wrap { trim: false });
        f.render_widget(content_para, content_area);

        // Scroll indicator (if content is scrollable)
        if total_content > content_h {
            let indicator = if actual_scroll > 0 && actual_scroll < max_scroll {
                format!("↑↓ {}/{}", actual_scroll + 1, total_content)
            } else if actual_scroll > 0 {
                format!("↑ {}/{}", actual_scroll + 1, total_content)
            } else {
                format!("↓ {}/{}", actual_scroll + 1, total_content)
            };
            // Draw indicator at top-right of content area
            let ind_len = indicator.len() as u16;
            if content_area.width > ind_len + 1 {
                let ind_area = Rect {
                    x: content_area.x + content_area.width - ind_len - 1,
                    y: content_area.y,
                    width: ind_len,
                    height: 1,
                };
                let ind_widget = Paragraph::new(Span::styled(
                    indicator,
                    Style::default().fg(Color::DarkGray),
                ));
                f.render_widget(ind_widget, ind_area);
            }
        }

        // Render separator line
        if sep_area.height > 0 {
            let sep_line = "─".repeat(inner.width as usize);
            let sep = Paragraph::new(Line::from(Span::styled(
                sep_line,
                Style::default().fg(Color::DarkGray),
            )));
            f.render_widget(sep, sep_area);
        }

        // Render footer (options) — always visible at bottom
        let footer_para = Paragraph::new(footer_lines);
        f.render_widget(footer_para, footer_area);
    }

    /// Render messages and return the inner rect used for message layout.
    fn render_messages(
        f: &mut Frame,
        area: Rect,
        messages: &[Message],
        scroll_offset: usize,
    ) -> Rect {
        // No border — clean layout like Codex
        let inner = area;
        if inner.width == 0 || inner.height == 0 {
            return inner;
        }

        let visible_height = inner.height as usize;
        let mut all_lines = Vec::new();
        for msg in messages {
            all_lines.extend(render_message_lines(msg, inner.width as usize));
        }

        // Calculate scroll
        let total_lines = all_lines.len();
        let max_scroll = total_lines.saturating_sub(visible_height);
        let actual_scroll = scroll_offset.min(max_scroll);

        // Get visible lines (from bottom, with scroll offset)
        let start = total_lines
            .saturating_sub(visible_height)
            .saturating_sub(actual_scroll);
        let visible: Vec<Line> = all_lines
            .into_iter()
            .skip(start)
            .take(visible_height)
            .collect();

        let paragraph = Paragraph::new(visible);
        f.render_widget(paragraph, inner);

        inner
    }
}

impl Drop for App {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        let _ = execute!(
            self.terminal.backend_mut(),
            crossterm::event::DisableBracketedPaste
        );
        let _ = self.terminal.show_cursor();
    }
}

#[cfg(test)]
mod tests {
    use super::{take_pending_request_type, PendingRequestType};
    use std::collections::HashMap;

    #[test]
    fn pending_requests_match_by_id_without_overwrite() {
        let mut pending = HashMap::new();
        pending.insert(101, PendingRequestType::StatusRead);
        pending.insert(102, PendingRequestType::ThreadList);

        let first = take_pending_request_type(&mut pending, 101);
        assert_eq!(first, Some(PendingRequestType::StatusRead));
        assert_eq!(pending.get(&102), Some(&PendingRequestType::ThreadList));
    }

    #[test]
    fn unknown_response_id_does_not_clear_pending_requests() {
        let mut pending = HashMap::new();
        pending.insert(201, PendingRequestType::ModelList);
        pending.insert(202, PendingRequestType::AppsList);

        let unmatched = take_pending_request_type(&mut pending, 999);
        assert_eq!(unmatched, None);
        assert_eq!(pending.len(), 2);
    }
}
