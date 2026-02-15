//! Context Compaction for Gugugaga — aligned with Codex's compaction model.
//!
//! When context window usage reaches 90%, performs a unified compaction:
//!   1. Preserve initial_context (notebook, instructions, task, decisions)
//!   2. Keep recent user messages (up to COMPACT_USER_MESSAGE_MAX_TOKENS)
//!   3. Generate an LLM summary of everything else
//!   4. Replace conversation_history with summary + recent user messages
//!
//! Reference: codex-rs/core/src/compact.rs

use crate::gugugaga_agent::Evaluator;
use crate::Result;
use super::persistent::{ConversationTurn, TurnRole};
use tracing::{info, warn};

/// Approximate bytes per token for context estimation
const APPROX_BYTES_PER_TOKEN: usize = 4;

/// Maximum tokens for user messages to keep after compaction (aligned with Codex)
const COMPACT_USER_MESSAGE_MAX_TOKENS: usize = 20_000;

/// Default context window size (tokens) — used when not configured
pub const DEFAULT_CONTEXT_WINDOW: usize = 128_000;

/// Prompt sent to LLM to generate a handoff summary (aligned with Codex's
/// `codex-rs/core/templates/compact/prompt.md`)
pub const COMPACT_PROMPT: &str = r#"You are performing a CONTEXT CHECKPOINT COMPACTION. Create a handoff summary for another LLM that will resume the task.

Include:
- Current progress and key decisions made
- Important context, constraints, or user preferences
- What remains to be done (clear next steps)
- Any critical data, examples, or references needed to continue

Be concise, structured, and focused on helping the next LLM seamlessly continue the work."#;

/// Prefix added to summary messages to identify them (aligned with Codex's
/// `codex-rs/core/templates/compact/summary_prefix.md`)
pub const SUMMARY_PREFIX: &str = "Another language model started to solve this problem and produced a summary of its thinking process. You also have access to the state of the tools that were used by that language model. Use this to build on the work that has already been done and avoid duplicating work. Here is the summary produced by the other language model, use the information in this summary to assist with your own analysis:";

/// Compaction utilities — all methods take `&Evaluator` as a parameter so we
/// don't need to duplicate or Arc-wrap the evaluator.
pub struct Compactor;

impl Compactor {
    /// Compact conversation history if token usage exceeds 90% of context_window.
    ///
    /// Returns `Ok(true)` if compaction was performed.
    pub async fn compact_history_if_needed(
        evaluator: &Evaluator,
        context_window: usize,
        total_token_usage: usize,
        history: &mut Vec<ConversationTurn>,
    ) -> Result<bool> {
        let threshold = context_window * 9 / 10; // 90%, aligned with Codex
        if total_token_usage < threshold {
            return Ok(false);
        }

        info!(
            "Context compaction triggered ({} / {} tokens, {}%)",
            total_token_usage,
            context_window,
            (total_token_usage * 100) / context_window.max(1)
        );

        Self::compact_history(evaluator, history).await?;
        Ok(true)
    }

    /// Force-run compaction on the given conversation history.
    ///
    /// Algorithm (aligned with Codex `compact.rs`):
    ///   1. Collect real user messages (exclude previous summaries).
    ///   2. Build prompt from full history + COMPACT_PROMPT.
    ///   3. Call LLM to generate a summary.
    ///   4. Replace history with: selected recent user messages + summary turn.
    pub async fn compact_history(
        evaluator: &Evaluator,
        history: &mut Vec<ConversationTurn>,
    ) -> Result<()> {
        if history.is_empty() {
            return Ok(());
        }

        // 1. Collect user messages (exclude old summaries)
        let user_messages: Vec<String> = history
            .iter()
            .filter_map(|turn| {
                if (turn.role == TurnRole::User || turn.role == TurnRole::UserToGugugaga) && !is_summary_message(&turn.content) {
                    Some(turn.content.clone())
                } else {
                    None
                }
            })
            .collect();

        // 2. Build full context string for summarization
        let history_text = history
            .iter()
            .map(|t| {
                let role = match t.role {
                    TurnRole::User => "User",
                    TurnRole::UserToGugugaga => "User (to Gugugaga)",
                    TurnRole::Codex => "Codex",
                    TurnRole::Gugugaga => "Gugugaga",
                };
                format!("[{}] {}", role, t.content)
            })
            .collect::<Vec<_>>()
            .join("\n\n");

        let prompt = format!(
            "{}\n\n=== CURRENT CONTEXT TO SUMMARIZE ===\n{}\n\n=== YOUR SUMMARY ===",
            COMPACT_PROMPT, history_text
        );

        // 3. Call LLM to generate summary
        let summary = match evaluator.call_llm(&prompt).await {
            Ok(s) => s,
            Err(e) => {
                warn!("Compaction LLM call failed: {}. Using fallback.", e);
                generate_fallback_summary(&user_messages)
            }
        };

        // 4. Build compacted history
        let new_history = build_compacted_history(&user_messages, &summary);

        let old_tokens: usize = history.iter().map(|t| t.tokens).sum();
        *history = new_history;
        let new_tokens: usize = history.iter().map(|t| t.tokens).sum();

        info!(
            "Context compaction complete: {} tokens -> {} tokens",
            old_tokens, new_tokens
        );

        Ok(())
    }

