//! Slash command system
//!
//! - `/xxx` = Codex commands (transparently forwarded)
//! - `//xxx` = Gugugaga-specific commands

use std::fmt;

/// Codex built-in slash commands (forwarded to Codex)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CodexCommand {
    Model,
    Approvals,
    Permissions,
    ElevateSandbox,
    SandboxReadRoot,
    Experimental,
    Skills,
    Review,
    Rename,
    New,
    Resume,
    Fork,
    Init,
    Compact,
    Plan,
    Collab,
    Agent,
    Diff,
    Mention,
    Status,
    DebugConfig,
    Statusline,
    Mcp,
    Apps,
    Logout,
    Quit,
    Exit,
    Feedback,
    Rollout,
    Ps,
    Clean,
    Personality,
    TestApproval,
    MemoryDrop,
    MemoryUpdate,
}

impl CodexCommand {
    /// All Codex commands
    pub fn all() -> &'static [CodexCommand] {
        &[
            CodexCommand::Model,
            CodexCommand::Approvals,
            CodexCommand::Permissions,
            CodexCommand::ElevateSandbox,
            CodexCommand::SandboxReadRoot,
            CodexCommand::Experimental,
            CodexCommand::Skills,
            CodexCommand::Review,
            CodexCommand::Rename,
            CodexCommand::New,
            CodexCommand::Resume,
            CodexCommand::Fork,
            CodexCommand::Init,
            CodexCommand::Compact,
            CodexCommand::Plan,
            CodexCommand::Collab,
            CodexCommand::Agent,
            CodexCommand::Diff,
            CodexCommand::Mention,
            CodexCommand::Status,
            CodexCommand::DebugConfig,
            CodexCommand::Statusline,
            CodexCommand::Mcp,
            CodexCommand::Apps,
            CodexCommand::Logout,
            CodexCommand::Quit,
            CodexCommand::Exit,
            CodexCommand::Feedback,
            CodexCommand::Rollout,
            CodexCommand::Ps,
            CodexCommand::Clean,
            CodexCommand::Personality,
            CodexCommand::TestApproval,
            CodexCommand::MemoryDrop,
            CodexCommand::MemoryUpdate,
        ]
    }

    pub fn name(&self) -> &'static str {
        match self {
            CodexCommand::Model => "model",
            CodexCommand::Approvals => "approvals",
            CodexCommand::Permissions => "permissions",
            CodexCommand::ElevateSandbox => "setup-default-sandbox",
            CodexCommand::SandboxReadRoot => "sandbox-add-read-dir",
            CodexCommand::Experimental => "experimental",
            CodexCommand::Skills => "skills",
            CodexCommand::Review => "review",
            CodexCommand::Rename => "rename",
            CodexCommand::New => "new",
            CodexCommand::Resume => "resume",
            CodexCommand::Fork => "fork",
            CodexCommand::Init => "init",
            CodexCommand::Compact => "compact",
            CodexCommand::Plan => "plan",
            CodexCommand::Collab => "collab",
            CodexCommand::Agent => "agent",
            CodexCommand::Diff => "diff",
            CodexCommand::Mention => "mention",
            CodexCommand::Status => "status",
            CodexCommand::DebugConfig => "debug-config",
            CodexCommand::Statusline => "statusline",
            CodexCommand::Mcp => "mcp",
            CodexCommand::Apps => "apps",
            CodexCommand::Logout => "logout",
            CodexCommand::Quit => "quit",
            CodexCommand::Exit => "exit",
            CodexCommand::Feedback => "feedback",
            CodexCommand::Rollout => "rollout",
            CodexCommand::Ps => "ps",
            CodexCommand::Clean => "clean",
            CodexCommand::Personality => "personality",
            CodexCommand::TestApproval => "test-approval",
            CodexCommand::MemoryDrop => "debug-m-drop",
            CodexCommand::MemoryUpdate => "debug-m-update",
        }
    }

    pub fn description(&self) -> &'static str {
        match self {
            CodexCommand::Model => "choose model and reasoning effort",
            CodexCommand::Approvals => "choose what Codex is allowed to do",
            CodexCommand::Permissions => "choose what Codex is allowed to do",
            CodexCommand::ElevateSandbox => "set up elevated agent sandbox",
            CodexCommand::SandboxReadRoot => {
                "let sandbox read a directory: /sandbox-add-read-dir <absolute_path>"
            }
            CodexCommand::Experimental => "toggle experimental features",
            CodexCommand::Skills => "use skills to improve Codex",
            CodexCommand::Review => "review current changes and find issues",
            CodexCommand::Rename => "rename the current thread",
            CodexCommand::New => "start a new chat",
            CodexCommand::Resume => "resume a saved chat",
            CodexCommand::Fork => "fork the current chat",
            CodexCommand::Init => "create AGENTS.md file",
            CodexCommand::Compact => "summarize conversation to avoid context limit",
            CodexCommand::Plan => "switch to Plan mode",
            CodexCommand::Collab => "change collaboration mode",
            CodexCommand::Agent => "switch the active agent thread",
            CodexCommand::Diff => "show git diff",
            CodexCommand::Mention => "mention a file",
            CodexCommand::Status => "show session config and token usage",
            CodexCommand::DebugConfig => "show config layers for debugging",
            CodexCommand::Statusline => "configure status line items",
            CodexCommand::Mcp => "list configured MCP tools",
            CodexCommand::Apps => "manage apps",
            CodexCommand::Logout => "log out of Codex",
            CodexCommand::Quit => "exit Codex",
            CodexCommand::Exit => "exit Codex",
            CodexCommand::Feedback => "send logs to maintainers",
            CodexCommand::Rollout => "print rollout file path",
            CodexCommand::Ps => "list background terminals",
            CodexCommand::Clean => "stop all background terminals",
            CodexCommand::Personality => "choose communication style",
            CodexCommand::TestApproval => "test approval request",
            CodexCommand::MemoryDrop => "debug command (internal use)",
            CodexCommand::MemoryUpdate => "debug command (internal use)",
        }
    }

    pub fn matches(prefix: &str) -> Vec<CodexCommand> {
        let prefix = prefix.to_lowercase();
        Self::all()
            .iter()
            .filter(|cmd| cmd.is_visible())
            // Match codex popup behavior: debug commands are dispatchable but
            // not shown in the slash command menu.
            .filter(|cmd| !cmd.is_hidden_in_popup())
            // Match codex popup behavior: hide alias commands in the default
            // (empty-filter) list to avoid duplicate actions.
            .filter(|cmd| !prefix.is_empty() || !cmd.is_alias_in_popup())
            .filter(|cmd| cmd.name().starts_with(&prefix))
            .copied()
            .collect()
    }

    pub fn parse(name: &str) -> Option<CodexCommand> {
        let name = name.to_lowercase();
        Self::all().iter().find(|cmd| cmd.name() == name).copied()
    }

    fn is_visible(&self) -> bool {
        match self {
            CodexCommand::ElevateSandbox => cfg!(target_os = "windows"),
            CodexCommand::SandboxReadRoot => cfg!(target_os = "windows"),
            CodexCommand::Rollout | CodexCommand::TestApproval => cfg!(debug_assertions),
            _ => true,
        }
    }

    fn is_hidden_in_popup(&self) -> bool {
        matches!(
            self,
            CodexCommand::DebugConfig | CodexCommand::MemoryDrop | CodexCommand::MemoryUpdate
        )
    }

    fn is_alias_in_popup(&self) -> bool {
        matches!(self, CodexCommand::Quit | CodexCommand::Approvals)
    }
}

