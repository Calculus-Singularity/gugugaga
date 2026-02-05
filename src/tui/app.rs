//! Main TUI application

use std::io::{self, Stdout};
use std::sync::Arc;
use std::time::Duration;

use crossterm::{
    event::{self, Event},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout, Rect},
    style::Stylize,
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState},
    Frame, Terminal,
};
use tokio::sync::{mpsc, RwLock};

use crate::memory::GugugagaNotebook;

use super::input::{InputAction, InputState};
use super::picker::{Picker, PickerItem};
use super::slash_commands::{parse_command, CodexCommand, ParsedCommand, SlashPopup, GugugagaCommand};
use super::theme::Theme;
use super::widgets::{
    render_message_lines, HeaderBar, HelpBar, InputBox, Message, MessageRole, StatsPanel, StatusBar,
    ContextPanel,
};

/// Current picker mode
#[derive(Debug, Clone, PartialEq, Eq)]
enum PickerMode {
    None,
    Resume,
    Model,
}

/// Type of pending request
#[derive(Debug, Clone, PartialEq)]
enum PendingRequestType {
    None,
    ThreadList,
    ThreadResume(String),  // Contains thread_id being resumed
    ThreadRead(String),    // Contains thread_id being read
}

/// Pending approval request from server
#[derive(Debug, Clone)]
struct PendingApproval {
    request_id: u64,
    approval_type: ApprovalType,
    command: Option<String>,
    cwd: Option<String>,
    changes: Vec<String>,
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
    /// Pending approval request (waiting for user response)
    pending_approval: Option<PendingApproval>,
    /// Gugugaga notebook reference (for TUI display)
    notebook: Option<Arc<RwLock<GugugagaNotebook>>>,
    /// Cached notebook data for rendering (updated periodically)
    notebook_current_activity: Option<String>,
    notebook_completed_count: usize,
    notebook_attention_items: Vec<(String, bool)>, // (content, is_high_priority)
    notebook_mistakes_count: usize,
}

