//! Context Manager for Gugugaga
//!
//! Manages conversation history with intelligent compaction,
//! following Codex's context management patterns.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Approximate bytes per token for context estimation
const APPROX_BYTES_PER_TOKEN: usize = 4;

/// Default context window size (in tokens)
const DEFAULT_CONTEXT_WINDOW: usize = 128_000;

/// Threshold for triggering compaction (95% of context window)
const COMPACTION_THRESHOLD: f32 = 0.95;

/// Maximum tokens for user messages to keep after compaction
const COMPACT_USER_MESSAGE_MAX_TOKENS: usize = 20_000;

/// Conversation item types
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ConversationItem {
    /// System/initial context message
    System { content: String },
    /// User message
    User { content: String, timestamp: DateTime<Utc> },
    /// Codex agent output
    Codex { content: String, timestamp: DateTime<Utc> },
    /// Gugugaga supervisor message
    Gugugaga { content: String, timestamp: DateTime<Utc> },
    /// Compaction summary (replaces old history)
    Summary { content: String, timestamp: DateTime<Utc> },
}

impl ConversationItem {
    /// Get the content of this item
    pub fn content(&self) -> &str {
        match self {
            Self::System { content } => content,
            Self::User { content, .. } => content,
            Self::Codex { content, .. } => content,
            Self::Gugugaga { content, .. } => content,
            Self::Summary { content, .. } => content,
        }
    }

    /// Estimate token count for this item
    pub fn estimate_tokens(&self) -> usize {
        self.content().len() / APPROX_BYTES_PER_TOKEN
    }

    /// Check if this is a user message
    pub fn is_user(&self) -> bool {
        matches!(self, Self::User { .. })
    }

    /// Check if this is a summary message
    pub fn is_summary(&self) -> bool {
        matches!(self, Self::Summary { .. })
    }

    /// Get role name for display
    pub fn role_name(&self) -> &'static str {
        match self {
            Self::System { .. } => "System",
            Self::User { .. } => "User",
            Self::Codex { .. } => "Codex",
            Self::Gugugaga { .. } => "Gugugaga",
            Self::Summary { .. } => "Summary",
        }
    }
}

/// Token usage information
#[derive(Debug, Clone, Default)]
pub struct TokenUsageInfo {
    /// Estimated tokens used
    pub tokens_used: usize,
    /// Context window size
    pub context_window: usize,
}

impl TokenUsageInfo {
    /// Calculate usage percentage
    pub fn usage_percent(&self) -> f32 {
        if self.context_window == 0 {
            return 0.0;
        }
        (self.tokens_used as f32 / self.context_window as f32) * 100.0
    }

    /// Check if compaction is needed
    pub fn needs_compaction(&self) -> bool {
        self.tokens_used as f32 >= (self.context_window as f32 * COMPACTION_THRESHOLD)
    }
}

/// Context Manager - manages conversation history with compaction support
#[derive(Debug, Clone)]
pub struct ContextManager {
    /// Conversation items (oldest first)
    items: Vec<ConversationItem>,
    /// Initial context items (system instructions, user instructions)
    /// These are never compacted
    initial_context: Vec<ConversationItem>,
    /// Token usage information
    token_info: TokenUsageInfo,
}

impl ContextManager {
    /// Create a new context manager
    pub fn new() -> Self {
        Self {
            items: Vec::new(),
            initial_context: Vec::new(),
            token_info: TokenUsageInfo {
                tokens_used: 0,
                context_window: DEFAULT_CONTEXT_WINDOW,
            },
        }
    }

    /// Set the context window size
    pub fn set_context_window(&mut self, size: usize) {
        self.token_info.context_window = size;
    }

    /// Get current token usage info
    pub fn token_info(&self) -> &TokenUsageInfo {
        &self.token_info
    }

    /// Check if compaction is needed
    pub fn needs_compaction(&self) -> bool {
        self.token_info.needs_compaction()
    }

    /// Add initial context (never compacted)
    pub fn add_initial_context(&mut self, item: ConversationItem) {
        let tokens = item.estimate_tokens();
        self.initial_context.push(item);
        self.token_info.tokens_used += tokens;
    }

    /// Record new conversation items
    pub fn record_items(&mut self, items: &[ConversationItem]) {
        for item in items {
            let tokens = item.estimate_tokens();
            self.items.push(item.clone());
            self.token_info.tokens_used += tokens;
        }
    }

    /// Record a single user message
    pub fn record_user(&mut self, content: String) {
        self.record_items(&[ConversationItem::User {
            content,
            timestamp: Utc::now(),
        }]);
    }

    /// Record a single Codex message
    pub fn record_codex(&mut self, content: String) {
        self.record_items(&[ConversationItem::Codex {
            content,
            timestamp: Utc::now(),
        }]);
    }

    /// Record a single Gugugaga message
    pub fn record_gugugaga(&mut self, content: String) {
        self.record_items(&[ConversationItem::Gugugaga {
            content,
            timestamp: Utc::now(),
        }]);
    }

