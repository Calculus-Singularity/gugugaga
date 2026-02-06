//! Context Compaction for Gugugaga
//!
//! Implements intelligent context compression following Codex's patterns.
//! When context window is nearly full, generates a summary and replaces old history.

use crate::gugugaga_agent::Evaluator;
use crate::Result;
use super::context_manager::ContextManager;
use tracing::{info, warn};

/// Prompt sent to LLM to generate a handoff summary
pub const SUMMARIZATION_PROMPT: &str = r#"You are performing a CONTEXT CHECKPOINT COMPACTION. Create a handoff summary for yourself (the same AI) that will resume the task.

Include:
- Current progress and key decisions made
- Important context, constraints, or user preferences
- What remains to be done (clear next steps)
- Any critical data, examples, or references needed to continue
- Mistakes made and lessons learned

Be concise, structured, and focused on helping yourself seamlessly continue the work. Write in second person (you did X, you need to Y)."#;

/// Prefix added to summary messages to identify them
pub const SUMMARY_PREFIX: &str = "=== CONTEXT CHECKPOINT ===
A previous session worked on this task and produced a summary. Use this to build on the work already done and avoid duplicating effort:";

/// Warning message shown after compaction
pub const COMPACTION_WARNING: &str = "Heads up: Context was compacted due to length. Some earlier details may be summarized. Start fresh threads for new tasks when possible.";

/// Compaction manager
pub struct Compactor {
    evaluator: Evaluator,
}

impl Compactor {
    /// Create a new compactor with the given evaluator
    pub fn new(evaluator: Evaluator) -> Self {
        Self { evaluator }
    }

    /// Run compaction on the given context manager
    /// 
    /// Returns Ok(true) if compaction was performed, Ok(false) if not needed
    pub async fn run_if_needed(&self, context_manager: &mut ContextManager) -> Result<bool> {
        if !context_manager.needs_compaction() {
            return Ok(false);
        }

        info!("Context compaction triggered ({}% usage)", 
            context_manager.token_info().usage_percent() as u32);

        self.run(context_manager).await?;
        Ok(true)
    }

    /// Force run compaction
    pub async fn run(&self, context_manager: &mut ContextManager) -> Result<()> {
        // Build the summarization prompt with current context
        let current_context = context_manager.for_prompt_string();
        let prompt = format!(
            "{}\n\n=== CURRENT CONTEXT TO SUMMARIZE ===\n{}\n\n=== YOUR SUMMARY ===",
            SUMMARIZATION_PROMPT,
            current_context
        );

        // Call LLM to generate summary
        let summary = match self.evaluator.call_llm(&prompt).await {
            Ok(s) => s,
            Err(e) => {
                warn!("Compaction LLM call failed: {}. Using fallback.", e);
                self.generate_fallback_summary(context_manager)
            }
        };

        // Build new compacted history
        let new_items = context_manager.build_compacted_history(&summary);

        // Replace the conversation items
        context_manager.replace(new_items);

        info!("Context compaction complete. New token count: {}", 
            context_manager.estimate_token_count());

        Ok(())
    }

    /// Generate a fallback summary when LLM call fails
    fn generate_fallback_summary(&self, context_manager: &ContextManager) -> String {
        let user_messages = context_manager.collect_user_messages();
        let recent_count = user_messages.len().min(5);
        
        let mut summary = String::from("Previous session summary (auto-generated):\n");
        
        if !user_messages.is_empty() {
            summary.push_str(&format!(
                "- {} user messages processed\n",
                user_messages.len()
            ));
            
            if recent_count > 0 {
                summary.push_str("- Recent topics: ");
                let topics: Vec<&str> = user_messages.iter()
                    .rev()
                    .take(recent_count)
                    .map(|m| {
                        // Take first 50 chars as topic hint
                        let end = m.char_indices()
                            .nth(50)
                            .map(|(i, _)| i)
                            .unwrap_or(m.len());
                        &m[..end]
                    })
                    .collect();
                summary.push_str(&topics.join(", "));
                summary.push('\n');
            }
        }
        
        summary.push_str("- Note: Full LLM summarization failed, this is a minimal summary\n");
        
        summary
    }
}

/// Check if a message is a summary message
#[allow(dead_code)]
pub fn is_summary_message(content: &str) -> bool {
    content.starts_with(SUMMARY_PREFIX) || content.starts_with("=== CONTEXT CHECKPOINT ===")
}

/// Extract the actual summary content from a summary message
#[allow(dead_code)]
pub fn extract_summary_content(content: &str) -> &str {
    if let Some(pos) = content.find("\n\n") {
        // Skip the prefix
        &content[pos + 2..]
    } else {
        content
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_summary_message() {
        assert!(is_summary_message(SUMMARY_PREFIX));
        assert!(is_summary_message(&format!("{}\nSome summary", SUMMARY_PREFIX)));
        assert!(!is_summary_message("Regular message"));
    }

    #[test]
    fn test_extract_summary() {
        let content = format!("{}\n\nActual summary here", SUMMARY_PREFIX);
        let extracted = extract_summary_content(&content);
        assert_eq!(extracted, "Actual summary here");
    }
}
