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
    Mcp,
    Apps,
    Logout,
    Quit,
    Exit,
    Feedback,
    Ps,
    Personality,
}

impl CodexCommand {
    /// All Codex commands
    pub fn all() -> &'static [CodexCommand] {
        &[
            CodexCommand::Model,
            CodexCommand::Approvals,
            CodexCommand::Permissions,
            CodexCommand::ElevateSandbox,
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
            CodexCommand::Mcp,
            CodexCommand::Apps,
            CodexCommand::Logout,
            CodexCommand::Quit,
            CodexCommand::Exit,
            CodexCommand::Feedback,
            CodexCommand::Ps,
            CodexCommand::Personality,
        ]
    }

    pub fn name(&self) -> &'static str {
        match self {
            CodexCommand::Model => "model",
            CodexCommand::Approvals => "approvals",
            CodexCommand::Permissions => "permissions",
            CodexCommand::ElevateSandbox => "setup-elevated-sandbox",
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
            CodexCommand::Mcp => "mcp",
            CodexCommand::Apps => "apps",
            CodexCommand::Logout => "logout",
            CodexCommand::Quit => "quit",
            CodexCommand::Exit => "exit",
            CodexCommand::Feedback => "feedback",
            CodexCommand::Ps => "ps",
            CodexCommand::Personality => "personality",
        }
    }

    pub fn description(&self) -> &'static str {
        match self {
            CodexCommand::Model => "choose model and reasoning effort",
            CodexCommand::Approvals => "choose what Codex can do without approval",
            CodexCommand::Permissions => "choose what Codex is allowed to do",
            CodexCommand::ElevateSandbox => "set up elevated agent sandbox",
            CodexCommand::Experimental => "toggle experimental features",
            CodexCommand::Skills => "use skills to improve Codex",
            CodexCommand::Review => "review current changes and find issues",
            CodexCommand::Rename => "rename the current thread",
            CodexCommand::New => "start a new chat",
            CodexCommand::Resume => "resume a saved chat",
            CodexCommand::Fork => "fork the current chat",
            CodexCommand::Init => "create AGENTS.md file",
            CodexCommand::Compact => "summarize to prevent context limit",
            CodexCommand::Plan => "switch to Plan mode",
            CodexCommand::Collab => "change collaboration mode",
            CodexCommand::Agent => "switch the active agent thread",
            CodexCommand::Diff => "show git diff",
            CodexCommand::Mention => "mention a file",
            CodexCommand::Status => "show session config and token usage",
            CodexCommand::Mcp => "list configured MCP tools",
            CodexCommand::Apps => "manage apps",
            CodexCommand::Logout => "log out of Codex",
            CodexCommand::Quit => "exit Codex",
            CodexCommand::Exit => "exit Codex",
            CodexCommand::Feedback => "send logs to maintainers",
            CodexCommand::Ps => "list background terminals",
            CodexCommand::Personality => "choose communication style",
        }
    }

    pub fn matches(prefix: &str) -> Vec<CodexCommand> {
        let prefix = prefix.to_lowercase();
        Self::all()
            .iter()
            .filter(|cmd| cmd.name().starts_with(&prefix))
            .copied()
            .collect()
    }

    pub fn parse(name: &str) -> Option<CodexCommand> {
        let name = name.to_lowercase();
        Self::all().iter().find(|cmd| cmd.name() == name).copied()
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
    /// Show current rules and instructions
    Rules,
    /// Add a permanent instruction
    Instruct,
    /// Set current task objective
    Task,
    /// List detected violations
    Violations,
    /// Show moonissues status
    Issues,
    /// Pause gugugaga monitoring
    Pause,
    /// Resume gugugaga monitoring (note: different from Codex /resume!)
    Unpause,
    /// Save memory to disk
    Save,
    /// Quit gugugaga
    Quit,
}