impl fmt::Display for CodexCommand {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "/{}", self.name())
    }
}

/// Gugugaga-specific commands (prefixed with //)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GugugagaCommand {
    /// Show gugugaga help
    Help,
    /// Clear chat display
    Clear,
    /// Show gugugaga stats
    Stats,
    /// View or set Gugugaga's model
    Model,
    /// View Gugugaga notebook
    Notebook,
}

impl GugugagaCommand {
    pub fn all() -> &'static [GugugagaCommand] {
        &[
            GugugagaCommand::Help,
            GugugagaCommand::Clear,
            GugugagaCommand::Stats,
            GugugagaCommand::Model,
            GugugagaCommand::Notebook,
        ]
    }

    pub fn name(&self) -> &'static str {
        match self {
            GugugagaCommand::Help => "help",
            GugugagaCommand::Clear => "clear",
            GugugagaCommand::Stats => "stats",
            GugugagaCommand::Model => "model",
            GugugagaCommand::Notebook => "notebook",
        }
    }

    pub fn description(&self) -> &'static str {
        match self {
            GugugagaCommand::Help => "Show Gugugaga help",
            GugugagaCommand::Clear => "Clear chat history",
            GugugagaCommand::Stats => "Show monitoring stats",
            GugugagaCommand::Model => "View or set Gugugaga model",
            GugugagaCommand::Notebook => "View Gugugaga notebook",
        }
    }

    pub fn takes_args(&self) -> bool {
        matches!(self, GugugagaCommand::Model)
    }

    pub fn matches(prefix: &str) -> Vec<GugugagaCommand> {
        let prefix = prefix.to_lowercase();
        Self::all()
            .iter()
            .filter(|cmd| cmd.name().starts_with(&prefix))
            .copied()
            .collect()
    }

    pub fn parse(name: &str) -> Option<GugugagaCommand> {
        let name = name.to_lowercase();
        Self::all().iter().find(|cmd| cmd.name() == name).copied()
    }
}

