//! Gugugaga Notebook - Persistent state for the supervisor agent
//!
//! This is the supervisor's own notebook, updated via tool calls.
//! It stores:
//! - What Codex is currently doing
//! - What has been completed
//! - Points that need attention
//! - Mistakes and lessons learned
//!
//! This notebook is NEVER affected by context compaction.

use crate::Result;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use tokio::fs;
use tokio::io::AsyncWriteExt;

/// Priority levels for attention items
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Priority {
    High,
    Medium,
    Low,
}

impl Default for Priority {
    fn default() -> Self {
        Self::Medium
    }
}

impl std::fmt::Display for Priority {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::High => write!(f, "high"),
            Self::Medium => write!(f, "medium"),
            Self::Low => write!(f, "low"),
        }
    }
}

/// Source of an attention item
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AttentionSource {
    /// From user instruction
    UserInstruction,
    /// From a past mistake
    Mistake,
    /// Inferred by the agent
    Inference,
}

impl Default for AttentionSource {
    fn default() -> Self {
        Self::Inference
    }
}

impl std::fmt::Display for AttentionSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UserInstruction => write!(f, "user"),
            Self::Mistake => write!(f, "mistake"),
            Self::Inference => write!(f, "inference"),
        }
    }
}

/// A completed item with significance
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompletedItem {
    pub timestamp: DateTime<Utc>,
    pub what: String,
    pub significance: String,
}

/// An item that needs attention
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AttentionItem {
    pub content: String,
    pub source: AttentionSource,
    pub priority: Priority,
    pub added_at: DateTime<Utc>,
}

/// A mistake entry with lesson learned
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MistakeEntry {
    pub timestamp: DateTime<Utc>,
    pub what_happened: String,
    pub how_corrected: String,
    pub lesson: String,
}

/// Gugugaga's personal notebook
///
/// This is independent of conversation history and never compacted.
/// The agent updates it via tool calls.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct GugugagaNotebook {
    /// What Codex is currently doing
    #[serde(default)]
    pub current_activity: Option<String>,

    /// Completed items with significance
    #[serde(default)]
    pub completed: Vec<CompletedItem>,

    /// Items that need attention
    #[serde(default)]
    pub attention: Vec<AttentionItem>,

    /// Mistakes and lessons learned
    #[serde(default)]
    pub mistakes: Vec<MistakeEntry>,

    /// Last update timestamp
    #[serde(default)]
    pub last_updated: Option<DateTime<Utc>>,

    /// File path for persistence
    #[serde(skip)]
    file_path: Option<PathBuf>,
}

impl GugugagaNotebook {
    /// Create a new notebook with persistence path
    pub async fn new(file_path: PathBuf) -> Result<Self> {
        let mut notebook = Self {
            file_path: Some(file_path.clone()),
            ..Default::default()
        };

        // Load if exists
        if file_path.exists() {
            notebook = notebook.load().await?;
        } else {
            // Create directory if needed
            if let Some(parent) = file_path.parent() {
                fs::create_dir_all(parent).await?;
            }
            notebook.save().await?;
        }

        Ok(notebook)
    }

    /// Create an in-memory notebook (no persistence)
    pub fn in_memory() -> Self {
        Self::default()
    }

    /// Set current activity
    pub async fn set_current_activity(&mut self, activity: Option<String>) -> Result<()> {
        self.current_activity = activity;
        self.last_updated = Some(Utc::now());
        self.save().await
    }

    /// Add a completed item
    pub async fn add_completed(&mut self, what: String, significance: String) -> Result<()> {
        self.completed.push(CompletedItem {
            timestamp: Utc::now(),
            what,
            significance,
        });
        self.last_updated = Some(Utc::now());

        // Keep only last 20 items
        if self.completed.len() > 20 {
            self.completed.remove(0);
        }

        self.save().await
    }

    /// Add an attention item
    pub async fn add_attention(
        &mut self,
        content: String,
        source: AttentionSource,
        priority: Priority,
    ) -> Result<()> {
        // Check if already exists
        if self.attention.iter().any(|a| a.content == content) {
            return Ok(());
        }

        self.attention.push(AttentionItem {
            content,
            source,
            priority,
            added_at: Utc::now(),
        });
        self.last_updated = Some(Utc::now());

        // Keep only last 30 items
        if self.attention.len() > 30 {
            self.attention.remove(0);
        }

        self.save().await
    }

    /// Remove an attention item by content
    pub async fn remove_attention(&mut self, content: &str) -> Result<bool> {
        let initial_len = self.attention.len();
        self.attention.retain(|a| a.content != content);

        if self.attention.len() != initial_len {
            self.last_updated = Some(Utc::now());
            self.save().await?;
            Ok(true)
        } else {
            Ok(false)
        }
    }

    /// Record a mistake
    pub async fn record_mistake(
        &mut self,
        what_happened: String,
        how_corrected: String,
        lesson: String,
    ) -> Result<()> {
        self.mistakes.push(MistakeEntry {
            timestamp: Utc::now(),
            what_happened: what_happened.clone(),
            how_corrected,
            lesson: lesson.clone(),
        });
        self.last_updated = Some(Utc::now());

        // Keep only last 15 mistakes
        if self.mistakes.len() > 15 {
            self.mistakes.remove(0);
        }

        // Also add to attention items
        self.add_attention(
            format!("Avoid: {}", lesson),
            AttentionSource::Mistake,
            Priority::High,
        )
        .await?;

        self.save().await
    }