    /// Estimate total token count
    pub fn estimate_token_count(&self) -> usize {
        let initial_tokens: usize = self.initial_context.iter()
            .map(|i| i.estimate_tokens())
            .sum();
        let items_tokens: usize = self.items.iter()
            .map(|i| i.estimate_tokens())
            .sum();
        initial_tokens + items_tokens
    }

    /// Recompute token usage
    pub fn recompute_token_usage(&mut self) {
        self.token_info.tokens_used = self.estimate_token_count();
    }

    /// Get items for LLM prompt (normalized)
    pub fn for_prompt(&self) -> Vec<&ConversationItem> {
        let mut result: Vec<&ConversationItem> = Vec::new();
        
        // Initial context first
        for item in &self.initial_context {
            result.push(item);
        }
        
        // Then conversation items
        for item in &self.items {
            result.push(item);
        }
        
        result
    }

    /// Get items as formatted string for prompt
    pub fn for_prompt_string(&self) -> String {
        self.for_prompt()
            .iter()
            .map(|item| format!("[{}] {}", item.role_name(), item.content()))
            .collect::<Vec<_>>()
            .join("\n\n")
    }

    /// Get raw items reference
    pub fn raw_items(&self) -> &[ConversationItem] {
        &self.items
    }

    /// Get initial context reference
    pub fn initial_context(&self) -> &[ConversationItem] {
        &self.initial_context
    }

    /// Remove the first (oldest) item
    pub fn remove_first_item(&mut self) {
        if !self.items.is_empty() {
            let removed = self.items.remove(0);
            self.token_info.tokens_used = self.token_info.tokens_used
                .saturating_sub(removed.estimate_tokens());
        }
    }

    /// Replace all conversation items (used after compaction)
    pub fn replace(&mut self, items: Vec<ConversationItem>) {
        self.items = items;
        self.recompute_token_usage();
    }

    /// Collect user messages from items
    pub fn collect_user_messages(&self) -> Vec<String> {
        self.items
            .iter()
            .filter_map(|item| {
                if let ConversationItem::User { content, .. } = item {
                    // Skip summary messages
                    if !content.starts_with(super::compact::SUMMARY_PREFIX) {
                        return Some(content.clone());
                    }
                }
                None
            })
            .collect()
    }

    /// Build compacted history
    /// 
    /// Returns a new set of items that includes:
    /// 1. Initial context (system instructions)
    /// 2. Recent user messages (up to max tokens)
    /// 3. Summary message
    pub fn build_compacted_history(&self, summary: &str) -> Vec<ConversationItem> {
        let mut result = Vec::new();
        
        // Collect user messages with token limit
        let user_messages = self.collect_user_messages();
        let mut selected_messages: Vec<String> = Vec::new();
        let mut remaining_tokens = COMPACT_USER_MESSAGE_MAX_TOKENS;
        
        // Take most recent messages first
        for message in user_messages.iter().rev() {
            let tokens = message.len() / APPROX_BYTES_PER_TOKEN;
            if tokens <= remaining_tokens {
                selected_messages.push(message.clone());
                remaining_tokens = remaining_tokens.saturating_sub(tokens);
            } else if remaining_tokens > 0 {
                // Truncate the message to fit
                let truncated_len = remaining_tokens * APPROX_BYTES_PER_TOKEN;
                let truncated = format!(
                    "{}... [truncated, {} tokens]",
                    &message[..truncated_len.min(message.len())],
                    tokens - remaining_tokens
                );
                selected_messages.push(truncated);
                break;
            } else {
                break;
            }
        }
        selected_messages.reverse();
        
        // Add selected user messages
        for message in selected_messages {
            result.push(ConversationItem::User {
                content: message,
                timestamp: Utc::now(),
            });
        }
        
        // Add summary
        let summary_content = if summary.is_empty() {
            "(no summary available)".to_string()
        } else {
            format!("{}\n{}", super::compact::SUMMARY_PREFIX, summary)
        };
        
        result.push(ConversationItem::Summary {
            content: summary_content,
            timestamp: Utc::now(),
        });
        
        result
    }

    /// Get count of items
    pub fn len(&self) -> usize {
        self.items.len()
    }

    /// Check if empty
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }
}

impl Default for ContextManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_token_estimation() {
        let item = ConversationItem::User {
            content: "hello world".to_string(), // 11 bytes = ~2-3 tokens
            timestamp: Utc::now(),
        };
        assert!(item.estimate_tokens() > 0);
    }

    #[test]
    fn test_record_and_retrieve() {
        let mut mgr = ContextManager::new();
        mgr.record_user("Hello".to_string());
        mgr.record_codex("Hi there".to_string());
        
        assert_eq!(mgr.len(), 2);
        assert!(!mgr.is_empty());
    }

    #[test]
    fn test_compaction_threshold() {
        let info = TokenUsageInfo {
            tokens_used: 96_000,
            context_window: 100_000,
        };
        assert!(info.needs_compaction()); // 96% > 95%
        
        let info2 = TokenUsageInfo {
            tokens_used: 90_000,
            context_window: 100_000,
        };
        assert!(!info2.needs_compaction()); // 90% < 95%
    }
}