    /// Compact a list of tool results when they exceed the token budget.
    ///
    /// Returns `Ok(Some(compacted))` if compaction happened (tool_results replaced
    /// with a single summary string), `Ok(None)` if not needed.
    pub async fn compact_tool_results_if_needed(
        evaluator: &Evaluator,
        tool_results: &mut Vec<String>,
        max_tokens: usize,
    ) -> Result<bool> {
        let total_tokens: usize = tool_results
            .iter()
            .map(|r| r.len() / APPROX_BYTES_PER_TOKEN)
            .sum();

        if total_tokens < max_tokens {
            return Ok(false);
        }

        info!(
            "Tool results compaction triggered ({} tokens > {} limit)",
            total_tokens, max_tokens
        );

        let all_results = tool_results.join("\n---\n");
        let prompt = format!(
            "{}\n\n=== TOOL RESULTS TO SUMMARIZE ===\n{}\n\n=== YOUR SUMMARY ===",
            COMPACT_PROMPT, all_results
        );

        let summary = match evaluator.call_llm(&prompt).await {
            Ok(s) => format!("[Compacted tool results summary]\n{}", s),
            Err(e) => {
                warn!("Tool results compaction failed: {}. Keeping originals.", e);
                return Ok(false);
            }
        };

        // Replace all tool results with the summary
        tool_results.clear();
        tool_results.push(summary);
        Ok(true)
    }
}

/// Build compacted history: recent user messages + summary turn.
///
/// Aligned with Codex's `build_compacted_history_with_limit`.
fn build_compacted_history(
    user_messages: &[String],
    summary_text: &str,
) -> Vec<ConversationTurn> {
    let mut result = Vec::new();

    // Select recent user messages (newest first, up to token limit)
    let mut selected: Vec<String> = Vec::new();
    let mut remaining = COMPACT_USER_MESSAGE_MAX_TOKENS;

    for message in user_messages.iter().rev() {
        let tokens = message.len() / APPROX_BYTES_PER_TOKEN;
        if remaining == 0 {
            break;
        }
        if tokens <= remaining {
            selected.push(message.clone());
            remaining = remaining.saturating_sub(tokens);
        } else {
            // Truncate the message to fit
            let truncated_len = remaining * APPROX_BYTES_PER_TOKEN;
            let truncated = if truncated_len < message.len() {
                format!(
                    "{}... [truncated, ~{} tokens omitted]",
                    &message[..truncated_len],
                    tokens - remaining
                )
            } else {
                message.clone()
            };
            selected.push(truncated);
            break;
        }
    }
    selected.reverse();

    // Add selected user messages
    for msg in selected {
        let tokens = msg.len() / APPROX_BYTES_PER_TOKEN;
        result.push(ConversationTurn {
            timestamp: chrono::Utc::now(),
            role: TurnRole::User,
            content: msg,
            tokens,
        });
    }

    // Add summary turn
    let summary_content = if summary_text.is_empty() {
        "(no summary available)".to_string()
    } else {
        format!("{}\n{}", SUMMARY_PREFIX, summary_text)
    };
    let tokens = summary_content.len() / APPROX_BYTES_PER_TOKEN;
    result.push(ConversationTurn {
        timestamp: chrono::Utc::now(),
        role: TurnRole::Gugugaga,
        content: summary_content,
        tokens,
    });

    result
}

/// Check if a message is a compaction summary
pub fn is_summary_message(content: &str) -> bool {
    content.starts_with(SUMMARY_PREFIX)
        || content.starts_with("Another language model started to solve")
        || content.starts_with("[Compacted tool results summary]")
}

/// Generate a basic fallback summary when LLM call fails
fn generate_fallback_summary(user_messages: &[String]) -> String {
    let mut summary = String::from("Previous session summary (auto-generated, LLM unavailable):\n");

    summary.push_str(&format!(
        "- {} user messages processed\n",
        user_messages.len()
    ));

    if !user_messages.is_empty() {
        let recent_count = user_messages.len().min(5);
        summary.push_str("- Recent topics: ");
        let topics: Vec<&str> = user_messages
            .iter()
            .rev()
            .take(recent_count)
            .map(|m| {
                let end = m
                    .char_indices()
                    .nth(80)
                    .map(|(i, _)| i)
                    .unwrap_or(m.len());
                &m[..end]
            })
            .collect();
        summary.push_str(&topics.join("; "));
        summary.push('\n');
    }

    summary
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_summary_message() {
        assert!(is_summary_message(SUMMARY_PREFIX));
        assert!(is_summary_message(&format!("{}\nSome summary", SUMMARY_PREFIX)));
        assert!(is_summary_message("[Compacted tool results summary]\nfoo"));
        assert!(!is_summary_message("Regular message"));
    }

    #[test]
    fn test_build_compacted_history() {
        let user_msgs = vec!["Hello".to_string(), "Do something".to_string()];
        let summary = "Made progress on the task.";
        let history = build_compacted_history(&user_msgs, summary);

        // Should have 2 user messages + 1 summary
        assert_eq!(history.len(), 3);
        assert_eq!(history[0].role, TurnRole::User);
        assert_eq!(history[1].role, TurnRole::User);
        assert_eq!(history[2].role, TurnRole::Gugugaga);
        assert!(history[2].content.contains(SUMMARY_PREFIX));
        assert!(history[2].content.contains("Made progress"));
    }

    #[test]
    fn test_fallback_summary() {
        let msgs = vec!["Task A".to_string(), "Task B".to_string()];
        let summary = generate_fallback_summary(&msgs);
        assert!(summary.contains("2 user messages"));
        assert!(summary.contains("Task B"));
    }
}
