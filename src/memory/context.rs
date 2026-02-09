//! Context builder for Gugugaga LLM interactions

use super::PersistentMemory;
use super::GugugagaNotebook;

/// Builds context strings for different Gugugaga operations
pub struct ContextBuilder<'a> {
    memory: &'a PersistentMemory,
    notebook: Option<&'a GugugagaNotebook>,
}

impl<'a> ContextBuilder<'a> {
    pub fn new(memory: &'a PersistentMemory) -> Self {
        Self { memory, notebook: None }
    }

    /// Add notebook context
    pub fn with_notebook(mut self, notebook: &'a GugugagaNotebook) -> Self {
        self.notebook = Some(notebook);
        self
    }

    /// Build combined context from memory and notebook
    fn build_full_context(&self) -> String {
        let mut context = String::new();
        
        // Notebook first (most important - persistent state)
        if let Some(notebook) = self.notebook {
            let notebook_str = notebook.to_prompt_string();
            if !notebook_str.is_empty() {
                context.push_str("=== Your Notebook (Persistent) ===\n");
                context.push_str(&notebook_str);
                context.push_str("\n\n");
            }
        }
        
        // Then memory context
        context.push_str(&self.memory.build_context());
        
        context
    }

    /// Build context for evaluating whether a request needs human intervention
    pub fn for_evaluation(&self, request_content: &str) -> String {
        let base_context = self.build_full_context();

        format!(
            r#"You are Gugugaga, the supervisor of Codex Agent. Your responsibilities:
1. Monitor Codex Agent behavior
2. Correct violations (fallbacks, ignoring user instructions)
3. Intelligently filter requests to reduce user interruptions

{base_context}

=== Current Request ===
{request_content}

Evaluate this request:
- If simple confirmation (e.g., "can I continue?"), return AUTO_REPLY: [your reply]
- If Agent did something wrong, return CORRECT: [correction content]
- If strategic user decision needed, return FORWARD_TO_USER

Output format: ACTION: [content]"#
        )
    }

    /// Build context for detecting violations
    pub fn for_violation_detection(&self, agent_message: &str) -> String {
        let base_context = self.build_full_context();

        // If no actual content, return simplified response
        if agent_message.trim().is_empty() {
            return "OK: No content".to_string();
        }

        format!(
            r#"You are Gugugaga, the supervisor Agent for Codex. You have your own notebook and memory, never forgetting important matters.

{base_context}

=== Codex Output This Turn ===
{agent_message}

Your duties:
1. Check if Codex violated rules (fallback, ignoring instructions, using builtin todo)
2. Judge if behavior is reasonable based on context
3. If violation found, provide **specific** correction instructions
4. Actively update your notebook to track progress, mistakes, and important context

Available tools (use as needed, can call multiple times):

Notebook tools (persistent, never lost):
- TOOL: update_notebook({{"current_activity": "...", "add_completed": {{"what": "...", "significance": "..."}}, "add_attention": {{"content": "...", "priority": "high|medium|low"}}, "record_mistake": {{"what": "...", "how_corrected": "...", "lesson": "..."}}}})
- TOOL: set_activity("what Codex is doing now")
- TOOL: clear_activity()
- TOOL: add_completed("what|significance")
- TOOL: add_attention("content|priority")
- TOOL: notebook_mistake("what|how_corrected|lesson")

Memory tools:
- TOOL: search_history("keyword") - Search conversation history
- TOOL: set_focus("current task description") - Update current focus
- TOOL: add_context("important info") - Add key context
- TOOL: record_mistake("issue|lesson") - Record mistakes

File system tools (read-only, for verification):
- TOOL: read_file("path") or read_file("path|offset|limit") - Read file content
- TOOL: glob("pattern") - Find files matching pattern (e.g., "*.rs", "src/**/*.ts")
- TOOL: shell("command") - Execute read-only shell command (rg, cat, ls, etc.)
- TOOL: rg("pattern") - Search code with ripgrep (shortcut for shell)
- TOOL: ls("path") - List directory contents

Severe violation types:
- FALLBACK: Saying "can't do it", "let's simplify", "skip for now", etc.
- IGNORED_INSTRUCTION: Violating explicit user instructions
Normal behavior: Asking questions, offering options, explaining, reporting

Return format (give final answer after tool calls):
- Violation: VIOLATION: [type] - [specific issue] - [specific correction instruction]
- Normal: OK: [What Codex did, one sentence]"#
        )
    }

    /// Build context for understanding user intent from input
    pub fn for_user_input_analysis(&self, user_input: &str) -> String {
        format!(
            r#"Analyze the following user input and extract key information:

{user_input}

Extract:
1. What is the main goal?
2. What are the constraints?
3. What are explicit instructions (must/don't/always)?

Output in JSON format:
{{
  "main_goal": "...",
  "constraints": ["...", "..."],
  "explicit_instructions": ["...", "..."]
}}"#
        )
    }
}
