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

Evaluate this request and respond with a single JSON object:

{{"action": "AUTO_REPLY", "content": "your reply"}}
  — for simple confirmations (e.g., "can I continue?")

{{"action": "CORRECT", "content": "correction content"}}
  — if Agent did something wrong

{{"action": "FORWARD_TO_USER"}}
  — if strategic user decision needed

Output ONLY the JSON object."#
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
1. Check if Codex violated rules (fallback, ignoring instructions, unnecessary interaction)
2. Judge if behavior is reasonable **given the user's specific instructions and preferences**
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

History tools (full conversation archive, never lost):
- TOOL: search_history("keyword") - Search all past conversations by keyword
- TOOL: read_recent("5") - Read the most recent N turns (default 5, max 20)
- TOOL: read_turn("3") - Read a specific turn by index (0-based)
- TOOL: history_stats() - Get total turn count and token usage

File system tools (read-only, for verification):
- TOOL: read_file("path") or read_file("path|offset|limit") - Read file content
- TOOL: glob("pattern") - Find files matching pattern (e.g., "*.rs", "src/**/*.ts")
- TOOL: shell("command") - Execute read-only shell command (rg, cat, ls, etc.)
- TOOL: rg("pattern") - Search code with ripgrep (shortcut for shell)
- TOOL: ls("path") - List directory contents

Violation types:
- FALLBACK: Saying "can't do it", "let's simplify", "skip for now", etc.
- IGNORED_INSTRUCTION: Violating explicit user instructions
- UNNECESSARY_INTERACTION: Stopping to narrate, explain plans, or ask for confirmation when the user wants autonomous work. Examples:
  * User said "don't interact until done", but Codex stops to say "Next I will..."
  * User said "just do it", but Codex asks "Shall I proceed?"
  * Codex outputs a plan/roadmap instead of just executing when user clearly wants action
  * Codex explains what it's about to do instead of doing it (play-by-play narration)
  This is ONLY a violation when user instructions indicate they want autonomous/uninterrupted work. If the user has NOT expressed such preference, explaining plans is acceptable.
- OVER_ENGINEERING: Adding unnecessary complexity, redundant mechanisms, or doing more than what was asked. Examples:
  * User asks for a simple LLM-based check, but Codex also adds hardcoded string-matching patterns "for safety" — this is redundant and shows distrust of the chosen approach
  * User asks to fix one thing, but Codex refactors the entire module "while we're at it"
  * Adding "fallback layers", "safety nets", or "pre-filters" that the user didn't ask for and that duplicate existing capabilities
  * Wrapping a solution in extra abstraction/indirection that adds no real value
  The key test: did the user ask for this? If not, and it adds complexity without clear necessity, it is over-engineering.

**Critical rules**:
1. Pay close attention to ALL user instructions in context. If the user expressed any intent for autonomous/uninterrupted work (e.g. "just do it", "don't ask", "work autonomously", "finish before talking to me"), then ANY mid-task narration, plan explanation, or confirmation request from Codex is a UNNECESSARY_INTERACTION violation — even if the narration itself sounds polite or helpful.
2. If the user explicitly chose an approach (e.g. "use LLM for this"), do NOT add redundant mechanisms using a different approach (e.g. regex matching). Trust the user's architectural decisions. Doing extra unrequested work that contradicts the user's design intent is a violation.

Return format — you MUST respond with a single JSON object (after any tool calls):

If no violation:
{{"result": "ok", "summary": "What Codex did, one sentence"}}

If violation found:
{{"result": "violation", "type": "VIOLATION_TYPE", "description": "What went wrong specifically", "correction": "Specific instruction to fix it"}}

Valid violation types: FALLBACK, IGNORED_INSTRUCTION, UNAUTHORIZED_CHANGE, UNNECESSARY_INTERACTION, OVER_ENGINEERING

Important: Output ONLY the JSON object as your final answer. No extra text before or after."#
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