impl fmt::Display for GugugagaCommand {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "//{}", self.name())
    }
}

/// Parsed command result
#[derive(Debug, Clone)]
pub enum ParsedCommand {
    /// Codex command (forward to Codex)
    Codex(CodexCommand, String),
    /// Gugugaga command (handle locally)
    Gugugaga(GugugagaCommand, String),
    /// Direct chat message to Gugugaga (// followed by non-command text)
    GugugagaChat(String),
    /// Unknown command
    Unknown(String),
}

/// Parse input to determine command type
pub fn parse_command(input: &str) -> Option<ParsedCommand> {
    let input = input.trim();

    // Check for gugugaga command (//)
    if let Some(rest) = input.strip_prefix("//") {
        let parts: Vec<&str> = rest.splitn(2, ' ').collect();
        let cmd_name = parts[0];
        let args = parts.get(1).unwrap_or(&"").to_string();

        if let Some(cmd) = GugugagaCommand::parse(cmd_name) {
            return Some(ParsedCommand::Gugugaga(cmd, args));
        }
        // Not a known command — treat the entire text after // as a direct
        // chat message to Gugugaga (e.g. "// why did you flag that?")
        let chat_text = rest.trim().to_string();
        if !chat_text.is_empty() {
            return Some(ParsedCommand::GugugagaChat(chat_text));
        }
        return Some(ParsedCommand::Unknown(cmd_name.to_string()));
    }

    // Check for Codex command (/)
    if let Some(rest) = input.strip_prefix('/') {
        let parts: Vec<&str> = rest.splitn(2, ' ').collect();
        let cmd_name = parts[0];
        let args = parts.get(1).unwrap_or(&"").to_string();

        if let Some(cmd) = CodexCommand::parse(cmd_name) {
            return Some(ParsedCommand::Codex(cmd, args));
        }
        return Some(ParsedCommand::Unknown(cmd_name.to_string()));
    }

    None
}

/// Slash command popup state
#[derive(Debug, Clone)]
pub struct SlashPopup {
    /// Whether the popup is visible
    pub visible: bool,
    /// Current filter prefix
    pub filter: String,
    /// Whether showing gugugaga commands (// prefix)
    pub is_gugugaga: bool,
    /// Matched Codex commands
    pub codex_matches: Vec<CodexCommand>,
    /// Matched Gugugaga commands
    pub gugugaga_matches: Vec<GugugagaCommand>,
    /// Selected index
    pub selected: usize,
    /// Top row of the visible command window
    pub scroll_top: usize,
}

impl Default for SlashPopup {
    fn default() -> Self {
        Self::new()
    }
}

impl SlashPopup {
    const MAX_VISIBLE_ROWS: usize = 8;

    pub fn new() -> Self {
        Self {
            visible: false,
            filter: String::new(),
            is_gugugaga: false,
            codex_matches: Vec::new(),
            gugugaga_matches: Vec::new(),
            selected: 0,
            scroll_top: 0,
        }
    }

    /// Open the popup for Codex commands
    pub fn open_codex(&mut self) {
        self.visible = true;
        self.is_gugugaga = false;
        self.filter.clear();
        self.update_matches();
        self.selected = 0;
        self.scroll_top = 0;
        self.ensure_selected_visible();
    }

    /// Open the popup for Gugugaga commands
    pub fn open_gugugaga(&mut self) {
        self.visible = true;
        self.is_gugugaga = true;
        self.filter.clear();
        self.update_matches();
        self.selected = 0;
        self.scroll_top = 0;
        self.ensure_selected_visible();
    }

    /// Close the popup
    pub fn close(&mut self) {
        self.visible = false;
        self.filter.clear();
        self.codex_matches.clear();
        self.gugugaga_matches.clear();
        self.selected = 0;
        self.scroll_top = 0;
    }

