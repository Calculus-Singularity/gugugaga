//! Persistent memory storage in Markdown format

use crate::Result;
use chrono::{DateTime, Utc};
use std::path::{Path, PathBuf};
use tokio::fs;
use tokio::io::AsyncWriteExt;

/// Approximate bytes per token for context estimation
const APPROX_BYTES_PER_TOKEN: usize = 4;

/// Persistent memory that survives context compaction
#[derive(Debug, Clone)]
pub struct PersistentMemory {
    /// Path to the memory file
    file_path: PathBuf,

    /// User's core instructions (never deleted)
    pub user_instructions: Vec<UserInstruction>,

    /// Current task objectives
    pub current_task: Option<TaskObjective>,

    /// Key decisions made during the session
    pub decisions: Vec<Decision>,

    /// Agent behavior log (recent entries)
    pub behavior_log: Vec<BehaviorEntry>,

    /// Recent conversation history (user + codex messages)
    pub conversation_history: Vec<ConversationTurn>,

    /// Full conversation archive for search (stored separately)
    conversation_archive_path: PathBuf,
}


#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ConversationTurn {
    pub timestamp: DateTime<Utc>,
    pub role: TurnRole,
    pub content: String,
    /// Approximate token count
    pub tokens: usize,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum TurnRole {
    User,
    Codex,
    Gugugaga,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct UserInstruction {
    pub timestamp: DateTime<Utc>,
    pub content: String,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct TaskObjective {
    pub main_goal: String,
    pub constraints: Vec<String>,
    pub started_at: DateTime<Utc>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Decision {
    pub timestamp: DateTime<Utc>,
    pub what: String,
    pub why: String,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct BehaviorEntry {
    pub timestamp: DateTime<Utc>,
    pub action: String,
    pub was_corrected: bool,
}

impl PersistentMemory {
    /// Create a new persistent memory instance
    pub async fn new(file_path: PathBuf) -> Result<Self> {
        let archive_path = file_path.with_extension("archive.jsonl");
        let memory = Self {
            file_path: file_path.clone(),
            user_instructions: Vec::new(),
            current_task: None,
            decisions: Vec::new(),
            behavior_log: Vec::new(),
            conversation_history: Vec::new(),
            conversation_archive_path: archive_path,
        };

        // Load existing memory if file exists
        if file_path.exists() {
            memory.load().await
        } else {
            // Create directory if needed
            if let Some(parent) = file_path.parent() {
                fs::create_dir_all(parent).await?;
            }
            memory.save().await?;
            Ok(memory)
        }
    }

    /// Estimate token count for a string
    fn estimate_tokens(text: &str) -> usize {
        text.len() / APPROX_BYTES_PER_TOKEN
    }

    /// Add a conversation turn and manage history size
    pub async fn add_turn(&mut self, role: TurnRole, content: String) -> Result<()> {
        let tokens = Self::estimate_tokens(&content);
        let turn = ConversationTurn {
            timestamp: Utc::now(),
            role,
            content: content.clone(),
            tokens,
        };

        // Archive to file for search
        self.archive_turn(&turn).await?;

        // Add to recent history (compaction is handled externally by Compactor)
        self.conversation_history.push(turn);

        Ok(())
    }

    /// Estimate total token usage of conversation history
    pub fn history_token_usage(&self) -> usize {
        self.conversation_history.iter().map(|t| t.tokens).sum()
    }

    /// Get mutable access to conversation history (for compaction)
    pub fn conversation_history_mut(&mut self) -> &mut Vec<ConversationTurn> {
        &mut self.conversation_history
    }

    /// Archive a turn to the JSONL file for later search
    async fn archive_turn(&self, turn: &ConversationTurn) -> Result<()> {
        let json = serde_json::json!({
            "timestamp": turn.timestamp.to_rfc3339(),
            "role": format!("{:?}", turn.role),
            "content": turn.content,
        });
        let mut file = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.conversation_archive_path)
            .await?;
        file.write_all(format!("{}\n", json).as_bytes()).await?;
        Ok(())
    }

    /// Search conversation history by keyword
    pub async fn search_history(&self, query: &str) -> Result<Vec<ConversationTurn>> {
        let mut results = Vec::new();
        let query_lower = query.to_lowercase();

        // First search in-memory recent history
        for turn in &self.conversation_history {
            if turn.content.to_lowercase().contains(&query_lower) {
                results.push(turn.clone());
            }
        }

        // Then search archive file
        if self.conversation_archive_path.exists() {
            let content = fs::read_to_string(&self.conversation_archive_path).await?;
            for line in content.lines() {
                if let Ok(json) = serde_json::from_str::<serde_json::Value>(line) {
                    if let Some(text) = json.get("content").and_then(|c| c.as_str()) {
                        if text.to_lowercase().contains(&query_lower) {
                            let role = match json.get("role").and_then(|r| r.as_str()) {
                                Some("User") => TurnRole::User,
                                Some("Codex") => TurnRole::Codex,
                                _ => TurnRole::Gugugaga,
                            };
                            let timestamp = json
                                .get("timestamp")
                                .and_then(|t| t.as_str())
                                .and_then(|t| DateTime::parse_from_rfc3339(t).ok())
                                .map(|t| t.with_timezone(&Utc))
                                .unwrap_or_else(Utc::now);
                            results.push(ConversationTurn {
                                timestamp,
                                role,
                                content: text.to_string(),
                                tokens: Self::estimate_tokens(text),
                            });
                        }
                    }
                }
            }
        }

        Ok(results)
    }

    /// Get recent conversation as formatted string for prompt
    pub fn recent_conversation_str(&self) -> String {
        self.conversation_history
            .iter()
            .map(|t| {
                let role = match t.role {
                    TurnRole::User => "User",
                    TurnRole::Codex => "Codex",
                    TurnRole::Gugugaga => "Gugugaga",
                };
                format!("[{}] {}", role, t.content)
            })
            .collect::<Vec<_>>()
            .join("\n\n")
    }

    /// Read the most recent N turns from the JSONL archive (full content).
    pub async fn read_recent_turns(&self, n: usize) -> Result<Vec<ConversationTurn>> {
        if !self.conversation_archive_path.exists() {
            return Ok(Vec::new());
        }
        let content = fs::read_to_string(&self.conversation_archive_path).await?;
        let lines: Vec<&str> = content.lines().collect();
        let start = lines.len().saturating_sub(n);
        let mut turns = Vec::new();
        for line in &lines[start..] {
            if let Ok(json) = serde_json::from_str::<serde_json::Value>(line) {
                turns.push(Self::parse_archive_line(&json));
            }
        }
        Ok(turns)
    }

    /// Read a specific turn by index (0-based) from the JSONL archive.
    pub async fn read_turn_at(&self, index: usize) -> Result<Option<ConversationTurn>> {
        if !self.conversation_archive_path.exists() {
            return Ok(None);
        }
        let content = fs::read_to_string(&self.conversation_archive_path).await?;
        let line = content.lines().nth(index);
        Ok(line.and_then(|l| {
            serde_json::from_str::<serde_json::Value>(l)
                .ok()
                .map(|json| Self::parse_archive_line(&json))
        }))
    }

    /// Count total turns in the JSONL archive.
    pub async fn total_turns(&self) -> Result<usize> {
        if !self.conversation_archive_path.exists() {
            return Ok(0);
        }
        let content = fs::read_to_string(&self.conversation_archive_path).await?;
        Ok(content.lines().filter(|l| !l.trim().is_empty()).count())
    }

    /// Parse a single JSONL line from the archive into a ConversationTurn.
    fn parse_archive_line(json: &serde_json::Value) -> ConversationTurn {
        let role = match json.get("role").and_then(|r| r.as_str()) {
            Some("User") => TurnRole::User,
            Some("Codex") => TurnRole::Codex,
            _ => TurnRole::Gugugaga,
        };
        let content = json
            .get("content")
            .and_then(|c| c.as_str())
            .unwrap_or("")
            .to_string();
        let timestamp = json
            .get("timestamp")
            .and_then(|t| t.as_str())
            .and_then(|t| DateTime::parse_from_rfc3339(t).ok())
            .map(|t| t.with_timezone(&Utc))
            .unwrap_or_else(Utc::now);
        let tokens = Self::estimate_tokens(&content);
        ConversationTurn {
            timestamp,
            role,
            content,
            tokens,
        }
    }

    /// Load memory from file
    async fn load(mut self) -> Result<Self> {
        let content = fs::read_to_string(&self.file_path).await?;
        self.parse_markdown(&content)?;
        Ok(self)
    }

    /// Parse markdown content into memory structure
    fn parse_markdown(&mut self, content: &str) -> Result<()> {
        let mut current_section = String::new();
        let mut section_content = Vec::new();

        for line in content.lines() {
            if line.starts_with("## ") {
                // Process previous section
                if !current_section.is_empty() {
                    self.process_section(&current_section, &section_content)?;
                }
                current_section = line[3..].trim().to_string();
                section_content.clear();
            } else if !current_section.is_empty() {
                section_content.push(line.to_string());
            }
        }

        // Process last section
        if !current_section.is_empty() {
            self.process_section(&current_section, &section_content)?;
        }

        Ok(())
    }

    fn process_section(&mut self, section: &str, lines: &[String]) -> Result<()> {
        match section {
            "UserCore Instructions" | "User Core Instructions" => {
                for line in lines {
                    if let Some(instruction) = Self::parse_instruction_line(line) {
                        self.user_instructions.push(instruction);
                    }
                }
            }
            "Current TaskObjective" | "Current Task Objective" => {
                self.current_task = Self::parse_task_objective(lines);
            }
            "Key Decisions" => {
                for line in lines {
                    if let Some(decision) = Self::parse_decision_line(line) {
                        self.decisions.push(decision);
                    }
                }
            }
            "Agent Behavior Log" => {
                for line in lines {
                    if let Some(entry) = Self::parse_behavior_line(line) {
                        self.behavior_log.push(entry);
                    }
                }
            }
            _ => {}
        }
        Ok(())
    }

    fn parse_instruction_line(line: &str) -> Option<UserInstruction> {
        let line = line.trim();
        if !line.starts_with("- [") {
            return None;
        }

        // Parse: - [2024-01-01 10:00] content
        let end_bracket = line.find(']')?;
        let timestamp_str = &line[3..end_bracket];
        let content = line[end_bracket + 1..].trim().to_string();

        let timestamp = DateTime::parse_from_str(
            &format!("{} +0000", timestamp_str),
            "%Y-%m-%d %H:%M %z",
        )
        .ok()?
        .with_timezone(&Utc);

        Some(UserInstruction { timestamp, content })
    }

    fn parse_task_objective(lines: &[String]) -> Option<TaskObjective> {
        let mut main_goal = None;
        let mut constraints = Vec::new();

        for line in lines {
            let line = line.trim();
            if line.starts_with("- Main goal: ") || line.starts_with("- Main goal:") {
                main_goal = Some(line.split('：').nth(1).unwrap_or(line).trim().to_string());
            } else if line.starts_with("- Constraint: ") || line.starts_with("- Constraint:") {
                constraints.push(line.split('：').nth(1).unwrap_or(line).trim().to_string());
            } else if line.starts_with("- ") && main_goal.is_some() {
                constraints.push(line[2..].trim().to_string());
            }
        }

        main_goal.map(|goal| TaskObjective {
            main_goal: goal,
            constraints,
            started_at: Utc::now(),
        })
    }

    fn parse_decision_line(line: &str) -> Option<Decision> {
        let line = line.trim();
        if !line.starts_with("- ") {
            return None;
        }

        let content = &line[2..];
        // Try to parse "whatbecause  why" or "what, because why"
        let (what, why) = if let Some(pos) = content.find("because ") {
            (content[..pos].to_string(), content[pos + 9..].to_string())
        } else if let Some(pos) = content.find(", because") {
            (content[..pos].to_string(), content[pos + 10..].to_string())
        } else {
            (content.to_string(), String::new())
        };

        Some(Decision {
            timestamp: Utc::now(),
            what,
            why,
        })
    }

    fn parse_behavior_line(line: &str) -> Option<BehaviorEntry> {
        let line = line.trim();
        if !line.starts_with("- [") {
            return None;
        }

        let end_bracket = line.find(']')?;
        let action = line[end_bracket + 1..].trim().to_string();
        let was_corrected = action.contains("corrected") || action.contains("corrected");

        Some(BehaviorEntry {
            timestamp: Utc::now(),
            action,
            was_corrected,
        })
    }

    /// Save memory to file
    pub async fn save(&self) -> Result<()> {
        let content = self.to_markdown();

        let mut file = fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(&self.file_path)
            .await?;

        file.write_all(content.as_bytes()).await?;
        file.flush().await?;

        Ok(())
    }

    /// Convert memory to markdown format
    fn to_markdown(&self) -> String {
        let mut md = String::from("# Gugugaga Memory\n\n");

        // User instructions
        md.push_str("## UserCore Instructions\n");
        md.push_str("<!-- Never compacted or deleted -->\n");
        for instruction in &self.user_instructions {
            md.push_str(&format!(
                "- [{}] {}\n",
                instruction.timestamp.format("%Y-%m-%d %H:%M"),
                instruction.content
            ));
        }
        md.push('\n');

        // Current task
        md.push_str("## Current TaskObjective\n");
        md.push_str("<!-- Must be re-injected after each compact -->\n");
        if let Some(task) = &self.current_task {
            md.push_str(&format!("- Main goal: {}\n", task.main_goal));
            for constraint in &task.constraints {
                md.push_str(&format!("- Constraint: {}\n", constraint));
            }
        }
        md.push('\n');

        // Decisions
        md.push_str("## Key Decisions\n");
        md.push_str("<!-- Important why, not just what -->\n");
        for decision in &self.decisions {
            if decision.why.is_empty() {
                md.push_str(&format!("- {}\n", decision.what));
            } else {
                md.push_str(&format!("- {}because {}\n", decision.what, decision.why));
            }
        }
        md.push('\n');

        // Behavior log (last 20 entries)
        md.push_str("## Agent Behavior Log\n");
        md.push_str("<!-- Recent N entries for pattern detection -->\n");
        let start = self.behavior_log.len().saturating_sub(20);
        for entry in &self.behavior_log[start..] {
            let corrected_marker = if entry.was_corrected { " (corrected)" } else { "" };
            md.push_str(&format!(
                "- [{}] {}{}\n",
                entry.timestamp.format("%H:%M"),
                entry.action,
                corrected_marker
            ));
        }

        md
    }

    /// Clear ALL state for a fresh start. No cross-conversation persistence.
    ///
    /// Called when a new thread starts. Everything lives per-thread.
    pub async fn clear_all(&mut self) -> Result<()> {
        self.user_instructions.clear();
        self.current_task = None;
        self.decisions.clear();
        self.behavior_log.clear();
        self.conversation_history.clear();
        self.save().await
    }

    /// Record a user instruction
    pub async fn record_user_instruction(&mut self, content: &str) -> Result<()> {
        self.user_instructions.push(UserInstruction {
            timestamp: Utc::now(),
            content: content.to_string(),
        });
        self.save().await
    }

    /// Set the current task objective
    pub async fn set_task_objective(&mut self, main_goal: &str, constraints: Vec<String>) -> Result<()> {
        self.current_task = Some(TaskObjective {
            main_goal: main_goal.to_string(),
            constraints,
            started_at: Utc::now(),
        });
        self.save().await
    }

    /// Record a decision
    pub async fn record_decision(&mut self, what: &str, why: &str) -> Result<()> {
        self.decisions.push(Decision {
            timestamp: Utc::now(),
            what: what.to_string(),
            why: why.to_string(),
        });
        self.save().await
    }

    /// Record agent behavior
    pub async fn record_behavior(&mut self, action: &str, was_corrected: bool) -> Result<()> {
        self.behavior_log.push(BehaviorEntry {
            timestamp: Utc::now(),
            action: action.to_string(),
            was_corrected,
        });
        self.save().await
    }

    /// Build complete context for gugugaga LLM
    pub fn build_context(&self) -> String {
        let mut context = String::new();

        // User instructions
        if !self.user_instructions.is_empty() {
            context.push_str("=== UserCore Instructions ===\n");
            for instruction in &self.user_instructions {
                context.push_str(&format!("- {}\n", instruction.content));
            }
            context.push('\n');
        }

        // Current task
        if let Some(task) = &self.current_task {
            context.push_str("=== Current Task ===\n");
            context.push_str(&format!("Goal: {}\n", task.main_goal));
            for constraint in &task.constraints {
                context.push_str(&format!("Constraint: {}\n", constraint));
            }
            context.push('\n');
        }

        // Recent decisions
        if !self.decisions.is_empty() {
            context.push_str("=== Key Decisions ===\n");
            for decision in self.decisions.iter().rev().take(5) {
                context.push_str(&format!("- {}：{}\n", decision.what, decision.why));
            }
            context.push('\n');
        }

        // Recent conversation
        let recent = self.recent_conversation_str();
        if !recent.is_empty() {
            context.push_str("=== Recent Conversation ===\n");
            context.push_str(&recent);
            context.push('\n');
        }

        context
    }

    /// Get the file path
    pub fn file_path(&self) -> &Path {
        &self.file_path
    }

    /// Get recent behavior entries
    pub fn recent_behaviors(&self, count: usize) -> Vec<&BehaviorEntry> {
        let start = self.behavior_log.len().saturating_sub(count);
        self.behavior_log[start..].iter().collect()
    }
}