    /// Build string for injection into LLM prompt
    pub fn to_prompt_string(&self) -> String {
        let mut parts = Vec::new();

        // Current activity
        if let Some(activity) = &self.current_activity {
            parts.push(format!("**Current Activity**: {}", activity));
        }

        // Recent completions (last 5)
        if !self.completed.is_empty() {
            let recent: Vec<String> = self
                .completed
                .iter()
                .rev()
                .take(5)
                .map(|c| format!("- {} ({})", c.what, c.significance))
                .collect();
            parts.push(format!("**Recent Progress**:\n{}", recent.join("\n")));
        }

        // Attention items by priority
        if !self.attention.is_empty() {
            let mut attention_lines = Vec::new();

            // High priority first
            for item in self
                .attention
                .iter()
                .filter(|a| a.priority == Priority::High)
            {
                attention_lines.push(format!("- [!] {} ({})", item.content, item.source));
            }
            // Then medium
            for item in self
                .attention
                .iter()
                .filter(|a| a.priority == Priority::Medium)
            {
                attention_lines.push(format!("- {} ({})", item.content, item.source));
            }
            // Skip low priority in prompt to save tokens

            if !attention_lines.is_empty() {
                parts.push(format!("**Watch Points**:\n{}", attention_lines.join("\n")));
            }
        }

        // Recent mistakes (last 3)
        if !self.mistakes.is_empty() {
            let recent: Vec<String> = self
                .mistakes
                .iter()
                .rev()
                .take(3)
                .map(|m| format!("- {}: {}", m.what_happened, m.lesson))
                .collect();
            parts.push(format!("**Past Mistakes**:\n{}", recent.join("\n")));
        }

        if parts.is_empty() {
            String::new()
        } else {
            parts.join("\n\n")
        }
    }

    /// Get summary for TUI display
    pub fn summary(&self) -> NotebookSummary {
        NotebookSummary {
            current_activity: self.current_activity.clone(),
            completed_count: self.completed.len(),
            attention_count: self.attention.len(),
            high_priority_count: self
                .attention
                .iter()
                .filter(|a| a.priority == Priority::High)
                .count(),
            mistakes_count: self.mistakes.len(),
            last_updated: self.last_updated,
        }
    }

    /// Load from file
    async fn load(self) -> Result<Self> {
        let path = self.file_path.clone();
        if let Some(file_path) = &path {
            let content = fs::read_to_string(file_path).await?;
            let mut loaded: Self = serde_json::from_str(&content)?;
            loaded.file_path = path;
            Ok(loaded)
        } else {
            Ok(self)
        }
    }

    /// Save to file
    pub async fn save(&self) -> Result<()> {
        if let Some(file_path) = &self.file_path {
            let content = serde_json::to_string_pretty(self)?;

            let mut file = fs::OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(true)
                .open(file_path)
                .await?;

            file.write_all(content.as_bytes()).await?;
            file.flush().await?;
        }
        Ok(())
    }

    /// Clear ALL data for a fresh start. No cross-conversation persistence.
    ///
    /// Called when a new thread starts. Everything lives per-thread.
    pub async fn clear_all(&mut self) -> Result<()> {
        self.current_activity = None;
        self.completed.clear();
        self.attention.clear();
        self.mistakes.clear();
        self.last_updated = Some(Utc::now());
        self.save().await
    }
}

/// Summary for TUI display
#[derive(Debug, Clone)]
pub struct NotebookSummary {
    pub current_activity: Option<String>,
    pub completed_count: usize,
    pub attention_count: usize,
    pub high_priority_count: usize,
    pub mistakes_count: usize,
    pub last_updated: Option<DateTime<Utc>>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_notebook_default() {
        let nb = GugugagaNotebook::default();
        assert!(nb.current_activity.is_none());
        assert!(nb.completed.is_empty());
        assert!(nb.attention.is_empty());
        assert!(nb.mistakes.is_empty());
    }

    #[test]
    fn test_prompt_string_empty() {
        let nb = GugugagaNotebook::default();
        assert!(nb.to_prompt_string().is_empty());
    }

    #[tokio::test]
    async fn test_in_memory_operations() {
        let mut nb = GugugagaNotebook::in_memory();

        nb.set_current_activity(Some("Testing".to_string()))
            .await
            .unwrap();
        assert_eq!(nb.current_activity.as_deref(), Some("Testing"));

        nb.add_completed("Test 1".to_string(), "Important".to_string())
            .await
            .unwrap();
        assert_eq!(nb.completed.len(), 1);

        nb.add_attention(
            "Watch this".to_string(),
            AttentionSource::UserInstruction,
            Priority::High,
        )
        .await
        .unwrap();
        assert_eq!(nb.attention.len(), 1);

        let prompt = nb.to_prompt_string();
        assert!(prompt.contains("Testing"));
        assert!(prompt.contains("Test 1"));
        assert!(prompt.contains("Watch this"));
    }
}
