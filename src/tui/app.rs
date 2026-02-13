//! Main TUI application

use std::io::{self, Stdout};
use std::sync::Arc;
use std::time::Duration;

use crossterm::{
    event::{self, Event, MouseEventKind, MouseButton},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout, Rect},
    style::Stylize,
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState, Wrap},
    Frame, Terminal,
};
use tokio::sync::{mpsc, RwLock};

use crate::memory::GugugagaNotebook;

use super::ascii_animation::AsciiAnimation;
use super::input::{InputAction, InputState};
use super::picker::{Picker, PickerItem};
use super::slash_commands::{parse_command, CodexCommand, ParsedCommand, SlashPopup, GugugagaCommand};
use super::theme::Theme;
use super::widgets::{
    render_message_lines, HeaderBar, HelpBar, InputBox, Message, MessageRole, StatusBar,
    ContextPanel,
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

/// Current picker mode
#[derive(Debug, Clone, PartialEq, Eq)]
enum PickerMode {
    None,
    Resume,
    Model,
    SkillsMenu,       // First-level: "List skills" / "Enable/Disable"
    SkillsSelect,     // Second-level: select a skill to insert as $mention
    SkillsManage,     // Second-level: toggle individual skills on/off
    Approvals,
    Permissions,
    Personality,
    Collab,
    Agent,
}

/// Type of pending request
#[derive(Debug, Clone, PartialEq)]
enum PendingRequestType {
    None,
    ThreadList,
    ThreadResume(String),
    ThreadRead(String),
    ModelList,
    SkillsList,
    CollabModeList,
    AgentThreadList,
    McpServerList,
    AppsList,
    ConfigRead,
    FeedbackUpload,
    NewThread,
    ForkThread,
    RenameThread,
    Logout,
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
enum ApprovalType {
    CommandExecution,
    FileChange,
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
    /// Whether gugugaga monitoring is paused
    is_paused: bool,
    /// Generic picker for resume/model selection
    picker: Picker,
    /// What the picker is currently for
    picker_mode: PickerMode,
    /// Pending request ID
    pending_request_id: Option<u64>,
    /// Type of pending request
    pending_request_type: PendingRequestType,
    /// Request ID counter
    request_counter: u64,
    /// Current thread ID (from thread/start response)
    thread_id: Option<String>,
    /// Current turn ID (from turn/started notification, needed for interrupt)
    current_turn_id: Option<String>,
    /// Ctrl+C double-press quit: armed when first Ctrl+C is pressed
    quit_armed: bool,
    /// Timestamp of first Ctrl+C press for double-press timeout
    quit_armed_at: Option<std::time::Instant>,
    /// Pending approval request (waiting for user response)
    pending_approval: Option<PendingApproval>,
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
    /// Current application phase (Welcome animation → Chat).
    phase: AppPhase,
    /// ASCII art animation for the welcome screen.
    animation: AsciiAnimation,
    /// Trust onboarding context. `Some` = user still needs to choose.
    trust_ctx: Option<crate::trust::TrustContext>,

    // ── Mouse selection state ─────────────────────────────────
    /// Whether the user is currently drag-selecting with the mouse.
    selecting: bool,
    /// Anchor point of the selection (screen row relative to message inner area, col).
    sel_anchor: Option<(u16, u16)>,
    /// Current end of the selection (screen row, col).
    sel_end: Option<(u16, u16)>,
    /// Rendered line texts cached during draw() for selection copy.
    /// Index = screen-row relative to the inner message area.
    rendered_lines: Vec<String>,
    /// The Rect of the inner message area (set each draw frame).
    msg_inner_rect: Rect,
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
        execute!(stdout, EnterAlternateScreen, crossterm::event::EnableMouseCapture)?;
        let backend = CrosstermBackend::new(stdout);
        let terminal = Terminal::new(backend)?;

        Ok(Self {
            terminal,
            input: InputState::new(),
            messages: vec![
                Message::system("Gugugaga initialized. Monitoring enabled."),
                Message::system("Commands: /cmd = Codex, //cmd = Gugugaga. Tab to autocomplete."),
            ],
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
            is_paused: false,
            picker: Picker::new("Select"),
            picker_mode: PickerMode::None,
            pending_request_id: None,
            pending_request_type: PendingRequestType::None,
            request_counter: 100, // Start after other request IDs
            thread_id: None,
            current_turn_id: None,
            quit_armed: false,
            quit_armed_at: None,
            pending_approval: None,
            approval_scroll: 0,
            notebook: None,
            notebook_current_activity: None,
            notebook_completed_count: 0,
            notebook_attention_items: Vec::new(),
            notebook_mistakes_count: 0,
            gugugaga_status: None,
            // Skip the Welcome animation if trust is already established
            phase: if trust_ctx.is_some() {
                AppPhase::Welcome
            } else {
                AppPhase::Chat
            },
            animation: AsciiAnimation::new(),
            trust_ctx,
            selecting: false,
            sel_anchor: None,
            sel_end: None,
            rendered_lines: Vec::new(),
            msg_inner_rect: Rect::default(),
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
            self.notebook_attention_items = nb.attention
                .iter()
                .map(|item| (item.content.clone(), item.priority == crate::memory::Priority::High))
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
                        match event::read()? {
                            Event::Key(key) => {
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
                            // Silently consume non-key events
                            _ => {}
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
                        let msg = format!("draw() error: {e}\nmessage_count: {}\n", self.messages.len());
                        let _ = std::fs::write("gugugaga-crash.log", &msg);
                        return Err(e);
                    }

                    // Poll for keyboard/mouse events with short timeout
                    if event::poll(poll_timeout)? {
                        match event::read()? {
                            Event::Key(key) => {
                                self.handle_input(key).await;
                            }
                            Event::Mouse(mouse) => {
                                self.handle_mouse(mouse);
                            }
                            _ => {}
                        }
                    }
                }
            }
        }
        
        Ok(())
    }

    /// Handle mouse events: selection drag, scroll wheel, auto-scroll at edges.
    fn handle_mouse(&mut self, mouse: crossterm::event::MouseEvent) {
        let rect = self.msg_inner_rect;
        // Ignore if message area hasn't been laid out yet
        if rect.width == 0 || rect.height == 0 {
            return;
        }

        match mouse.kind {
            MouseEventKind::ScrollUp => {
                self.scroll_offset = self.scroll_offset.saturating_add(3);
            }
            MouseEventKind::ScrollDown => {
                self.scroll_offset = self.scroll_offset.saturating_sub(3);
            }
            MouseEventKind::Down(MouseButton::Left) => {
                // Start selection if click is in the message area
                if mouse.column >= rect.x
                    && mouse.column < rect.x + rect.width
                    && mouse.row >= rect.y
                    && mouse.row < rect.y + rect.height
                {
                    let rel_row = mouse.row - rect.y;
                    let rel_col = mouse.column - rect.x;
                    self.selecting = true;
                    self.sel_anchor = Some((rel_row, rel_col));
                    self.sel_end = Some((rel_row, rel_col));
                } else {
                    // Click outside message area — clear selection
                    self.selecting = false;
                    self.sel_anchor = None;
                    self.sel_end = None;
                }
            }
            MouseEventKind::Drag(MouseButton::Left) => {
                if self.selecting {
                    let rel_row = mouse.row.saturating_sub(rect.y);
                    let rel_col = mouse.column.saturating_sub(rect.x);
                    self.sel_end = Some((rel_row.min(rect.height - 1), rel_col));

                    // Auto-scroll when dragging near edges
                    if mouse.row <= rect.y + 1 {
                        // Near top edge — scroll up
                        self.scroll_offset = self.scroll_offset.saturating_add(1);
                    } else if mouse.row >= rect.y + rect.height - 2 {
                        // Near bottom edge — scroll down
                        self.scroll_offset = self.scroll_offset.saturating_sub(1);
                    }
                }
            }
            MouseEventKind::Up(MouseButton::Left) => {
                if self.selecting {
                    self.selecting = false;
                    // Copy selected text to clipboard
                    if let (Some(anchor), Some(end)) = (self.sel_anchor, self.sel_end) {
                        let text = self.extract_selected_text(anchor, end);
                        if !text.is_empty() {
                            Self::copy_to_clipboard(&text);
                        }
                    }
                    // Keep selection visible (don't clear anchor/end yet)
                    // It will be cleared on next mouse down
                }
            }
            _ => {}
        }
    }

    /// Extract the text between two selection points from the rendered lines.
    fn extract_selected_text(&self, anchor: (u16, u16), end: (u16, u16)) -> String {
        // Normalize so start <= end
        let (start, finish) = if anchor.0 < end.0 || (anchor.0 == end.0 && anchor.1 <= end.1) {
            (anchor, end)
        } else {
            (end, anchor)
        };

        let mut result = String::new();
        for row in start.0..=finish.0 {
            let idx = row as usize;
            if idx >= self.rendered_lines.len() {
                break;
            }
            let line = &self.rendered_lines[idx];
            let chars: Vec<char> = line.chars().collect();

            let col_start = if row == start.0 { start.1 as usize } else { 0 };
            let col_end = if row == finish.0 {
                (finish.1 as usize + 1).min(chars.len())
            } else {
                chars.len()
            };

            let selected: String = chars
                .get(col_start..col_end)
                .unwrap_or(&[])
                .iter()
                .collect();
            result.push_str(&selected);
            if row < finish.0 {
                result.push('\n');
            }
        }
        result
    }

    /// Copy text to system clipboard (macOS: pbcopy, Linux: xclip/xsel).
    fn copy_to_clipboard(text: &str) {
        use std::process::{Command, Stdio};
        // macOS
        if let Ok(mut child) = Command::new("pbcopy")
            .stdin(Stdio::piped())
            .spawn()
        {
            if let Some(mut stdin) = child.stdin.take() {
                use std::io::Write;
                let _ = stdin.write_all(text.as_bytes());
            }
            let _ = child.wait();
            return;
        }
        // Linux fallback: xclip
        if let Ok(mut child) = Command::new("xclip")
            .args(["-selection", "clipboard"])
            .stdin(Stdio::piped())
            .spawn()
        {
            if let Some(mut stdin) = child.stdin.take() {
                use std::io::Write;
                let _ = stdin.write_all(text.as_bytes());
            }
            let _ = child.wait();
        }
    }

    async fn handle_input(&mut self, key: event::KeyEvent) {
        // Handle approval dialog first (highest priority — modal overlay)
        if let Some(approval) = self.pending_approval.take() {
            match key.code {
                // y / Enter — Accept (approve this time)
                crossterm::event::KeyCode::Char('y') | crossterm::event::KeyCode::Char('Y') |
                crossterm::event::KeyCode::Enter => {
                    let cmd_display = approval.command.as_deref().unwrap_or("command");
                    self.messages.push(Message::system(&format!("✓ Approved: {}", cmd_display)));
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
                    self.messages.push(Message::system(&format!(
                        "✓ Approved (won't ask again for `{}`)", prefix
                    )));
                    self.respond_to_approval_with_amendment(&approval, &amendment).await;
                    return;
                }
                // a — Accept for session (file changes: don't ask again for these files)
                crossterm::event::KeyCode::Char('a') | crossterm::event::KeyCode::Char('A')
                    if matches!(approval.approval_type, ApprovalType::FileChange) =>
                {
                    self.messages.push(Message::system("✓ Approved (for session)"));
                    self.respond_to_approval_decision(&approval, "acceptForSession").await;
                    return;
                }
                // n / Esc — Cancel (reject + interrupt turn)
                crossterm::event::KeyCode::Char('n') | crossterm::event::KeyCode::Char('N') |
                crossterm::event::KeyCode::Esc => {
                    self.messages.push(Message::system("✗ Cancelled"));
                    self.respond_to_approval_decision(&approval, "cancel").await;
                    return;
                }
                // Ctrl+C during approval — also cancel (same as Codex)
                crossterm::event::KeyCode::Char('c')
                    if key.modifiers.contains(crossterm::event::KeyModifiers::CONTROL) =>
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
                crossterm::event::KeyCode::Tab | crossterm::event::KeyCode::Enter => {
                    // Complete selection
                    if let Some(completed) = self.slash_popup.complete() {
                        self.input.set_buffer(&completed);
                        self.slash_popup.close();
                    }
                    return;
                }
                crossterm::event::KeyCode::Esc => {
                    self.slash_popup.close();
                    return;
                }
                _ => {}
            }
        }

        // Reset quit_armed if timeout exceeded (2 seconds)
        if self.quit_armed {
            if let Some(armed_at) = self.quit_armed_at {
                if armed_at.elapsed() > std::time::Duration::from_secs(2) {
                    self.quit_armed = false;
                    self.quit_armed_at = None;
                }
            }
        }

        match self.input.handle_key(key) {
            InputAction::Quit => {
                // Ctrl+C: double-press mechanism
                if self.is_processing && self.current_turn_id.is_some() {
                    // Interrupt the running turn
                    self.send_turn_interrupt().await;
                } else if self.quit_armed {
                    // Second press within timeout — actually quit
                    self.should_quit = true;
                } else {
                    // First press — arm the quit
                    self.quit_armed = true;
                    self.quit_armed_at = Some(std::time::Instant::now());
                    self.messages.push(Message::system(
                        "Press Ctrl+C again to quit."
                    ));
                    self.scroll_to_bottom();
                }
            }
            InputAction::Submit(text) => {
                self.slash_popup.close();

                // Parse and handle command
                if let Some(parsed) = parse_command(&text) {
                    match parsed {
                        ParsedCommand::Codex(cmd, args) => {
                            // Forward to Codex
                            self.forward_codex_command(cmd, args).await;
                        }
                        ParsedCommand::Gugugaga(cmd, args) => {
                            self.execute_gugugaga_command(cmd, args);
                        }
                        ParsedCommand::Unknown(name) => {
                            self.messages.push(Message::system(&format!(
                                "Unknown command: {}. Use //help for Gugugaga commands.",
                                name
                            )));
                        }
                    }
                } else {
                    // Regular message to Codex - block if already processing
                    if self.is_processing {
                        self.messages.push(Message::system("⏳ Please wait for current processing"));
                        return;
                    }
                    self.messages.push(Message::user(&text));
                    self.scroll_to_bottom();
                    self.is_processing = true;

                    let msg = self.create_turn_message(&text);
                    if let Some(tx) = &self.input_tx {
                        let _ = tx.send(msg).await;
                    }
                }
            }
            InputAction::ScrollUp => {
                // PageUp - scroll up by multiple lines
                self.scroll_offset = self.scroll_offset.saturating_add(5);
            }
            InputAction::ScrollDown => {
                // PageDown - scroll down by multiple lines
                self.scroll_offset = self.scroll_offset.saturating_sub(5);
            }
            InputAction::HistoryPrev => {
                // Up arrow - scroll up by one line (alternate scroll mode converts wheel to arrows)
                self.scroll_offset = self.scroll_offset.saturating_add(1);
            }
            InputAction::HistoryNext => {
                // Down arrow - scroll down by one line
                self.scroll_offset = self.scroll_offset.saturating_sub(1);
            }
            InputAction::Tab => {
                self.handle_tab_completion();
            }
            InputAction::Escape => {
                // Priority: slash_popup > running turn > nothing
                if self.slash_popup.visible {
                    self.slash_popup.close();
                } else if self.is_processing && self.current_turn_id.is_some() {
                    self.send_turn_interrupt().await;
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
            InputAction::Input(_) | InputAction::Backspace | InputAction::DeleteWord => {
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
                self.show_status();
            }
            CodexCommand::Diff => {
                self.show_git_diff().await;
            }
            CodexCommand::Ps => {
                self.show_background_processes();
            }

            // === RPC commands (forward to Codex with correct method) ===
            CodexCommand::Model => {
                self.request_model_list().await;
            }
            CodexCommand::Skills => {
                self.open_skills_menu().await;
            }
            CodexCommand::Review => {
                self.request_review().await;
            }
            CodexCommand::Rename => {
                if args.is_empty() {
                    self.messages.push(Message::system("Usage: /rename <new name>"));
                } else {
                    self.request_rename(&args).await;
                }
            }
            CodexCommand::Fork => {
                self.request_fork().await;
            }
            CodexCommand::Logout => {
                self.request_logout().await;
            }
            CodexCommand::Feedback => {
                self.request_feedback().await;
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
            }
            CodexCommand::Agent => {
                self.open_agent_picker().await;
            }

            // === Special commands ===
            CodexCommand::Init => {
                self.execute_init().await;
            }
            CodexCommand::Mention => {
                // Insert @ at cursor - simple local action
                self.input.buffer.push('@');
                self.input.cursor += 1;
            }
            CodexCommand::ElevateSandbox => {
                self.messages.push(Message::system(
                    "/setup-elevated-sandbox is only available on Windows"
                ));
            }
        }
    }

    /// Request a new thread
    async fn request_new_thread(&mut self) {
        if let Some(tx) = &self.input_tx {
            self.request_counter += 1;
            self.pending_request_id = Some(self.request_counter);
            self.pending_request_type = PendingRequestType::NewThread;
            let msg = serde_json::json!({
                "jsonrpc": "2.0",
                "method": "thread/start",
                "id": self.request_counter,
                "params": {}
            })
            .to_string();
            let _ = tx.send(msg).await;
            self.messages.clear();
            self.messages.push(Message::system("Starting new session..."));
        }
    }

    /// Show status information
    fn show_status(&mut self) {
        let status = format!(
            "Session Status:\n  Violations detected: {}\n  Corrections made: {}\n  Auto-replies: {}\n  Monitoring: {}",
            self.violations_detected,
            self.corrections_made,
            self.auto_replies,
            if self.is_paused { "Paused" } else { "Active" }
        );
        self.messages.push(Message::system(&status));
    }

    /// Show git diff
    async fn show_git_diff(&mut self) {
        // Run git diff command
        match tokio::process::Command::new("git")
            .args(["diff", "--stat"])
            .current_dir(&self.cwd)
            .output()
            .await
        {
            Ok(output) => {
                let stdout = String::from_utf8_lossy(&output.stdout);
                let stderr = String::from_utf8_lossy(&output.stderr);
                if !stdout.is_empty() {
                    self.messages.push(Message::system(&format!("Git diff:\n{}", stdout)));
                } else if !stderr.is_empty() {
                    self.messages.push(Message::system(&format!("Git error: {}", stderr)));
                } else {
                    self.messages.push(Message::system("No changes detected"));
                }
            }
            Err(e) => {
                self.messages.push(Message::system(&format!("Failed to run git: {}", e)));
            }
        }
    }

    /// Show background processes - lists recent command executions from message history
    fn show_background_processes(&mut self) {
        let running: Vec<&str> = self.messages.iter()
            .filter(|m| m.role == MessageRole::CommandExec)
            .map(|m| m.content.as_str())
            .collect();

        if running.is_empty() {
            self.messages.push(Message::system("No command executions in this session."));
        } else {
            let last_n = running.len().min(10);
            let display: Vec<String> = running[running.len() - last_n..]
                .iter()
                .enumerate()
                .map(|(i, cmd)| {
                    let preview = truncate_utf8(cmd, 60);
                    format!("  [{}] {}", i + 1, preview)
                })
                .collect();
            self.messages.push(Message::system(&format!(
                "Recent command executions ({}):\n{}",
                running.len(),
                display.join("\n")
            )));
        }
    }

    /// Request model list
    async fn request_model_list(&mut self) {
        self.picker_mode = PickerMode::Model;
        self.picker.title = "Select Model".to_string();
        self.picker.open_loading();

        if let Some(tx) = &self.input_tx {
            self.request_counter += 1;
            self.pending_request_id = Some(self.request_counter);
            self.pending_request_type = PendingRequestType::ModelList;
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

    /// Open skills menu - first level picker
    async fn open_skills_menu(&mut self) {
        self.picker_mode = PickerMode::SkillsMenu;
        self.picker.title = "Skills".to_string();
        let items = vec![
            PickerItem { id: "list".to_string(), title: "List skills".to_string(), subtitle: "Show all available skills".to_string(), metadata: None },
            PickerItem { id: "manage".to_string(), title: "Enable/Disable skills".to_string(), subtitle: "Toggle individual skills on/off".to_string(), metadata: None },
        ];
        self.picker.open(items);
    }

    /// Fetch skills list from Codex (used by both list and manage flows)
    async fn fetch_skills_list(&mut self) {
        if let Some(tx) = &self.input_tx {
            self.request_counter += 1;
            self.pending_request_id = Some(self.request_counter);
            self.pending_request_type = PendingRequestType::SkillsList;
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
            let action = if currently_enabled { "Disabled" } else { "Enabled" };
            // Extract just the skill name from the path for display
            let display_name = std::path::Path::new(skill_path)
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or(skill_path);
            self.messages.push(Message::system(&format!("{} skill: {}", action, display_name)));
        }
    }

    /// Request code review
    async fn request_review(&mut self) {
        if let Some(thread_id) = &self.thread_id {
            if let Some(tx) = &self.input_tx {
                self.request_counter += 1;
                let msg = serde_json::json!({
                    "jsonrpc": "2.0",
                    "method": "review/start",
                    "id": self.request_counter,
                    "params": {
                        "threadId": thread_id,
                        "target": {
                            "type": "uncommittedChanges"
                        }
                    }
                })
                .to_string();
                let _ = tx.send(msg).await;
                self.messages.push(Message::system("Starting code review (uncommitted changes)..."));
                self.is_processing = true;
            }
        } else {
            self.messages.push(Message::system("No active thread. Start a conversation first."));
        }
    }

    /// Request thread rename
    async fn request_rename(&mut self, name: &str) {
        if let Some(thread_id) = &self.thread_id {
            if let Some(tx) = &self.input_tx {
                self.request_counter += 1;
                self.pending_request_id = Some(self.request_counter);
                self.pending_request_type = PendingRequestType::RenameThread;
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
                self.messages.push(Message::system(&format!("Renaming to: {}", name)));
            }
        } else {
            self.messages.push(Message::system("No active thread to rename"));
        }
    }

    /// Request thread fork
    async fn request_fork(&mut self) {
        if let Some(thread_id) = &self.thread_id {
            if let Some(tx) = &self.input_tx {
                self.request_counter += 1;
                self.pending_request_id = Some(self.request_counter);
                self.pending_request_type = PendingRequestType::ForkThread;
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
            self.messages.push(Message::system("No active thread to fork"));
        }
    }

    /// Request logout
    async fn request_logout(&mut self) {
        if let Some(tx) = &self.input_tx {
            self.request_counter += 1;
            self.pending_request_id = Some(self.request_counter);
            self.pending_request_type = PendingRequestType::Logout;
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

    /// Request feedback upload
    async fn request_feedback(&mut self) {
        if let Some(tx) = &self.input_tx {
            self.request_counter += 1;
            self.pending_request_id = Some(self.request_counter);
            self.pending_request_type = PendingRequestType::FeedbackUpload;
            let msg = serde_json::json!({
                "jsonrpc": "2.0",
                "method": "feedback/upload",
                "id": self.request_counter,
                "params": {
                    "classification": "general",
                    "reason": null,
                    "threadId": self.thread_id,
                    "includeLogs": true
                }
            })
            .to_string();
            let _ = tx.send(msg).await;
            self.messages.push(Message::system("Uploading feedback..."));
        }
    }

    /// Show MCP tools - query the server for MCP server statuses
    async fn show_mcp_tools(&mut self) {
        if let Some(tx) = &self.input_tx {
            self.request_counter += 1;
            self.pending_request_id = Some(self.request_counter);
            self.pending_request_type = PendingRequestType::McpServerList;
            let msg = serde_json::json!({
                "jsonrpc": "2.0",
                "method": "mcpServerStatus/list",
                "id": self.request_counter,
                "params": {}
            })
            .to_string();
            let _ = tx.send(msg).await;
            self.messages.push(Message::system("Fetching MCP servers..."));
        }
    }

    /// Request apps list
    async fn request_apps_list(&mut self) {
        if let Some(tx) = &self.input_tx {
            self.request_counter += 1;
            self.pending_request_id = Some(self.request_counter);
            self.pending_request_type = PendingRequestType::AppsList;
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

    /// Open approvals picker - shows approval mode options
    async fn open_approvals_picker(&mut self) {
        self.picker_mode = PickerMode::Approvals;
        self.picker.title = "Approval Mode".to_string();
        let items = vec![
            PickerItem { id: "suggest".to_string(), title: "Suggest".to_string(), subtitle: "Approve everything".to_string(), metadata: None },
            PickerItem { id: "auto-edit".to_string(), title: "Auto Edit".to_string(), subtitle: "Auto-approve file edits".to_string(), metadata: None },
            PickerItem { id: "full-auto".to_string(), title: "Full Auto".to_string(), subtitle: "Auto-approve everything".to_string(), metadata: None },
        ];
        self.picker.open(items);
    }

    /// Open permissions picker - shows sandbox policy options  
    async fn open_permissions_picker(&mut self) {
        self.picker_mode = PickerMode::Permissions;
        self.picker.title = "Permissions".to_string();
        let items = vec![
            PickerItem { id: "read-only".to_string(), title: "Read Only".to_string(), subtitle: "Can only read files".to_string(), metadata: None },
            PickerItem { id: "workspace-write".to_string(), title: "Workspace Write".to_string(), subtitle: "Can write in workspace".to_string(), metadata: None },
            PickerItem { id: "full-access".to_string(), title: "Full Access".to_string(), subtitle: "Full system access".to_string(), metadata: None },
        ];
        self.picker.open(items);
    }

    /// Open personality picker
    async fn open_personality_picker(&mut self) {
        self.picker_mode = PickerMode::Personality;
        self.picker.title = "Personality".to_string();
        let items = vec![
            PickerItem { id: "friendly".to_string(), title: "Friendly".to_string(), subtitle: "Warm and encouraging".to_string(), metadata: None },
            PickerItem { id: "pragmatic".to_string(), title: "Pragmatic".to_string(), subtitle: "Direct and efficient".to_string(), metadata: None },
        ];
        self.picker.open(items);
    }

    /// Open experimental features picker - reads config first
    async fn open_experimental_picker(&mut self) {
        if let Some(tx) = &self.input_tx {
            self.request_counter += 1;
            self.pending_request_id = Some(self.request_counter);
            self.pending_request_type = PendingRequestType::ConfigRead;
            let msg = serde_json::json!({
                "jsonrpc": "2.0",
                "method": "config/read",
                "id": self.request_counter,
                "params": {}
            })
            .to_string();
            let _ = tx.send(msg).await;
            self.messages.push(Message::system("Fetching experimental features..."));
        }
    }

    /// Open collaboration mode picker
    async fn open_collab_picker(&mut self) {
        self.picker_mode = PickerMode::Collab;
        self.picker.title = "Collaboration Mode".to_string();
        self.picker.open_loading();

        if let Some(tx) = &self.input_tx {
            self.request_counter += 1;
            self.pending_request_id = Some(self.request_counter);
            self.pending_request_type = PendingRequestType::CollabModeList;
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

    /// Set plan mode directly
    async fn set_plan_mode(&mut self) {
        // Plan mode is a specific collaboration mode
        if let Some(tx) = &self.input_tx {
            self.request_counter += 1;
            let msg = serde_json::json!({
                "jsonrpc": "2.0",
                "method": "config/value/write",
                "id": self.request_counter,
                "params": {
                    "keyPath": "collaborationMode",
                    "value": "plan",
                    "mergeStrategy": "replace"
                }
            })
            .to_string();
            let _ = tx.send(msg).await;
            self.messages.push(Message::system("Switching to Plan mode..."));
        }
    }

    /// Open agent thread picker
    async fn open_agent_picker(&mut self) {
        self.picker_mode = PickerMode::Agent;
        self.picker.title = "Active Agents".to_string();
        self.picker.open_loading();

        if let Some(tx) = &self.input_tx {
            self.request_counter += 1;
            self.pending_request_id = Some(self.request_counter);
            self.pending_request_type = PendingRequestType::AgentThreadList;
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

    /// Execute /init command - creates AGENTS.md
    async fn execute_init(&mut self) {
        let init_target = std::path::Path::new(&self.cwd).join("AGENTS.md");
        if init_target.exists() {
            self.messages.push(Message::system(
                "AGENTS.md already exists. Skipping /init to avoid overwriting it."
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

        let msg = self.create_turn_message(INIT_PROMPT);
        if let Some(tx) = &self.input_tx {
            let _ = tx.send(msg).await;
            self.messages.push(Message::user("/init"));
            self.messages.push(Message::system("Creating AGENTS.md..."));
            self.is_processing = true;
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
            self.pending_request_id = Some(request_id);
            self.pending_request_type = PendingRequestType::ThreadList;

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

            match self.picker_mode {
                PickerMode::Resume => {
                    self.messages.push(Message::system(&format!(
                        "Resuming session: {}",
                        item_title
                    )));
                    if let Some(tx) = &self.input_tx {
                        self.request_counter += 1;
                        self.pending_request_id = Some(self.request_counter);
                        self.pending_request_type = PendingRequestType::ThreadResume(item_id.clone());
                        // Build params with both threadId and path (if available).
                        // The path takes precedence in the app-server, bypassing
                        // the potentially unreliable UUID-based file search.
                        let mut params = serde_json::json!({ "threadId": item_id });
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
                PickerMode::Model => {
                    self.messages.push(Message::system(&format!("Model set to: {}", item_title)));
                    self.write_config("model", &serde_json::json!(item_id)).await;
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
                PickerMode::Approvals => {
                    self.messages.push(Message::system(&format!("Approval mode: {}", item_title)));
                    self.write_config("approvalPolicy", &serde_json::json!(item_id)).await;
                }
                PickerMode::Permissions => {
                    self.messages.push(Message::system(&format!("Permissions: {}", item_title)));
                    self.write_config("sandboxPolicy", &serde_json::json!(item_id)).await;
                }
                PickerMode::Personality => {
                    self.messages.push(Message::system(&format!("Personality: {}", item_title)));
                    self.write_config("personality", &serde_json::json!(item_id)).await;
                }
                PickerMode::Collab => {
                    self.messages.push(Message::system(&format!("Collaboration mode: {}", item_title)));
                    self.write_config("collaborationMode", &serde_json::json!(item_id)).await;
                }
                PickerMode::Agent => {
                    self.messages.push(Message::system(&format!("Switched to agent: {}", item_title)));
                    self.thread_id = Some(item_id);
                }
                PickerMode::None => {}
            }
        }

        self.picker.close();
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
    async fn respond_to_approval_with_amendment(&mut self, approval: &PendingApproval, amendment: &[String]) {
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

    fn execute_gugugaga_command(&mut self, cmd: GugugagaCommand, args: String) {
        match cmd {
            GugugagaCommand::Help => {
                self.messages.push(Message::system("Gugugaga commands (//):"));
                for c in GugugagaCommand::all() {
                    self.messages.push(Message::system(&format!(
                        "  //{:<12} - {}",
                        c.name(),
                        c.description()
                    )));
                }
                self.messages.push(Message::system("\nCodex commands (/):"));
                self.messages.push(Message::system("  /model, /resume, /new, /status, /diff, etc."));
                self.messages.push(Message::system("  Type / and press Tab for full list."));
            }
            GugugagaCommand::Clear => {
                self.messages.clear();
                self.messages.push(Message::system("Chat history cleared."));
            }
            GugugagaCommand::Stats => {
                self.messages.push(Message::system(&format!(
                    "Session stats:\n  Violations: {}\n  Corrections: {}\n  Auto-replies: {}",
                    self.violations_detected, self.corrections_made, self.auto_replies
                )));
            }
            GugugagaCommand::Rules => {
                self.messages
                    .push(Message::system("Current rules: (loaded from memory.md)"));
                // TODO: Load and display actual rules from PersistentMemory
            }
            GugugagaCommand::Instruct => {
                if args.is_empty() {
                    self.messages
                        .push(Message::system("Usage: //instruct <instruction>"));
                } else {
                    self.messages.push(Message::system(&format!(
                        "Added instruction: {}",
                        args
                    )));
                    // TODO: Add to PersistentMemory
                }
            }
            GugugagaCommand::Task => {
                if args.is_empty() {
                    self.messages
                        .push(Message::system("Usage: //task <task description>"));
                } else {
                    self.messages
                        .push(Message::system(&format!("Task set: {}", args)));
                    // TODO: Add to PersistentMemory
                }
            }
            GugugagaCommand::Violations => {
                self.messages.push(Message::system(&format!(
                    "Total violations detected: {}",
                    self.violations_detected
                )));
            }
            GugugagaCommand::Pause => {
                self.is_paused = true;
                self.messages
                    .push(Message::system("Gugugaga monitoring paused."));
            }
            GugugagaCommand::Unpause => {
                self.is_paused = false;
                self.messages
                    .push(Message::system("Gugugaga monitoring resumed."));
            }
            GugugagaCommand::Save => {
                self.messages.push(Message::system("Memory saved to disk."));
                // TODO: Trigger memory save
            }
            GugugagaCommand::Quit => {
                self.should_quit = true;
            }
        }
        self.scroll_to_bottom();
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
            self.handle_output_message(&msg);
        }
    }

    fn handle_output_message(&mut self, msg: &str) {
        // Debug: show raw message method for troubleshooting
        if let Ok(json) = serde_json::from_str::<serde_json::Value>(msg) {
            if let Some(method) = json.get("method").and_then(|m| m.as_str()) {
                // Show important events
                if method.contains("error") || method.contains("Error") {
                    let preview = truncate_utf8(msg, 200);
                    self.messages.push(Message::system(&format!("[{}] {}", method, preview)));
                }
            }
        }

        if let Ok(json) = serde_json::from_str::<serde_json::Value>(msg) {
            // Check if this is a server-initiated REQUEST (has both "id" and "method")
            // These are approval requests that need our response
            if let (Some(id), Some(method)) = (json.get("id").and_then(|i| i.as_u64()), json.get("method").and_then(|m| m.as_str())) {
                match method {
                    "item/commandExecution/requestApproval" => {
                        // Command execution approval request
                        let params = json.get("params").cloned().unwrap_or_default();
                        let command = params.get("command").and_then(|c| c.as_str()).map(String::from);
                        let cwd = params.get("cwd").and_then(|c| c.as_str()).map(String::from);
                        let reason = params.get("reason").and_then(|r| r.as_str()).map(String::from);
                        // Extract proposed execpolicy amendment (array of command prefix strings)
                        let proposed_amendment = params.get("proposedExecpolicyAmendment")
                            .and_then(|v| v.as_array())
                            .map(|arr| arr.iter().filter_map(|s| s.as_str().map(String::from)).collect::<Vec<_>>());
                        
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
                        let reason = params.get("reason").and_then(|r| r.as_str()).map(String::from);
                        
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
                self.thread_id = Some(tid.to_string());
            }

            // Check if this is a JSON-RPC response (has "id" and "result" or "error")
            if let Some(id) = json.get("id") {
                // This is a response to a request we made
                if let Some(req_id) = id.as_u64() {
                    if self.pending_request_id == Some(req_id) {
                        self.pending_request_id = None;
                        self.handle_rpc_response(&json);
                        return;
                    }
                }
                // Check for error responses (only for non-pending requests)
                if let Some(error) = json.get("error") {
                    let error_msg = error
                        .get("message")
                        .and_then(|m| m.as_str())
                        .unwrap_or("Unknown error");
                    // Only show error if it seems relevant (not from init)
                    if self.pending_request_id.is_some() {
                        self.messages.push(Message::system(&format!("Error: {}", error_msg)));
                        self.is_processing = false;
                        self.pending_request_id = None;
                        self.pending_request_type = PendingRequestType::None;
                        // Close picker if it was open
                        if self.picker.visible {
                            self.picker.close();
                            self.picker_mode = PickerMode::None;
                        }
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
                        self.is_processing = true;
                        if let Some(last) = self.messages.last_mut() {
                            if last.role == MessageRole::Codex {
                                last.content.push_str(delta);
                                return;
                            }
                        }
                        self.messages.push(Message::codex(delta));
                    }
                }
                "item/agentReasoning/delta" | "item/agentReasoning/summaryDelta" => {
                    // Reasoning/thinking delta - show as thinking message
                    if let Some(delta) = json
                        .get("params")
                        .and_then(|p| p.get("delta"))
                        .and_then(|t| t.as_str())
                    {
                        self.is_processing = true;
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
                        self.is_processing = true;
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
                        self.is_processing = true;
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
                        self.is_processing = true;
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
                    if let Some(delta) = json
                        .get("params")
                        .and_then(|p| p.get("delta"))
                        .and_then(|t| t.as_str())
                    {
                        self.is_processing = true;
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
                                if lines.len() > MAX_OUTPUT_LINES || last.content.len() > MAX_OUTPUT_CHARS {
                                    let truncated: String = lines.iter()
                                        .take(MAX_OUTPUT_LINES)
                                        .map(|s| *s)
                                        .collect::<Vec<_>>()
                                        .join("\n");
                                    let truncated = if truncated.len() > MAX_OUTPUT_CHARS {
                                        format!("{}...", truncate_utf8(&truncated, MAX_OUTPUT_CHARS))
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
                        self.is_processing = true;
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
                                if m.role == MessageRole::FileChange && m.content.starts_with(marker) {
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
                        let item_type = item.get("type").and_then(|t| t.as_str()).unwrap_or("unknown");
                        match item_type {
                            "commandExecution" => {
                                let cmd = item.get("command").and_then(|c| c.as_str()).unwrap_or("command");
                                self.messages.push(Message::command_exec(format!("$ {}", cmd)));
                            }
                            "fileChange" => {
                                if let Some(changes) = item.get("changes").and_then(|c| c.as_array()) {
                                    let item_id = item.get("id").and_then(|i| i.as_str()).unwrap_or("");
                                    let mut full_diff = String::new();
                                    for change in changes {
                                        let path = change.get("path").and_then(|p| p.as_str()).unwrap_or("file");
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
                                        if let Some(diff) = change.get("diff").and_then(|d| d.as_str()) {
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
                                        let content = format!("{}\n{}", marker, full_diff.trim_end());
                                        self.messages.push(Message::file_change(&content));
                                    }
                                }
                            }
                            "contextCompaction" => {
                                self.messages.push(Message::system("Context compaction in progress..."));
                            }
                            "webSearch" => {
                                let query = item.get("query").and_then(|q| q.as_str()).unwrap_or("...");
                                self.messages.push(Message::system(&format!("🔍 Searching: {}", query)));
                            }
                            "enteredReviewMode" => {
                                let review = item.get("review").and_then(|r| r.as_str()).unwrap_or("changes");
                                self.messages.push(Message::system(&format!("📋 Reviewing: {}", review)));
                            }
                            "collabAgentToolCall" => {
                                let tool = item.get("details")
                                    .and_then(|d| d.get("tool"))
                                    .and_then(|t| t.as_str())
                                    .unwrap_or("unknown");
                                let sender = item.get("details")
                                    .and_then(|d| d.get("senderThreadId"))
                                    .and_then(|s| s.as_str())
                                    .unwrap_or("?");
                                match tool {
                                    "spawnAgent" => {
                                        let prompt = item.get("details")
                                            .and_then(|d| d.get("prompt"))
                                            .and_then(|p| p.as_str())
                                            .unwrap_or("");
                                        let preview = if prompt.len() > 80 {
                                            format!("{}...", truncate_utf8(&prompt, 80))
                                        } else {
                                            prompt.to_string()
                                        };
                                        self.messages.push(Message::system(&format!(
                                            "🔀 Spawning sub-agent: {}", preview
                                        )));
                                    }
                                    "sendInput" => {
                                        self.messages.push(Message::system(&format!(
                                            "📨 Sending input to agent (from {})", sender
                                        )));
                                    }
                                    "wait" => {
                                        self.messages.push(Message::system("⏳ Waiting for sub-agent..."));
                                    }
                                    "closeAgent" => {
                                        self.messages.push(Message::system("🔚 Closing sub-agent"));
                                    }
                                    _ => {
                                        self.messages.push(Message::system(&format!(
                                            "🤖 Collab: {} (from {})", tool, sender
                                        )));
                                    }
                                }
                            }
                            _ => {}
                        }
                        self.is_processing = true;
                    }
                }
                "item/completed" => {
                    // Item lifecycle complete
                    if let Some(item) = json.get("params").and_then(|p| p.get("item")) {
                        let item_type = item.get("type").and_then(|t| t.as_str()).unwrap_or("unknown");
                        match item_type {
                            "commandExecution" => {
                                let exit_code = item.get("exitCode").and_then(|e| e.as_i64());
                                let duration = item.get("durationMs").and_then(|d| d.as_i64());

                                let dur_str = duration.map(|d| format!(" • {}ms", d)).unwrap_or_default();
                                let status_line = if exit_code.unwrap_or(0) == 0 {
                                    format!("\n\u{2713}{}", dur_str)
                                } else {
                                    format!("\n\u{2717} ({}){}", exit_code.unwrap_or(-1), dur_str)
                                };

                                // Update the last in-progress CommandExec message
                                let updated = self.messages.iter_mut().rev()
                                    .find(|m| m.role == MessageRole::CommandExec
                                        && !m.content.contains('\u{2713}')
                                        && !m.content.contains('\u{2717}'))
                                    .map(|m| {
                                        m.content.push_str(&status_line);
                                        true
                                    })
                                    .unwrap_or(false);

                                if !updated {
                                    // Fallback: separate system message
                                    let code_str = exit_code.map(|c| format!(" (exit {})", c)).unwrap_or_default();
                                    let dur = duration.map(|d| format!(" in {}ms", d)).unwrap_or_default();
                                    self.messages.push(Message::system(
                                        &format!("Command completed{}{}", code_str, dur)
                                    ));
                                }
                            }
                            "fileChange" => {
                                let item_id = item.get("id").and_then(|i| i.as_str()).unwrap_or("");
                                let status = item.get("status").and_then(|s| s.as_str()).unwrap_or("completed");
                                // Build the final diff from completed changes.
                                if let Some(changes) = item.get("changes").and_then(|c| c.as_array()) {
                                    let mut full_diff = String::new();
                                    for change in changes {
                                        let path = change.get("path").and_then(|p| p.as_str()).unwrap_or("file");
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
                                        if let Some(diff) = change.get("diff").and_then(|d| d.as_str()) {
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
                                        let new_content = format!("{}\n{}", marker, full_diff.trim_end());
                                        // Replace the in-progress message if it exists.
                                        let replaced = self.messages.iter_mut().rev().any(|m| {
                                            if m.role == MessageRole::FileChange && m.content.starts_with(&marker) {
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
                                self.messages.push(Message::system(&format!("{} File change {}", icon, status)));
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
                                let tool = item.get("details")
                                    .and_then(|d| d.get("tool"))
                                    .and_then(|t| t.as_str())
                                    .unwrap_or("unknown");
                                let status = item.get("details")
                                    .and_then(|d| d.get("status"))
                                    .and_then(|s| s.as_str())
                                    .unwrap_or("completed");
                                // Show agent states if available
                                if let Some(agents) = item.get("details")
                                    .and_then(|d| d.get("agentsStates"))
                                    .and_then(|a| a.as_object())
                                {
                                    for (agent_id, state) in agents {
                                        let agent_status = state.get("status")
                                            .and_then(|s| s.as_str())
                                            .unwrap_or("unknown");
                                        let msg = state.get("message")
                                            .and_then(|m| m.as_str());
                                        let icon = match agent_status {
                                            "completed" => "✅",
                                            "running" => "🔄",
                                            "errored" => "❌",
                                            "shutdown" => "⏹️",
                                            _ => "🤖"
                                        };
                                        let short_id = if agent_id.len() > 8 {
                                            truncate_utf8(&agent_id, 8)
                                        } else {
                                            agent_id
                                        };
                                        if let Some(msg) = msg {
                                            let preview = if msg.len() > 100 {
                                                format!("{}...", truncate_utf8(&msg, 100))
                                            } else {
                                                msg.to_string()
                                            };
                                            self.messages.push(Message::system(&format!(
                                                "{} Agent {}.. {}: {}", icon, short_id, agent_status, preview
                                            )));
                                        } else {
                                            self.messages.push(Message::system(&format!(
                                                "{} Agent {}.. {}", icon, short_id, agent_status
                                            )));
                                        }
                                    }
                                } else {
                                    self.messages.push(Message::system(&format!(
                                        "🤖 Collab {} {}", tool, status
                                    )));
                                }
                            }
                            _ => {}
                        }
                    }
                }
                "turn/plan/updated" => {
                    // Plan update notification
                    if let Some(explanation) = json.get("params").and_then(|p| p.get("explanation")).and_then(|e| e.as_str()) {
                        self.messages.push(Message::system(&format!("📋 Plan: {}", explanation)));
                    }
                    if let Some(plan) = json.get("params").and_then(|p| p.get("plan")).and_then(|p| p.as_array()) {
                        for (i, step) in plan.iter().enumerate() {
                            let step_text = step.get("step").and_then(|s| s.as_str()).unwrap_or("step");
                            let status = step.get("status").and_then(|s| s.as_str()).unwrap_or("pending");
                            let icon = match status {
                                "completed" => "✓",
                                "inProgress" => "→",
                                _ => "○"
                            };
                            self.messages.push(Message::system(&format!("  {} {}. {}", icon, i + 1, step_text)));
                        }
                    }
                }
                // "turn/diff/updated" handled above (near item/fileChange/outputDelta)
                "turn/started" => {
                    self.is_processing = true;
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
                                "Turn interrupted — tell the model what to do differently."
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
                            self.messages.push(Message::system(
                                &format!("Turn failed: {}", error_msg)
                            ));
                        }
                        _ => {} // "completed" — normal
                    }

                    self.is_processing = false;
                    self.current_turn_id = None;
                    self.scroll_to_bottom();
                }
                "thread/started" => {
                    // Extract thread ID from notification
                    if let Some(thread) = json
                        .get("params")
                        .and_then(|p| p.get("thread"))
                    {
                        if let Some(id) = thread.get("id").and_then(|i| i.as_str()) {
                            self.thread_id = Some(id.to_string());
                            self.messages.push(Message::system("Session started. Ready to chat!"));
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
                        self.messages.push(Message::system(&format!("⚠️ violations: {}", text)));
                        self.scroll_to_bottom();
                    }
                }
                "gugugaga/thinking" => {
                    // Update status bar instead of pushing inline messages (aligned with Codex)
                    if let Some(msg) = json
                        .get("params")
                        .and_then(|p| p.get("message"))
                        .and_then(|m| m.as_str())
                    {
                        self.gugugaga_status = Some(msg.to_string());
                    }
                }
                "gugugaga/check" => {
                    // Clear gugugaga status (thinking is done)
                    self.gugugaga_status = None;

                    // Show supervision check result with Markdown support
                    if let Some(status) = json
                        .get("params")
                        .and_then(|p| p.get("status"))
                        .and_then(|s| s.as_str())
                    {

                        let msg = json
                            .get("params")
                            .and_then(|p| p.get("message"))
                            .and_then(|m| m.as_str())
                            .unwrap_or("");
                        
                        // Use Gugugaga role for Markdown rendering
                        match status {
                            "ok" => self.messages.push(Message::gugugaga(&format!("🛡️ {}", msg))),
                            "violation" => {
                                self.violations_detected += 1;
                                self.current_turn_violations += 1;
                                self.messages.push(Message::gugugaga(msg));
                            }
                            "error" => self.messages.push(Message::gugugaga(msg)),
                            _ => self.messages.push(Message::gugugaga(&format!("🛡️ {}", msg))),
                        }
                        self.scroll_to_bottom();
                    }
                }
                "gugugaga/status" => {
                    // Show gugugaga status
                    if let Some(text) = json
                        .get("params")
                        .and_then(|p| p.get("message"))
                        .and_then(|t| t.as_str())
                    {
                        let strict = json
                            .get("params")
                            .and_then(|p| p.get("strictMode"))
                            .and_then(|s| s.as_bool())
                            .unwrap_or(false);
                        let mode = if strict { " (strict mode)" } else { "" };
                        self.messages.push(Message::system(&format!("🛡️ {}{}", text, mode)));
                        self.scroll_to_bottom();
                    }
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
                        self.messages.push(Message::system(&format!("Error: {}", msg)));
                        self.is_processing = false;
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
        let request_type = std::mem::replace(&mut self.pending_request_type, PendingRequestType::None);
        
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
                                let rollout_path = thread.get("path").and_then(|p| p.as_str()).map(|s| s.to_string());
                                
                                // Filter by cwd - only show sessions from current directory
                                let thread_cwd = thread.get("cwd").and_then(|c| c.as_str()).unwrap_or("");
                                if !thread_cwd.is_empty() && thread_cwd != current_cwd {
                                    return None;
                                }
                                
                                // Use preview as title (first user message), fallback to id
                                let preview = thread
                                    .get("preview")
                                    .and_then(|p| p.as_str())
                                    .unwrap_or("");
                                let title = if preview.is_empty() {
                                    format!("Session {}", &id[..8.min(id.len())])
                                } else {
                                    // Truncate long previews
                                    let max_len = 40;
                                    if preview.len() > max_len {
                                        format!("{}...", &preview[..max_len])
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
                                    let datetime = UNIX_EPOCH + Duration::from_secs(created_at as u64);
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
                            self.messages
                                .push(Message::system("No saved sessions found for this directory."));
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
                    self.messages.push(Message::system(&format!(
                        "Failed to load sessions: {}",
                        error_msg
                    )));
                }
            }
            PendingRequestType::ThreadResume(_thread_id) => {
                // Handle thread/resume response
                if let Some(result) = json.get("result") {
                    // Extract thread ID from response and update our state
                    if let Some(thread) = result.get("thread") {
                        if let Some(id) = thread.get("id").and_then(|i| i.as_str()) {
                            self.thread_id = Some(id.to_string());
                            // Display history directly from the resume response.
                            // thread/resume already includes turns — no need for a
                            // separate thread/read call (which would also hit the
                            // unreliable UUID file search).
                            if let Some(turns) = thread.get("turns").and_then(|t| t.as_array()) {
                                if !turns.is_empty() {
                                    self.display_turns(turns);
                                    self.scroll_to_bottom();
                                }
                            }
                        }
                    }
                } else if let Some(error) = json.get("error") {
                    let error_msg = error
                        .get("message")
                        .and_then(|m| m.as_str())
                        .unwrap_or("Unknown error");
                    self.messages.push(Message::system(&format!(
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
            PendingRequestType::ModelList => {
                if let Some(result) = json.get("result") {
                    if let Some(models) = result.get("data").and_then(|m| m.as_array()) {
                        let items: Vec<PickerItem> = models
                            .iter()
                            .filter_map(|m| {
                                let id = m.get("id").and_then(|i| i.as_str())?.to_string();
                                let name = m.get("displayName").and_then(|n| n.as_str())
                                    .or_else(|| m.get("model").and_then(|n| n.as_str()))
                                    .unwrap_or(&id)
                                    .to_string();
                                let provider = m.get("modelProvider").and_then(|p| p.as_str()).unwrap_or("");
                                Some(PickerItem {
                                    id,
                                    title: name,
                                    subtitle: provider.to_string(),
                                    metadata: None,
                                })
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
                        self.picker.close();
                        self.picker_mode = PickerMode::None;
                        let text = serde_json::to_string_pretty(result).unwrap_or_default();
                        self.messages.push(Message::system(&format!("Models:\n{}", text)));
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
                                    let name = skill.get("name").and_then(|n| n.as_str()).unwrap_or("unnamed").to_string();
                                    let desc = skill.get("description").and_then(|d| d.as_str()).unwrap_or("").to_string();
                                    let enabled = skill.get("enabled").and_then(|e| e.as_bool()).unwrap_or(false);
                                    let path = skill.get("path").and_then(|p| p.as_str()).unwrap_or("").to_string();
                                    all_skills.push((name, desc, enabled, path));
                                }
                            }
                            if let Some(errs) = entry.get("errors").and_then(|e| e.as_array()) {
                                for err in errs {
                                    // SkillErrorInfo has path and message fields
                                    let path = err.get("path").and_then(|p| p.as_str()).unwrap_or("");
                                    let msg = err.get("message").and_then(|m| m.as_str())
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
                            let enabled_skills: Vec<&(String, String, bool, String)> = all_skills.iter()
                                .filter(|(_, _, enabled, _)| *enabled)
                                .collect();

                            if enabled_skills.is_empty() {
                                self.picker.close();
                                self.picker_mode = PickerMode::None;
                                self.messages.push(Message::system("No enabled skills found."));
                            } else {
                                let items: Vec<PickerItem> = enabled_skills.iter().map(|(name, desc, _, _)| {
                                    PickerItem {
                                        id: name.clone(),
                                        title: name.clone(),
                                        subtitle: desc.clone(),
                                        metadata: None,
                                    }
                                }).collect();
                                self.picker.set_items(items);
                            }
                        }
                        PickerMode::SkillsManage => {
                            // Show all skills in picker for toggling
                            if all_skills.is_empty() {
                                self.picker.close();
                                self.picker_mode = PickerMode::None;
                                self.messages.push(Message::system("No skills found to manage."));
                            } else {
                                let items: Vec<PickerItem> = all_skills.iter().map(|(name, desc, enabled, path)| {
                                    let status = if *enabled { "✓" } else { "✗" };
                                    let prefix = if *enabled { "enabled" } else { "disabled" };
                                    PickerItem {
                                        // Use path as ID since skills/config/write requires path
                                        id: format!("{}:{}", prefix, path),
                                        title: format!("{} {}", status, name),
                                        subtitle: desc.clone(),
                                        metadata: None,
                                    }
                                }).collect();
                                self.picker.set_items(items);
                            }
                        }
                        _ => {
                            // Fallback: display as text
                            self.picker.close();
                            self.picker_mode = PickerMode::None;

                            if all_skills.is_empty() && errors.is_empty() {
                                self.messages.push(Message::system("No skills found. Add skills via AGENTS.md or ~/.codex/skills/"));
                            } else {
                                let mut lines = Vec::new();
                                for (name, desc, enabled, _path) in &all_skills {
                                    let status = if *enabled { "✓" } else { "✗" };
                                    lines.push(format!("  {} {} — {}", status, name, desc));
                                }
                                for err in &errors {
                                    lines.push(format!("  ⚠ {}", err));
                                }
                                self.messages.push(Message::system(&format!("Skills:\n{}", lines.join("\n"))));
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
                            self.messages.push(Message::system("No collaboration modes available."));
                        } else {
                            self.picker.set_items(items);
                        }
                    } else {
                        self.picker.close();
                        self.picker_mode = PickerMode::None;
                        let text = serde_json::to_string_pretty(result).unwrap_or_default();
                        self.messages.push(Message::system(&format!("Collaboration modes:\n{}", text)));
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
                                let short_id = if id.len() > 12 { truncate_utf8(&id, 12) } else { &id };
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
                            self.messages.push(Message::system("No active agent threads."));
                        } else {
                            self.picker.set_items(items);
                        }
                    } else {
                        self.picker.close();
                        self.picker_mode = PickerMode::None;
                        let text = serde_json::to_string_pretty(result).unwrap_or_default();
                        self.messages.push(Message::system(&format!("Agent threads:\n{}", text)));
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
                            let name = server.get("name").and_then(|n| n.as_str()).unwrap_or("unnamed");
                            let auth_status = server.get("authStatus").and_then(|s| s.as_str()).unwrap_or("unknown");
                            // tools is a HashMap<String, McpTool>, not an array
                            let tool_count = server.get("tools").and_then(|t| t.as_object()).map(|o| o.len()).unwrap_or(0);
                            lines.push(format!("  {} [{}] - {} tools", name, auth_status, tool_count));

                            if let Some(tools) = server.get("tools").and_then(|t| t.as_object()) {
                                for (tool_name, _tool_info) in tools {
                                    lines.push(format!("    - {}", tool_name));
                                }
                            }
                        }
                    }

                    if lines.is_empty() {
                        self.messages.push(Message::system("MCP: No servers configured."));
                    } else {
                        self.messages.push(Message::system(&format!("MCP Servers:\n{}", lines.join("\n"))));
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
                            let name = app.get("name").and_then(|n| n.as_str()).unwrap_or("unnamed");
                            let desc = app.get("description").and_then(|d| d.as_str()).unwrap_or("");
                            let accessible = app.get("isAccessible").and_then(|a| a.as_bool()).unwrap_or(false);
                            let status = if accessible { "✓" } else { "✗" };
                            lines.push(format!("  {} {} — {}", status, name, desc));
                        }
                    }

                    if lines.is_empty() {
                        self.messages.push(Message::system("No apps configured."));
                    } else {
                        self.messages.push(Message::system(&format!("Apps:\n{}", lines.join("\n"))));
                    }
                } else {
                    self.handle_rpc_error(json, "Failed to load apps");
                }
            }
            PendingRequestType::ConfigRead => {
                if let Some(result) = json.get("result") {
                    let text = serde_json::to_string_pretty(result).unwrap_or_default();
                    self.messages.push(Message::system(&format!("Current config:\n{}", text)));
                } else {
                    self.handle_rpc_error(json, "Failed to read config");
                }
            }
            PendingRequestType::FeedbackUpload => {
                if json.get("result").is_some() {
                    self.messages.push(Message::system("Feedback uploaded successfully. Thank you!"));
                } else {
                    self.handle_rpc_error(json, "Failed to upload feedback");
                }
            }
            PendingRequestType::NewThread => {
                if let Some(result) = json.get("result") {
                    if let Some(thread_id) = result.get("thread").and_then(|t| t.get("id")).and_then(|i| i.as_str()) {
                        self.thread_id = Some(thread_id.to_string());
                        self.messages.push(Message::system("New session ready."));
                    }
                } else {
                    self.handle_rpc_error(json, "Failed to start new session");
                }
            }
            PendingRequestType::ForkThread => {
                if let Some(result) = json.get("result") {
                    if let Some(thread_id) = result.get("thread").and_then(|t| t.get("id")).and_then(|i| i.as_str()) {
                        self.thread_id = Some(thread_id.to_string());
                        self.messages.push(Message::system("Session forked successfully."));
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
                    self.messages.push(Message::system("Logged out. Exiting..."));
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

    /// Helper to display RPC error messages
    fn handle_rpc_error(&mut self, json: &serde_json::Value, fallback: &str) {
        if let Some(error) = json.get("error") {
            let error_msg = error
                .get("message")
                .and_then(|m| m.as_str())
                .unwrap_or(fallback);
            self.messages.push(Message::system(&format!("Error: {}", error_msg)));
        } else {
            self.messages.push(Message::system(fallback));
        }
    }
    
    fn request_thread_read(&mut self, thread_id: String) {
        if let Some(tx) = &self.input_tx {
            self.request_counter += 1;
            self.pending_request_id = Some(self.request_counter);
            self.pending_request_type = PendingRequestType::ThreadRead(thread_id.clone());
            
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
                                if let Some(content) = item.get("content").and_then(|c| c.as_array()) {
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
                            let output = item.get("aggregatedOutput").and_then(|o| o.as_str()).unwrap_or("");

                            let mut msg = format!("$ {}", cmd);

                            if !output.is_empty() {
                                let max_output = 2000;
                                if output.len() > max_output {
                                    msg.push_str(&format!("\n{}...\n... (output truncated)", &output[..max_output]));
                                } else {
                                    msg.push_str(&format!("\n{}", output));
                                }
                            }

                            // Status line matching new renderer format
                            let dur_str = duration.map(|d| format!(" \u{2022} {}ms", d)).unwrap_or_default();
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
                                    let path = change.get("path")
                                        .and_then(|p| p.as_str())
                                        .unwrap_or("unknown");
                                    let kind = change.get("kind")
                                        .and_then(|k| k.get("type"))
                                        .and_then(|t| t.as_str())
                                        .unwrap_or("update");
                                    let verb = match kind {
                                        "add" => "Added",
                                        "delete" => "Deleted",
                                        _ => "Edited",
                                    };
                                    full_diff.push_str(&format!("\u{2022} {} {}\n", verb, path));
                                    if let Some(diff) = change.get("diff").and_then(|d| d.as_str()) {
                                        if !diff.trim().is_empty() {
                                            full_diff.push_str(diff);
                                            if !diff.ends_with('\n') {
                                                full_diff.push('\n');
                                            }
                                        }
                                    }
                                }
                                if !full_diff.is_empty() {
                                    self.messages.push(Message::file_change(full_diff.trim_end()));
                                }
                            }
                        }
                        "plan" => {
                            if let Some(text) = item.get("text").and_then(|t| t.as_str()) {
                                if !text.is_empty() {
                                    self.messages.push(Message::system(&format!("Plan: {}", text)));
                                }
                            }
                        }
                        _ => {}
                    }
                }
            }
        }
    }

    fn create_turn_message(&mut self, text: &str) -> String {
        self.request_counter += 1;
        let thread_id = self.thread_id.as_deref().unwrap_or("main");
        serde_json::json!({
            "jsonrpc": "2.0",
            "method": "turn/start",
            "id": self.request_counter,
            "params": {
                "threadId": thread_id,
                "input": [{
                    "type": "text",
                    "text": text,
                    "textElements": []
                }]
            }
        })
        .to_string()
    }

    /// Send turn/interrupt RPC to cancel the current turn
    async fn send_turn_interrupt(&mut self) {
        if let (Some(ref turn_id), Some(ref tx)) = (&self.current_turn_id, &self.input_tx) {
            let thread_id = self.thread_id.as_deref().unwrap_or("main");
            self.request_counter += 1;
            let msg = serde_json::json!({
                "jsonrpc": "2.0",
                "method": "turn/interrupt",
                "id": self.request_counter,
                "params": {
                    "threadId": thread_id,
                    "turnId": turn_id
                }
            })
            .to_string();
            let _ = tx.send(msg).await;
        }
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
        let violations = self.violations_detected;
        let corrections = self.corrections_made;
        let _auto_replies = self.auto_replies;
        let slash_popup = &self.slash_popup;
        let is_paused = self.is_paused;
        let picker = &self.picker;
        let pending_approval = &self.pending_approval;
        let approval_scroll = self.approval_scroll;
        let notebook_current_activity = &self.notebook_current_activity;
        let notebook_completed_count = &self.notebook_completed_count;
        let notebook_attention_items = &self.notebook_attention_items;
        let notebook_mistakes_count = &self.notebook_mistakes_count;
        let gugugaga_status = &self.gugugaga_status;
        let sel_anchor = self.sel_anchor;
        let sel_end = self.sel_end;

        // These will be filled by the draw closure and written back after
        let mut captured_lines: Vec<String> = Vec::new();
        let mut captured_rect = Rect::default();

        self.terminal.draw(|f| {
            let size = f.area();

            // Main layout: header, content, input, help
            let main_chunks = Layout::default()
                .direction(Direction::Vertical)
                .constraints([
                    Constraint::Length(1), // Header
                    Constraint::Length(1), // Status
                    Constraint::Min(8),    // Content
                    Constraint::Length(4), // Input
                    Constraint::Length(1), // Help
                ])
                .split(size);

            // Header
            let header = HeaderBar {
                title: if is_paused {
                    "Gugugaga [PAUSED]"
                } else {
                    "Gugugaga"
                },
                project: project_name,
                is_processing,
                spinner_frame,
            };
            f.render_widget(header, main_chunks[0]);

            // Status bar
            let status = StatusBar {
                is_processing: is_processing || gugugaga_status.is_some(),
                spinner_frame,
                status_text: if let Some(gs) = gugugaga_status {
                    // Gugugaga is thinking — show its status (like Codex's StatusIndicatorWidget)
                    format!("Supervising: {} (Esc to interrupt)", gs)
                } else if is_processing {
                    "Thinking... (Esc to interrupt)".to_string()
                } else if is_paused {
                    "Monitoring paused".to_string()
                } else {
                    String::new()
                },
            };
            f.render_widget(status, main_chunks[1]);

            // Content area: messages + stats
            let content_chunks = Layout::default()
                .direction(Direction::Horizontal)
                .constraints([Constraint::Min(30), Constraint::Length(28)])
                .split(main_chunks[2]);

            // Messages (with selection highlighting)
            let (lines_text, inner_rect) = Self::render_messages(f, content_chunks[0], messages, scroll_offset, sel_anchor, sel_end);
            captured_lines = lines_text;
            captured_rect = inner_rect;

            // Context panel (shows notebook state from cached data)
            let context_panel = ContextPanel {
                current_activity: notebook_current_activity.clone(),
                completed_count: *notebook_completed_count,
                attention_items: notebook_attention_items.clone(),
                mistakes_count: *notebook_mistakes_count,
                violations,
                corrections,
                is_monitoring: is_processing,
            };
            f.render_widget(context_panel, content_chunks[1]);

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

            // Set cursor position (use display width for proper CJK support)
            let cursor_x = main_chunks[3].x + 1 + input.cursor_display_width() as u16;
            let cursor_y = main_chunks[3].y + 1;
            f.set_cursor_position((
                cursor_x.min(main_chunks[3].x + main_chunks[3].width - 2),
                cursor_y,
            ));

            // Help bar
            if pending_approval.is_some() {
                // When approval overlay is shown, show minimal hint in help bar
                let hint = ratatui::widgets::Paragraph::new(
                    " Approval pending — see overlay above "
                ).style(ratatui::style::Style::default().fg(ratatui::style::Color::Yellow).bg(ratatui::style::Color::DarkGray));
                f.render_widget(hint, main_chunks[4]);
            } else {
                f.render_widget(HelpBar, main_chunks[4]);
            }

            // Render picker overlay (on top of everything)
            if picker.visible {
                picker.render(size, f.buffer_mut());
            }

            // Render approval overlay (on top of everything, highest z-order)
            if let Some(approval) = pending_approval {
                Self::render_approval_overlay(f, size, approval, approval_scroll);
            }
        })?;

        // Write back the captured data from the draw closure
        self.rendered_lines = captured_lines;
        self.msg_inner_rect = captured_rect;

        Ok(())
    }

    fn render_slash_popup(f: &mut Frame, input_area: Rect, popup: &SlashPopup) {
        let items = popup.display_items();
        let popup_height = (items.len() as u16 + 2).min(12);
        let popup_width = 50.min(input_area.width.saturating_sub(2));

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
        let lines: Vec<Line> = items
            .iter()
            .map(|(cmd, desc, selected)| {
                let prefix = if *selected { "▸ " } else { "  " };
                let style = if *selected {
                    Theme::accent()
                } else {
                    Theme::text()
                };
                Line::from(vec![
                    Span::styled(prefix, style),
                    Span::styled(cmd.clone(), style.bold()),
                    Span::styled(format!(" - {}", desc), Theme::muted()),
                ])
            })
            .collect();

        let title = if popup.is_gugugaga {
            " Gugugaga Commands (Tab) "
        } else {
            " Codex Commands (Tab) "
        };

        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(if popup.is_gugugaga {
                Theme::correction_badge()
            } else {
                Theme::accent()
            })
            .title_top(Line::styled(title, Theme::title()));

        let paragraph = Paragraph::new(lines)
            .block(block)
            .wrap(Wrap { trim: false });
        f.render_widget(paragraph, popup_area);
    }

    fn render_approval_overlay(f: &mut Frame, area: Rect, approval: &PendingApproval, scroll: usize) {
        use ratatui::style::{Color, Modifier, Style};
        use ratatui::text::{Line, Span};

        let opt_style = Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD);
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
                        Span::styled(
                            format!("Yes, don't ask again for `{}`", prefix),
                            desc_style,
                        ),
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
            Style::default().fg(Color::White).add_modifier(Modifier::BOLD),
        )));
        content_lines.push(Line::from(""));

        if let Some(ref reason) = approval.reason {
            content_lines.push(Line::from(vec![
                Span::styled("Reason: ", Style::default().fg(Color::Gray)),
                Span::styled(reason.as_str(), Style::default().fg(Color::White).add_modifier(Modifier::ITALIC)),
            ]));
            content_lines.push(Line::from(""));
        }

        match approval.approval_type {
            ApprovalType::CommandExecution => {
                let cmd = approval.command.as_deref().unwrap_or("(unknown)");
                content_lines.push(Line::from(vec![
                    Span::styled("  $ ", Style::default().fg(Color::Green).add_modifier(Modifier::BOLD)),
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
        let overlay_area = Rect { x, y, width: overlay_width, height: overlay_height };

        f.render_widget(Clear, overlay_area);

        // Outer border
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Yellow))
            .title_top(Line::styled(
                " ⚡ APPROVAL REQUIRED ",
                Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD),
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
            height: footer_height.min(inner.height.saturating_sub(content_height + sep_area.height)),
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

        let content_para = Paragraph::new(visible_content)
            .wrap(Wrap { trim: false });
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

    /// Render messages with optional selection highlight.
    /// Returns (rendered_line_texts, inner_rect) for mouse selection support.
    fn render_messages(
        f: &mut Frame,
        area: Rect,
        messages: &[Message],
        scroll_offset: usize,
        sel_anchor: Option<(u16, u16)>,
        sel_end: Option<(u16, u16)>,
    ) -> (Vec<String>, Rect) {
        use ratatui::style::{Color, Modifier};

        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Theme::border())
            .title_top(Line::styled(" Conversation ", Theme::muted()));

        let inner = block.inner(area);
        f.render_widget(block, area);

        // Build all lines with word wrapping
        let content_width = inner.width as usize;
        let mut all_lines: Vec<Line> = Vec::new();
        for msg in messages {
            all_lines.extend(render_message_lines(msg, content_width));
        }

        // Calculate scroll
        let total_lines = all_lines.len();
        let visible_height = inner.height as usize;
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

        // Extract plain text for each visible line (for clipboard copy)
        let line_texts: Vec<String> = visible
            .iter()
            .map(|line| {
                line.spans
                    .iter()
                    .map(|span| span.content.as_ref())
                    .collect::<String>()
            })
            .collect();

        // Apply selection highlight if there's an active selection
        let visible = if let (Some(anchor), Some(end)) = (sel_anchor, sel_end) {
            let (sel_start, sel_finish) = if anchor.0 < end.0 || (anchor.0 == end.0 && anchor.1 <= end.1) {
                (anchor, end)
            } else {
                (end, anchor)
            };

            visible
                .into_iter()
                .enumerate()
                .map(|(i, line)| {
                    let row = i as u16;
                    if row >= sel_start.0 && row <= sel_finish.0 {
                        // This line is (at least partially) selected
                        let col_start = if row == sel_start.0 { sel_start.1 as usize } else { 0 };
                        let col_end = if row == sel_finish.0 { sel_finish.1 as usize + 1 } else { usize::MAX };

                        // Rebuild spans with selection highlight
                        let mut new_spans = Vec::new();
                        let mut col = 0usize;
                        for span in &line.spans {
                            let span_len = span.content.chars().count();
                            let span_end = col + span_len;

                            if span_end <= col_start || col >= col_end {
                                // Entirely outside selection
                                new_spans.push(span.clone());
                            } else if col >= col_start && span_end <= col_end {
                                // Entirely inside selection
                                new_spans.push(Span::styled(
                                    span.content.clone(),
                                    span.style.bg(Color::White).fg(Color::Black).remove_modifier(Modifier::all()),
                                ));
                            } else {
                                // Partially selected — split the span
                                let chars: Vec<char> = span.content.chars().collect();
                                let local_start = col_start.saturating_sub(col);
                                let local_end = col_end.saturating_sub(col).min(span_len);

                                if local_start > 0 {
                                    let before: String = chars[..local_start].iter().collect();
                                    new_spans.push(Span::styled(before, span.style));
                                }
                                let selected: String = chars[local_start..local_end].iter().collect();
                                new_spans.push(Span::styled(
                                    selected,
                                    span.style.bg(Color::White).fg(Color::Black).remove_modifier(Modifier::all()),
                                ));
                                if local_end < span_len {
                                    let after: String = chars[local_end..].iter().collect();
                                    new_spans.push(Span::styled(after, span.style));
                                }
                            }
                            col = span_end;
                        }
                        Line::from(new_spans)
                    } else {
                        line
                    }
                })
                .collect::<Vec<_>>()
        } else {
            visible
        };

        let paragraph = Paragraph::new(visible)
            .wrap(Wrap { trim: false });
        f.render_widget(paragraph, inner);

        // Scrollbar
        if total_lines > visible_height {
            let scrollbar = Scrollbar::new(ScrollbarOrientation::VerticalRight)
                .begin_symbol(Some("▲"))
                .end_symbol(Some("▼"))
                .track_symbol(Some("│"))
                .thumb_symbol("█");

            let mut scrollbar_state =
                ScrollbarState::new(max_scroll).position(max_scroll.saturating_sub(actual_scroll));

            f.render_stateful_widget(
                scrollbar,
                area.inner(ratatui::layout::Margin {
                    vertical: 1,
                    horizontal: 0,
                }),
                &mut scrollbar_state,
            );
        }

        (line_texts, inner)
    }
}

impl Drop for App {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        let _ = execute!(
            self.terminal.backend_mut(),
            crossterm::event::DisableMouseCapture,
            LeaveAlternateScreen
        );
        let _ = self.terminal.show_cursor();
    }
}