    /// Update filter and refresh matches
    pub fn set_filter(&mut self, filter: &str) {
        self.filter = filter.to_string();
        self.update_matches();
        self.ensure_selected_visible();
    }

    fn update_matches(&mut self) {
        if self.is_gugugaga {
            self.gugugaga_matches = GugugagaCommand::matches(&self.filter);
            self.codex_matches.clear();
        } else {
            self.codex_matches = CodexCommand::matches(&self.filter);
            self.gugugaga_matches.clear();
        }
    }

    pub fn total_matches(&self) -> usize {
        if self.is_gugugaga {
            self.gugugaga_matches.len()
        } else {
            self.codex_matches.len()
        }
    }

    /// Move selection up
    pub fn select_prev(&mut self) {
        let total = self.total_matches();
        if total > 0 {
            if self.selected == 0 {
                self.selected = total - 1;
            } else {
                self.selected -= 1;
            }
            self.ensure_selected_visible();
        }
    }

    /// Move selection down
    pub fn select_next(&mut self) {
        let total = self.total_matches();
        if total > 0 {
            self.selected = (self.selected + 1) % total;
            self.ensure_selected_visible();
        }
    }

    /// Move selection up by one visible page.
    pub fn page_up(&mut self) {
        let total = self.total_matches();
        if total == 0 {
            return;
        }

        let step = Self::MAX_VISIBLE_ROWS.min(total);
        self.selected = self.selected.saturating_sub(step);
        self.ensure_selected_visible();
    }

    /// Move selection down by one visible page.
    pub fn page_down(&mut self) {
        let total = self.total_matches();
        if total == 0 {
            return;
        }

        let step = Self::MAX_VISIBLE_ROWS.min(total);
        self.selected = (self.selected + step).min(total - 1);
        self.ensure_selected_visible();
    }

    pub fn page_progress(&self) -> Option<(usize, usize)> {
        let total = self.total_matches();
        if total <= Self::MAX_VISIBLE_ROWS {
            return None;
        }

        let total_pages = total.div_ceil(Self::MAX_VISIBLE_ROWS);
        let current_page = (self.scroll_top / Self::MAX_VISIBLE_ROWS) + 1;
        Some((current_page, total_pages))
    }

    /// Complete the current selection
    pub fn complete(&self) -> Option<String> {
        if self.is_gugugaga {
            self.gugugaga_matches.get(self.selected).map(|cmd| {
                if cmd.takes_args() {
                    format!("{} ", cmd)
                } else {
                    cmd.to_string()
                }
            })
        } else {
            self.codex_matches
                .get(self.selected)
                .map(|cmd| cmd.to_string())
        }
    }

    /// Get display items for rendering
    pub fn display_items(&self) -> Vec<(String, String, bool)> {
        let total = self.total_matches();
        if total == 0 {
            return Vec::new();
        }

        let visible = Self::MAX_VISIBLE_ROWS.min(total);
        let start = self.scroll_top.min(total.saturating_sub(visible));

        if self.is_gugugaga {
            self.gugugaga_matches
                .iter()
                .enumerate()
                .skip(start)
                .take(visible)
                .map(|(i, cmd)| {
                    (
                        format!("//{}", cmd.name()),
                        cmd.description().to_string(),
                        i == self.selected,
                    )
                })
                .collect()
        } else {
            self.codex_matches
                .iter()
                .enumerate()
                .skip(start)
                .take(visible)
                .map(|(i, cmd)| {
                    (
                        format!("/{}", cmd.name()),
                        cmd.description().to_string(),
                        i == self.selected,
                    )
                })
                .collect()
        }
    }