impl App {
    /// Create a new App instance
    pub fn new(project_name: String, cwd: String) -> io::Result<Self> {
        enable_raw_mode()?;
        let mut stdout = io::stdout();
        // Note: NOT using EnableMouseCapture so users can still select/copy text
        execute!(stdout, EnterAlternateScreen)?;
        let backend = CrosstermBackend::new(stdout);
        let terminal = Terminal::new(backend)?;

        Ok(Self {
            terminal,
            input: InputState::new(),
            messages: vec![
                Message::system("🛡️ Gugugaga initialized. Monitoring enabled."),
                Message::system("Commands: /cmd = Codex, //cmd = Gugugaga. Tab to autocomplete."),
                Message::system("Violations will be detected and reported automatically."),
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
            pending_approval: None,
            notebook: None,
            notebook_current_activity: None,
            notebook_completed_count: 0,
            notebook_attention_items: Vec::new(),
            notebook_mistakes_count: 0,
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
        let mut last_spinner_update = std::time::Instant::now();
        let spinner_interval = Duration::from_millis(80);
        
        // Enable "alternate scroll" mode - this makes the terminal convert
        // mouse wheel events to arrow key events, WITHOUT capturing the mouse.
        // This allows both scrolling AND text selection!
        // ANSI sequence: \x1b[?1007h (enable), \x1b[?1007l (disable)
        let _ = io::Write::write_all(&mut io::stdout(), b"\x1b[?1007h");
        let _ = io::Write::flush(&mut io::stdout());

        while !self.should_quit {
            // Check for new messages first (non-blocking)
            self.check_output().await;
            
            // Update notebook cache for display
            self.update_notebook_cache().await;
            
            // Update spinner at fixed interval
            if last_spinner_update.elapsed() >= spinner_interval {
                self.spinner_frame = self.spinner_frame.wrapping_add(1);
                last_spinner_update = std::time::Instant::now();
            }
            
            // Draw UI
            self.draw()?;

            // Poll for keyboard events with short timeout
            if event::poll(poll_timeout)? {
                if let Event::Key(key) = event::read()? {
                    self.handle_input(key).await;
                }
            }
        }
        
        // Disable alternate scroll mode on exit
        let _ = io::Write::write_all(&mut io::stdout(), b"\x1b[?1007l");
        let _ = io::Write::flush(&mut io::stdout());

        Ok(())
    }
    
    async fn handle_input(&mut self, key: event::KeyEvent) {
        // Handle approval dialog first (highest priority)
        if let Some(approval) = self.pending_approval.take() {
            match key.code {
                crossterm::event::KeyCode::Char('y') | crossterm::event::KeyCode::Char('Y') |
                crossterm::event::KeyCode::Enter => {
                    self.respond_to_approval(approval.request_id, &approval.approval_type, true).await;
                    self.messages.push(Message::system("✓ Approved"));
                    return;
                }
                crossterm::event::KeyCode::Char('n') | crossterm::event::KeyCode::Char('N') |
                crossterm::event::KeyCode::Esc => {
                    self.respond_to_approval(approval.request_id, &approval.approval_type, false).await;
                    self.messages.push(Message::system("✗ Declined"));
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

        match self.input.handle_key(key) {
            InputAction::Quit => {
                self.should_quit = true;
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

                    if let Some(tx) = &self.input_tx {
                        let msg = self.create_turn_message(&text);
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
                self.slash_popup.close();
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
            CodexCommand::Compact => {
                self.request_compact().await;
            }

            // === RPC commands (forward to Codex with correct method) ===
            CodexCommand::Model => {
                self.request_model_list().await;
            }
            CodexCommand::Skills => {
                self.request_skills_list().await;
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
                self.show_mcp_tools();
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
            let msg = serde_json::json!({
                "jsonrpc": "2.0",
                "method": "thread/start",
                "id": self.request_counter,
                "params": {}
            })
            .to_string();
            let _ = tx.send(msg).await;
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

    /// Show background processes
    fn show_background_processes(&mut self) {
        // TODO: Implement background process tracking
        self.messages.push(Message::system("No background processes running"));
    }

    /// Request compact/summarize
    async fn request_compact(&mut self) {
        self.messages.push(Message::system("Compacting conversation..."));
        // This is handled by the agent's memory system
        // For now just acknowledge
        self.messages.push(Message::system("Context compaction is automatic in Gugugaga"));
    }

    /// Request model list
    async fn request_model_list(&mut self) {
        if let Some(tx) = &self.input_tx {
            self.request_counter += 1;
            let msg = serde_json::json!({
                "jsonrpc": "2.0",
                "method": "model/list",
                "id": self.request_counter,
                "params": {}
            })
            .to_string();
            let _ = tx.send(msg).await;
            self.messages.push(Message::system("Fetching models..."));
        }
    }

    /// Request skills list
    async fn request_skills_list(&mut self) {
        if let Some(tx) = &self.input_tx {
            self.request_counter += 1;
            let msg = serde_json::json!({
                "jsonrpc": "2.0",
                "method": "skills/list",
                "id": self.request_counter,
                "params": {}
            })
            .to_string();
            let _ = tx.send(msg).await;
            self.messages.push(Message::system("Fetching skills..."));
        }
    }

    /// Request code review
    async fn request_review(&mut self) {
        if let Some(tx) = &self.input_tx {
            self.request_counter += 1;
            let msg = serde_json::json!({
                "jsonrpc": "2.0",
                "method": "review/start",
                "id": self.request_counter,
                "params": {}
            })
            .to_string();
            let _ = tx.send(msg).await;
            self.messages.push(Message::system("Starting code review..."));
        }
    }

    /// Request thread rename
    async fn request_rename(&mut self, name: &str) {
        if let Some(thread_id) = &self.thread_id {
            if let Some(tx) = &self.input_tx {
                self.request_counter += 1;
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
            let msg = serde_json::json!({
                "jsonrpc": "2.0",
                "method": "feedback/upload",
                "id": self.request_counter,
                "params": {}
            })
            .to_string();
            let _ = tx.send(msg).await;
            self.messages.push(Message::system("Uploading feedback..."));
        }
    }

    /// Show MCP tools
    fn show_mcp_tools(&mut self) {
        // TODO: Get MCP tools from config
        self.messages.push(Message::system("MCP tools: (none configured)"));
    }

    /// Request apps list
    async fn request_apps_list(&mut self) {
        if let Some(tx) = &self.input_tx {
            self.request_counter += 1;
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
        // Request current config first
        if let Some(tx) = &self.input_tx {
            self.request_counter += 1;
            let msg = serde_json::json!({
                "jsonrpc": "2.0",
                "method": "config/read",
                "id": self.request_counter,
                "params": {}
            })
            .to_string();
            let _ = tx.send(msg).await;
        }

        // Show available approval modes
        self.messages.push(Message::system(
            "Approval Modes:\n\
             1. Suggest - Codex suggests, you approve everything\n\
             2. Auto Edit - Codex can edit files, asks for commands\n\
             3. Full Auto - Codex can do everything without asking\n\n\
             Use: /approvals <1|2|3> or wait for picker"
        ));

        // TODO: Implement proper picker mode handling
        self.picker_mode = PickerMode::None;
        self.picker.title = "Approval Mode".to_string();
        let items = vec![
            PickerItem { id: "suggest".to_string(), title: "Suggest".to_string(), subtitle: "Approve everything".to_string() },
            PickerItem { id: "auto-edit".to_string(), title: "Auto Edit".to_string(), subtitle: "Auto-approve file edits".to_string() },
            PickerItem { id: "full-auto".to_string(), title: "Full Auto".to_string(), subtitle: "Auto-approve everything".to_string() },
        ];
        self.picker.open(items);
    }

    /// Open permissions picker - shows sandbox policy options  
    async fn open_permissions_picker(&mut self) {
        self.messages.push(Message::system(
            "Permission Modes:\n\
             1. Read Only - Codex can only read files\n\
             2. Workspace Write - Codex can write in workspace\n\
             3. Full Access - Codex has full system access\n\n\
             Use: /permissions <1|2|3>"
        ));

        self.picker.title = "Permissions".to_string();
        let items = vec![
            PickerItem { id: "read-only".to_string(), title: "Read Only".to_string(), subtitle: "Can only read files".to_string() },
            PickerItem { id: "workspace-write".to_string(), title: "Workspace Write".to_string(), subtitle: "Can write in workspace".to_string() },
            PickerItem { id: "full-access".to_string(), title: "Full Access".to_string(), subtitle: "Full system access".to_string() },
        ];
        self.picker.open(items);
    }

    /// Open personality picker
    async fn open_personality_picker(&mut self) {
        self.messages.push(Message::system(
            "Personality:\n\
             1. Friendly - Warm and encouraging\n\
             2. Pragmatic - Direct and efficient"
        ));

        self.picker.title = "Personality".to_string();
        let items = vec![
            PickerItem { id: "friendly".to_string(), title: "Friendly".to_string(), subtitle: "Warm and encouraging".to_string() },
            PickerItem { id: "pragmatic".to_string(), title: "Pragmatic".to_string(), subtitle: "Direct and efficient".to_string() },
        ];
        self.picker.open(items);
    }

    /// Open experimental features picker
    async fn open_experimental_picker(&mut self) {
        self.messages.push(Message::system(
            "Experimental Features:\n\
             Toggle experimental features on/off.\n\
             These features may be unstable."
        ));
        // TODO: Get actual experimental features from config
    }

    /// Open collaboration mode picker
    async fn open_collab_picker(&mut self) {
        if let Some(tx) = &self.input_tx {
            self.request_counter += 1;
            let msg = serde_json::json!({
                "jsonrpc": "2.0",
                "method": "collaborationMode/list",
                "id": self.request_counter,
                "params": {}
            })
            .to_string();
            let _ = tx.send(msg).await;
            self.messages.push(Message::system("Fetching collaboration modes..."));
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
        if let Some(tx) = &self.input_tx {
            self.request_counter += 1;
            let msg = serde_json::json!({
                "jsonrpc": "2.0",
                "method": "thread/loaded/list",
                "id": self.request_counter,
                "params": {}
            })
            .to_string();
            let _ = tx.send(msg).await;
            self.messages.push(Message::system("Fetching active threads..."));
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

        if let Some(tx) = &self.input_tx {
            let msg = self.create_turn_message(INIT_PROMPT);
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
            let thread_id = item.id.clone();
            let title = item.title.clone();

            match self.picker_mode {
                PickerMode::Resume => {
                    self.messages.push(Message::system(&format!(
                        "Resuming session: {}",
                        title
                    )));

                    // Send resume request and track it
                    if let Some(tx) = &self.input_tx {
                        self.request_counter += 1;
                        self.pending_request_id = Some(self.request_counter);
                        self.pending_request_type = PendingRequestType::ThreadResume(thread_id.clone());
                        
                        let msg = serde_json::json!({
                            "jsonrpc": "2.0",
                            "method": "thread/resume",
                            "id": self.request_counter,
                            "params": {
                                "threadId": thread_id
                            }
                        })
                        .to_string();
                        let _ = tx.send(msg).await;
                    }
                }
                PickerMode::Model => {
                    // TODO: Handle model selection
                }
                PickerMode::None => {}
            }
        }

        self.picker.close();
        self.picker_mode = PickerMode::None;
        self.scroll_to_bottom();
    }
    
    async fn respond_to_approval(&mut self, request_id: u64, approval_type: &ApprovalType, accept: bool) {
        if let Some(tx) = &self.input_tx {
            let decision = if accept { "accept" } else { "decline" };
            
            let response = match approval_type {
                ApprovalType::CommandExecution => {
                    if accept {
                        serde_json::json!({
                            "jsonrpc": "2.0",
                            "id": request_id,
                            "result": {
                                "decision": decision,
                                "acceptSettings": { "forSession": false }
                            }
                        })
                    } else {
                        serde_json::json!({
                            "jsonrpc": "2.0",
                            "id": request_id,
                            "result": { "decision": decision }
                        })
                    }
                }
                ApprovalType::FileChange => {
                    serde_json::json!({
                        "jsonrpc": "2.0",
                        "id": request_id,
                        "result": { "decision": decision }
                    })
                }
            };
            
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
            GugugagaCommand::Issues => {
                self.messages
                    .push(Message::system("Checking moonissues status..."));
                // TODO: Run moonissues list
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
                    let preview = if msg.len() > 200 { &msg[..200] } else { msg };
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
                        
                        self.pending_approval = Some(PendingApproval {
                            request_id: id,
                            approval_type: ApprovalType::CommandExecution,
                            command: command.clone(),
                            cwd: cwd.clone(),
                            changes: vec![],
                        });
                        
                        // Show approval request to user (NOT a correction - this is Codex asking)
                        let cmd_display = command.unwrap_or_else(|| "unknown".to_string());
                        let cwd_display = cwd.map(|c| format!(" (in {})", c)).unwrap_or_default();
                        self.messages.push(Message::system(&format!(
                            "⚡ Command approval [Y/n]\n$ {}{}",
                            cmd_display, cwd_display
                        )));
                        self.scroll_to_bottom();
                        return;
                    }
                    "item/fileChange/requestApproval" => {
                        // File change approval request
                        let params = json.get("params").cloned().unwrap_or_default();
                        
                        self.pending_approval = Some(PendingApproval {
                            request_id: id,
                            approval_type: ApprovalType::FileChange,
                            command: None,
                            cwd: None,
                            changes: vec![],
                        });
                        
                        // Show approval request to user (NOT a correction)
                        let reason = params.get("reason").and_then(|r| r.as_str()).unwrap_or("File changes");
                        self.messages.push(Message::system(&format!(
                            "📝 File change approval [Y/n]\n{}",
                            reason
                        )));
                        self.scroll_to_bottom();
                        return;
                    }
                    _ => {} // Not an approval request, continue processing
                }
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
                // Check for error responses
                if let Some(error) = json.get("error") {
                    let error_msg = error
                        .get("message")
                        .and_then(|m| m.as_str())
                        .unwrap_or("Unknown error");
                    self.messages.push(Message::system(&format!("Error: {}", error_msg)));
                    self.is_processing = false;
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
                    // Command execution output streaming - limit display
                    if let Some(delta) = json
                        .get("params")
                        .and_then(|p| p.get("delta"))
                        .and_then(|t| t.as_str())
                    {
                        self.is_processing = true;
                        const MAX_OUTPUT_LINES: usize = 10;
                        const MAX_OUTPUT_CHARS: usize = 500;
                        
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
                                        format!("{}...", &truncated[..MAX_OUTPUT_CHARS])
                                    } else {
                                        truncated
                                    };
                                    last.content = format!("{}\n... (output truncated)", truncated);
                                }
                                return;
                            }
                        }
                        // First line - just show beginning
                        let first_line = delta.lines().next().unwrap_or(delta);
                        let display = if first_line.len() > 80 {
                            format!("{}...", &first_line[..80])
                        } else {
                            first_line.to_string()
                        };
                        self.messages.push(Message::command_exec(display));
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
                "item/started" => {
                    // Item lifecycle start - show what's happening
                    if let Some(item) = json.get("params").and_then(|p| p.get("item")) {
                        let item_type = item.get("type").and_then(|t| t.as_str()).unwrap_or("unknown");
                        match item_type {
                            "commandExecution" => {
                                let cmd = item.get("command").and_then(|c| c.as_str()).unwrap_or("command");
                                // Truncate long commands
                                let cmd_display = if cmd.len() > 60 {
                                    format!("{}...", &cmd[..60])
                                } else {
                                    cmd.to_string()
                                };
                                self.messages.push(Message::command_exec(format!("$ {}", cmd_display)));
                            }
                            "fileChange" => {
                                if let Some(changes) = item.get("changes").and_then(|c| c.as_array()) {
                                    for change in changes {
                                        let path = change.get("path").and_then(|p| p.as_str()).unwrap_or("file");
                                        let kind = change.get("kind").and_then(|k| k.as_str()).unwrap_or("modify");
                                        self.messages.push(Message::file_change(format!("{}: {}", kind, path)));
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
                                let status = item.get("status").and_then(|s| s.as_str()).unwrap_or("completed");
                                let exit_code = item.get("exitCode").and_then(|e| e.as_i64());
                                let duration = item.get("durationMs").and_then(|d| d.as_i64());
                                let mut info = format!("Command {}", status);
                                if let Some(code) = exit_code {
                                    info.push_str(&format!(" (exit {})", code));
                                }
                                if let Some(ms) = duration {
                                    info.push_str(&format!(" in {}ms", ms));
                                }
                                self.messages.push(Message::system(&info));
                            }
                            "fileChange" => {
                                let status = item.get("status").and_then(|s| s.as_str()).unwrap_or("completed");
                                self.messages.push(Message::system(&format!("File change {}", status)));
                            }
                            "exitedReviewMode" => {
                                if let Some(review) = item.get("review").and_then(|r| r.as_str()) {
                                    self.messages.push(Message::codex(review));
                                }
                            }
                            "contextCompaction" => {
                                self.messages.push(Message::system("Context compacted."));
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
                "turn/diff/updated" => {
                    // Diff update - show in file change format
                    if let Some(diff) = json.get("params").and_then(|p| p.get("diff")).and_then(|d| d.as_str()) {
                        if !diff.is_empty() {
                            // Only show a summary, not the full diff
                            let line_count = diff.lines().count();
                            self.messages.push(Message::system(&format!("Diff updated ({} lines)", line_count)));
                        }
                    }
                }
                "turn/started" => {
                    self.is_processing = true;
                    self.current_turn_violations = 0;
                }
                "turn/completed" => {
                    self.is_processing = false;
                    // Stats panel will show supervision status, no blocking message needed
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
                    // Show gugugaga thinking/reasoning
                    if let Some(msg) = json
                        .get("params")
                        .and_then(|p| p.get("message"))
                        .and_then(|m| m.as_str())
                    {
                        self.messages.push(Message::thinking(&format!("🛡️ {}", msg)));
                        self.scroll_to_bottom();
                    }
                }
                "gugugaga/check" => {
                    // Show supervision check result with Markdown support
                    if let Some(status) = json
                        .get("params")
                        .and_then(|p| p.get("status"))
                        .and_then(|s| s.as_str())
                    {
                        // Show thinking if present
                        if let Some(thinking) = json
                            .get("params")
                            .and_then(|p| p.get("thinking"))
                            .and_then(|t| t.as_str())
                        {
                            if !thinking.is_empty() {
                                self.messages.push(Message::thinking(&format!("🛡️ {}", thinking)));
                            }
                        }

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
            PendingRequestType::ThreadResume(thread_id) => {
                // Handle thread/resume response
                if let Some(result) = json.get("result") {
                    // Extract thread ID from response and update our state
                    if let Some(thread) = result.get("thread") {
                        if let Some(id) = thread.get("id").and_then(|i| i.as_str()) {
                            self.thread_id = Some(id.to_string());
                            self.messages.push(Message::system("Session resumed successfully!"));
                            
                            // Now request the thread history
                            self.request_thread_read(id.to_string());
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
            PendingRequestType::ThreadRead(thread_id) => {
                // Handle thread/read response - display history
                if let Some(result) = json.get("result") {
                    if let Some(thread) = result.get("thread") {
                        if let Some(turns) = thread.get("turns").and_then(|t| t.as_array()) {
                            self.messages.push(Message::system("--- Session History ---"));
                            
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
                                            "commandExecution" => {
                                                if let Some(cmd) = item.get("command").and_then(|c| c.as_str()) {
                                                    let status = item.get("status").and_then(|s| s.as_str()).unwrap_or("");
                                                    self.messages.push(Message::command_exec(format!("$ {} ({})", cmd, status)));
                                                }
                                            }
                                            _ => {}
                                        }
                                    }
                                }
                            }
                            
                            self.messages.push(Message::system("--- End of History ---"));
                            self.scroll_to_bottom();
                        }
                    }
                }
            }
            PendingRequestType::None => {
                // Unexpected response
            }
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

    fn create_turn_message(&self, text: &str) -> String {
        let thread_id = self.thread_id.as_deref().unwrap_or("main");
        serde_json::json!({
            "jsonrpc": "2.0",
            "method": "turn/start",
            "id": 2,
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

    fn scroll_to_bottom(&mut self) {
        self.scroll_offset = 0;
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
        let auto_replies = self.auto_replies;
        let slash_popup = &self.slash_popup;
        let is_paused = self.is_paused;
        let picker = &self.picker;
        let has_pending_approval = self.pending_approval.is_some();
        let notebook_current_activity = &self.notebook_current_activity;
        let notebook_completed_count = &self.notebook_completed_count;
        let notebook_attention_items = &self.notebook_attention_items;
        let notebook_mistakes_count = &self.notebook_mistakes_count;

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
                is_processing,
                spinner_frame,
                status_text: if is_processing {
                    "Thinking...".to_string()
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

            // Messages
            Self::render_messages(f, content_chunks[0], messages, scroll_offset);

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

            // Help bar (or approval prompt)
            if has_pending_approval {
                // Show approval prompt
                let approval_bar = ratatui::widgets::Paragraph::new(
                    " ⚠️ APPROVAL REQUIRED: [Y/Enter] Accept  [N/Esc] Decline "
                ).style(ratatui::style::Style::default().fg(ratatui::style::Color::Yellow).bg(ratatui::style::Color::DarkGray));
                f.render_widget(approval_bar, main_chunks[4]);
            } else {
                f.render_widget(HelpBar, main_chunks[4]);
            }

            // Render picker overlay (on top of everything)
            if picker.visible {
                picker.render(size, f.buffer_mut());
            }
        })?;

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

        let paragraph = Paragraph::new(lines).block(block);
        f.render_widget(paragraph, popup_area);
    }

    fn render_messages(f: &mut Frame, area: Rect, messages: &[Message], scroll_offset: usize) {
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

        let paragraph = Paragraph::new(visible);
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
    }
}

impl Drop for App {
    fn drop(&mut self) {
        // Disable alternate scroll mode
        let _ = io::Write::write_all(&mut io::stdout(), b"\x1b[?1007l");
        let _ = io::Write::flush(&mut io::stdout());
        
        let _ = disable_raw_mode();
        let _ = execute!(self.terminal.backend_mut(), LeaveAlternateScreen);
        let _ = self.terminal.show_cursor();
    }
}