impl GugugagaCommand {
    pub fn all() -> &'static [GugugagaCommand] {
        &[
            GugugagaCommand::Help,
            GugugagaCommand::Clear,
            GugugagaCommand::Stats,
            GugugagaCommand::Rules,
            GugugagaCommand::Instruct,
            GugugagaCommand::Task,
            GugugagaCommand::Violations,
            GugugagaCommand::Issues,
            GugugagaCommand::Pause,
            GugugagaCommand::Unpause,
            GugugagaCommand::Save,
            GugugagaCommand::Quit,
        ]
    }

    pub fn name(&self) -> &'static str {
        match self {
            GugugagaCommand::Help => "help",
            GugugagaCommand::Clear => "clear",
            GugugagaCommand::Stats => "stats",
            GugugagaCommand::Rules => "rules",
            GugugagaCommand::Instruct => "instruct",
            GugugagaCommand::Task => "task",
            GugugagaCommand::Violations => "violations",
            GugugagaCommand::Issues => "issues",
            GugugagaCommand::Pause => "pause",
            GugugagaCommand::Unpause => "unpause",
            GugugagaCommand::Save => "save",
            GugugagaCommand::Quit => "quit",
        }
    }

    pub fn description(&self) -> &'static str {
        match self {
            GugugagaCommand::Help => "Show Gugugaga help",
            GugugagaCommand::Clear => "Clear chat history",
            GugugagaCommand::Stats => "Show monitoring stats",
            GugugagaCommand::Rules => "Show current rules",
            GugugagaCommand::Instruct => "Add permanent instruction",
            GugugagaCommand::Task => "Set current task objective",
            GugugagaCommand::Violations => "List detected violations",
            GugugagaCommand::Issues => "Show moonissues status",
            GugugagaCommand::Pause => "Pause monitoring",
            GugugagaCommand::Unpause => "Resume monitoring",
            GugugagaCommand::Save => "Save memory to disk",
            GugugagaCommand::Quit => "Quit Gugugaga",
        }
    }

    pub fn takes_args(&self) -> bool {
        matches!(self, GugugagaCommand::Instruct | GugugagaCommand::Task)
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
    /// Unknown command
    Unknown(String),
}

/// Parse input to determine command type
pub fn parse_command(input: &str) -> Option<ParsedCommand> {
    let input = input.trim();

    // Check for gugugaga command (//)
    if input.starts_with("//") {
        let rest = &input[2..];
        let parts: Vec<&str> = rest.splitn(2, ' ').collect();
        let cmd_name = parts[0];
        let args = parts.get(1).unwrap_or(&"").to_string();

        if let Some(cmd) = GugugagaCommand::parse(cmd_name) {
            return Some(ParsedCommand::Gugugaga(cmd, args));
        }
        return Some(ParsedCommand::Unknown(cmd_name.to_string()));
    }

    // Check for Codex command (/)
    if input.starts_with('/') {
        let rest = &input[1..];
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
}

impl Default for SlashPopup {
    fn default() -> Self {
        Self::new()
    }
}

impl SlashPopup {
    pub fn new() -> Self {
        Self {
            visible: false,
            filter: String::new(),
            is_gugugaga: false,
            codex_matches: Vec::new(),
            gugugaga_matches: Vec::new(),
            selected: 0,
        }
    }

    /// Open the popup for Codex commands
    pub fn open_codex(&mut self) {
        self.visible = true;
        self.is_gugugaga = false;
        self.filter.clear();
        self.update_matches();
        self.selected = 0;
    }

    /// Open the popup for Gugugaga commands
    pub fn open_gugugaga(&mut self) {
        self.visible = true;
        self.is_gugugaga = true;
        self.filter.clear();
        self.update_matches();
        self.selected = 0;
    }

    /// Close the popup
    pub fn close(&mut self) {
        self.visible = false;
        self.filter.clear();
        self.codex_matches.clear();
        self.gugugaga_matches.clear();
        self.selected = 0;
    }

    /// Update filter and refresh matches
    pub fn set_filter(&mut self, filter: &str) {
        self.filter = filter.to_string();
        self.update_matches();
        if self.selected >= self.total_matches() {
            self.selected = 0;
        }
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

    fn total_matches(&self) -> usize {
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
        }
    }

    /// Move selection down
    pub fn select_next(&mut self) {
        let total = self.total_matches();
        if total > 0 {
            self.selected = (self.selected + 1) % total;
        }
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
        if self.is_gugugaga {
            self.gugugaga_matches
                .iter()
                .enumerate()
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
}

// Re-export for backwards compatibility
pub use CodexCommand as SlashCommand;

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
    fn test_parse_gugugaga_command() {
        match parse_command("//help") {
            Some(ParsedCommand::Gugugaga(GugugagaCommand::Help, _)) => {}
            _ => panic!("Should parse as Gugugaga help"),
        }
    }

    #[test]
    fn test_parse_gugugaga_with_args() {
        match parse_command("//instruct Remember to speak Chinese") {
            Some(ParsedCommand::Gugugaga(GugugagaCommand::Instruct, args)) => {
                assert_eq!(args, "Remember to speak Chinese");
            }
            _ => panic!("Should parse with args"),
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
}