    fn ensure_selected_visible(&mut self) {
        let total = self.total_matches();
        if total == 0 {
            self.selected = 0;
            self.scroll_top = 0;
            return;
        }

        if self.selected >= total {
            self.selected = total - 1;
        }

        let visible = Self::MAX_VISIBLE_ROWS.min(total);
        if self.selected < self.scroll_top {
            self.scroll_top = self.selected;
        } else {
            let bottom = self.scroll_top + visible - 1;
            if self.selected > bottom {
                self.scroll_top = self.selected + 1 - visible;
            }
        }

        let max_top = total.saturating_sub(visible);
        if self.scroll_top > max_top {
            self.scroll_top = max_top;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_codex_command() {
        match parse_command("/resume") {
            Some(ParsedCommand::Codex(CodexCommand::Resume, _)) => {}
            _ => panic!("Should parse as Codex resume"),
        }
    }

    #[test]
    fn test_parse_codex_kebab_case_command() {
        match parse_command("/debug-config") {
            Some(ParsedCommand::Codex(CodexCommand::DebugConfig, _)) => {}
            _ => panic!("Should parse as Codex debug-config"),
        }
    }

    #[test]
    fn test_parse_codex_sandbox_read_root_command() {
        match parse_command("/sandbox-add-read-dir /tmp") {
            Some(ParsedCommand::Codex(CodexCommand::SandboxReadRoot, args)) => {
                assert_eq!(args, "/tmp")
            }
            _ => panic!("Should parse as Codex sandbox-add-read-dir"),
        }
    }

    #[test]
    fn test_parse_codex_debug_memory_commands() {
        match parse_command("/debug-m-drop") {
            Some(ParsedCommand::Codex(CodexCommand::MemoryDrop, _)) => {}
            _ => panic!("Should parse as Codex debug-m-drop"),
        }
        match parse_command("/debug-m-update") {
            Some(ParsedCommand::Codex(CodexCommand::MemoryUpdate, _)) => {}
            _ => panic!("Should parse as Codex debug-m-update"),
        }
    }

    #[test]
    fn test_parse_gugugaga_command() {
        match parse_command("//help") {
            Some(ParsedCommand::Gugugaga(GugugagaCommand::Help, _)) => {}
            _ => panic!("Should parse as Gugugaga help"),
        }
    }

    #[test]
    fn test_parse_gugugaga_stats() {
        match parse_command("//stats") {
            Some(ParsedCommand::Gugugaga(GugugagaCommand::Stats, _)) => {}
            _ => panic!("Should parse as Gugugaga stats"),
        }
    }

    #[test]
    fn test_popup_modes() {
        let mut popup = SlashPopup::new();

        popup.open_codex();
        assert!(!popup.is_gugugaga);
        assert!(!popup.codex_matches.is_empty());

        popup.open_gugugaga();
        assert!(popup.is_gugugaga);
        assert!(!popup.gugugaga_matches.is_empty());
    }

    #[test]
    fn test_popup_hides_aliases_by_default() {
        let mut popup = SlashPopup::new();
        popup.open_codex();
        let names: Vec<&str> = popup.codex_matches.iter().map(CodexCommand::name).collect();

        assert!(!names.contains(&"quit"));
        assert!(!names.contains(&"approvals"));
        assert!(names.contains(&"exit"));
        assert!(names.contains(&"permissions"));
    }

    #[test]
    fn test_popup_shows_alias_when_filtered() {
        let mut popup = SlashPopup::new();
        popup.open_codex();
        popup.set_filter("appro");
        let names: Vec<&str> = popup.codex_matches.iter().map(CodexCommand::name).collect();
        assert_eq!(names, vec!["approvals"]);
    }

    #[test]
    fn test_popup_hides_debug_commands() {
        let mut popup = SlashPopup::new();
        popup.open_codex();
        popup.set_filter("debug");
        let names: Vec<&str> = popup.codex_matches.iter().map(CodexCommand::name).collect();
        assert!(names.is_empty());
    }

    #[test]
    fn test_popup_scrolls_visible_window_with_selection() {
        let mut popup = SlashPopup::new();
        popup.open_codex();
        assert!(
            popup.total_matches() > 8,
            "expected enough commands to test popup scrolling"
        );

        for _ in 0..8 {
            popup.select_next();
        }

        assert_eq!(popup.selected, 8);
        assert_eq!(popup.scroll_top, 1);
        let items = popup.display_items();
        assert_eq!(items.len(), 8);
        assert!(items.iter().any(|(_, _, selected)| *selected));
    }

    #[test]
    fn test_popup_page_navigation() {
        let mut popup = SlashPopup::new();
        popup.open_codex();
        assert!(
            popup.total_matches() > 8,
            "expected enough commands to test popup paging"
        );

        popup.page_down();
        assert!(popup.selected >= 8);
        assert!(popup.scroll_top > 0);

        popup.page_up();
        assert_eq!(popup.selected, 0);
        assert_eq!(popup.scroll_top, 0);
    }

    #[cfg(not(target_os = "windows"))]
    #[test]
    fn test_popup_hides_windows_only_commands_on_non_windows() {
        let mut popup = SlashPopup::new();
        popup.open_codex();
        let names: Vec<&str> = popup.codex_matches.iter().map(CodexCommand::name).collect();
        assert!(!names.contains(&"sandbox-add-read-dir"));
        assert!(!names.contains(&"setup-default-sandbox"));
    }
}
