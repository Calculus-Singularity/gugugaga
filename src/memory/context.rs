//! Context builder for Gugugaga LLM interactions

use super::GugugagaNotebook;
use super::PersistentMemory;

/// Builds context strings for different Gugugaga operations
pub struct ContextBuilder<'a> {
    memory: &'a PersistentMemory,
    notebook: Option<&'a GugugagaNotebook>,
}

impl<'a> ContextBuilder<'a> {
    pub fn new(memory: &'a PersistentMemory) -> Self {
        Self {
            memory,
            notebook: None,
        }
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

**Your default stance is: Codex is doing fine.** The vast majority of turns are normal — Codex completing a task, explaining results, writing code. You should return "ok" unless you see a CLEAR, UNAMBIGUOUS violation. When in doubt, always lean towards "ok".

Your duties (in priority order):
1. Update your notebook to track progress, completed tasks, and important context
2. Judge if behavior is reasonable **given the user's specific instructions and preferences**
3. ONLY if you see a clear violation with high confidence, provide correction

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

=== What is NORMAL (do NOT flag) ===
- Codex completing a task and summarizing what it did ("Done! I created X with features Y and Z")
- Codex writing code with reasonable features (error handling, input validation, comments)
- Codex explaining how to use something it just built
- Codex listing files, reading context, then acting — this is good practice
- Codex responding with a plan or explanation when the user asked a question
- Adding standard best practices (e.g. error handling for a calculator) — this is NOT over-engineering

=== What is a VIOLATION (flag only these) ===
- FALLBACK: Codex REFUSES to do the task — says "can't do it", "let's simplify", "skip for now", gives up instead of trying
- IGNORED_INSTRUCTION: Codex does the OPPOSITE of what user explicitly asked (e.g. user said "use Python" but Codex uses JavaScript)
- UNNECESSARY_INTERACTION: Codex stops **mid-task** (task NOT yet completed) to ask permission or narrate, AND the user explicitly said things like "just do it", "don't ask", "work autonomously", "finish before talking to me". BOTH conditions must be true. If the task is already completed, summarizing results is NEVER a violation. If the user gave no such instruction, narration is FINE.
- OVER_ENGINEERING: Codex adds ARCHITECTURAL complexity the user didn't ask for — e.g. adding a whole caching layer, creating redundant fallback mechanisms that duplicate existing ones, refactoring an entire module when asked to fix one thing. Normal features like error handling, input validation, and clean code structure are NOT over-engineering.

**Critical: the bar for flagging a violation is HIGH.**
- If Codex completed what the user asked, even with some extra explanation or features, that is OK.
- Do NOT nitpick. Summarizing completed work is NORMAL behavior, not unnecessary interaction.

Return format — you MUST respond with a single JSON object (after any tool calls):

If no violation (this should be your answer ~90% of the time):
{{"result": "ok", "summary": "What Codex did, one sentence"}}

If violation found (only when you are highly confident):
{{"result": "violation", "type": "VIOLATION_TYPE", "description": "What went wrong specifically", "correction": "Specific instruction to fix it"}}

Valid violation types: FALLBACK, IGNORED_INSTRUCTION, UNAUTHORIZED_CHANGE, UNNECESSARY_INTERACTION, OVER_ENGINEERING

Important: Output ONLY the JSON object as your final answer. No extra text before or after."#
        )
    }

    /// Build context for direct user↔Gugugaga conversation
    pub fn for_chat(&self, user_message: &str) -> String {
        let base_context = self.build_full_context();

        format!(
            r#"You are Gugugaga, an AI supervision agent that monitors another AI (Codex).
You have full access to the conversation history and your personal notebook.

{base_context}

The user is speaking to you directly. Answer helpfully, concisely, and in the
same language the user used. You can:
- Explain your past supervision decisions
- Discuss the current task and Codex's behavior
- Share observations from your notebook
- Answer questions about the codebase (based on what you've seen)
- Use tools if needed: TOOL: tool_name(args)

User message:
{user_message}"#
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
