//! Gugugaga Agent module
//!
//! The gugugaga agent uses LLM to evaluate Codex behavior and decide actions.

mod evaluator;
mod responder;

pub use evaluator::{
    Evaluator, GugugagaThinking, ParsedResponse, StructuredToolCall, StructuredTurnResponse,
};
pub use responder::Responder;

use crate::memory::compact::DEFAULT_CONTEXT_WINDOW;
use crate::memory::{
    AttentionSource, Compactor, ContextBuilder, GugugagaNotebook, PersistentMemory, Priority,
};
use crate::rules::Violation;
use crate::Result;
use glob::glob;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::process::Command;
use tokio::sync::RwLock;

/// Result of violation detection
#[derive(Debug, Clone)]
pub struct CheckResult {
    /// If a violation was found
    pub violation: Option<Violation>,
    /// Summary of what the LLM analyzed/concluded
    pub summary: String,
    /// Thinking/reasoning content from LLM (if any)
    pub thinking: Option<String>,
}

#[derive(Debug, Clone)]
struct ToolExecutionOutcome {
    call_id: String,
    tool_result: String,
}

struct ToolCallEventMeta<'a> {
    duration_ms: u64,
    success: bool,
    normalized_args: Option<&'a str>,
    normalized_error: Option<&'a str>,
    guarded: bool,
    duplicate: bool,
}

/// The gugugaga agent that monitors and corrects Codex behavior
pub struct GugugagaAgent {
    /// Evaluator for LLM-based evaluation
    evaluator: Evaluator,

    /// Auto-responder for handling requests
    responder: Responder,

    /// Persistent memory reference
    memory: Arc<RwLock<PersistentMemory>>,

    /// Gugugaga's personal notebook (never compacted)
    notebook: Arc<RwLock<GugugagaNotebook>>,
}

/// Result of evaluating a user input request
#[derive(Debug)]
pub enum EvaluationResult {
    /// Automatically reply to the request
    AutoReply(String),

    /// Correct the agent's behavior
    Correct(String),

    /// Forward to user for decision
    ForwardToUser,
}

impl GugugagaAgent {
    /// Create a new gugugaga agent
    pub async fn new(
        codex_home: &Path,
        memory: Arc<RwLock<PersistentMemory>>,
        notebook: Arc<RwLock<GugugagaNotebook>>,
    ) -> Result<Self> {
        let evaluator = Evaluator::new(codex_home).await?;
        let responder = Responder::new();

        Ok(Self {
            evaluator,
            responder,
            memory,
            notebook,
        })
    }

    /// Get the evaluator (for compaction)
    pub fn evaluator(&self) -> &Evaluator {
        &self.evaluator
    }

    /// Get notebook reference
    pub fn notebook(&self) -> Arc<RwLock<GugugagaNotebook>> {
        self.notebook.clone()
    }

    /// Direct chat with the user. Gugugaga answers using full context
    /// (memory, notebook, conversation history) and can use tools.
    pub async fn chat(
        &self,
        user_message: &str,
        event_tx: Option<&tokio::sync::mpsc::Sender<String>>,
    ) -> Result<String> {
        let emit =
            |tx: &tokio::sync::mpsc::Sender<String>, method: &str, params: serde_json::Value| {
                let msg = serde_json::json!({ "method": method, "params": params }).to_string();
                let _ = tx.try_send(msg);
            };

        let mut turn_items: Vec<serde_json::Value> = Vec::new();
        let mut executed_tool_signatures: HashSet<String> = HashSet::new();
        let mut executed_tool_calls: usize = 0;
        let mut iteration = 0u32;
        const MAX_FOLLOW_UP_ROUNDS: u32 = 8;
        const MAX_TOOL_CALLS_PER_TURN: usize = 24;

        loop {
            iteration += 1;
            if iteration > MAX_FOLLOW_UP_ROUNDS {
                let final_response =
                    "Stopping follow-up after guard limit; share remaining constraints explicitly."
                        .to_string();
                let mut mem = self.memory.write().await;
                let _ = mem
                    .add_turn(crate::memory::TurnRole::Gugugaga, final_response.clone())
                    .await;
                return Ok(final_response);
            }
            if let Some(tx) = event_tx {
                let label = if iteration == 1 {
                    "Thinking...".to_string()
                } else {
                    format!("Thinking (follow-up #{})...", iteration - 1)
                };
                emit(
                    tx,
                    "gugugaga/thinking",
                    serde_json::json!({ "status": "thinking", "message": label }),
                );
            }

            let prompt = {
                let memory = self.memory.read().await;
                let notebook = self.notebook.read().await;
                let context_builder = ContextBuilder::new(&memory).with_notebook(&notebook);
                context_builder.for_chat(user_message)
            };

            let started = std::time::Instant::now();
            let parsed = self
                .evaluator
                .call_llm_with_structured_tools(&prompt, &turn_items)
                .await?;
            let duration = started.elapsed();
            let response = parsed.response.trim().to_string();

            if let (Some(tx), Some(thinking)) = (event_tx, &parsed.thinking) {
                emit(
                    tx,
                    "gugugaga/thinking",
                    serde_json::json!({
                        "status": "thought",
                        "message": thinking,
                        "duration_ms": duration.as_millis() as u64,
                    }),
                );
            }

            if parsed.tool_calls.is_empty() {
                let mut mem = self.memory.write().await;
                let _ = mem
                    .add_turn(crate::memory::TurnRole::Gugugaga, response.clone())
                    .await;
                return Ok(response);
            }

            for tool_call in parsed.tool_calls {
                turn_items.push(tool_call.item.clone());
                let signature = format!("{}::{}", tool_call.tool_name, tool_call.arguments);

                if !executed_tool_signatures.insert(signature) {
                    let output = self.emit_guarded_tool_call_result(
                        &tool_call,
                        event_tx,
                        "duplicate tool call skipped in same turn; continue without re-running equivalent calls",
                        true,
                    );
                    turn_items.push(Evaluator::responses_function_output_item(
                        &output.call_id,
                        &output.tool_result,
                    ));
                    continue;
                }

                if executed_tool_calls >= MAX_TOOL_CALLS_PER_TURN {
                    let output = self.emit_guarded_tool_call_result(
                        &tool_call,
                        event_tx,
                        "tool-call limit reached for this turn; finalize with currently available evidence",
                        false,
                    );
                    turn_items.push(Evaluator::responses_function_output_item(
                        &output.call_id,
                        &output.tool_result,
                    ));
                    continue;
                }

                executed_tool_calls += 1;
                let output = self
                    .execute_tool_call_with_events(&tool_call, event_tx)
                    .await
                    .unwrap_or_else(|| ToolExecutionOutcome {
                        call_id: tool_call.call_id.clone(),
                        tool_result: "tool execution failed".to_string(),
                    });
                turn_items.push(Evaluator::responses_function_output_item(
                    &output.call_id,
                    &output.tool_result,
                ));
            }
        }
    }

    /// Evaluate whether a request needs human intervention
    pub async fn evaluate_request(&self, request_content: &str) -> Result<EvaluationResult> {
        let memory = self.memory.read().await;
        let notebook = self.notebook.read().await;
        let context_builder = ContextBuilder::new(&memory).with_notebook(&notebook);
        let prompt = context_builder.for_evaluation(request_content);

        let response = self.evaluator.call_llm(&prompt).await?;
        self.responder.parse_evaluation_response(&response)
    }

    /// Detect violations in agent output with tool call support.
    ///
    /// Includes compaction aligned with Codex:
    /// - Before first LLM call: compact conversation history if token usage >= 90%
    /// - Continue follow-up requests when structured tool outputs are pending
    ///
    /// If `event_tx` is provided, emits real-time `gugugaga/*` notifications so
    /// the TUI can display thinking and tool-call activity (like Codex does).
    pub async fn detect_violation(
        &self,
        agent_message: &str,
        event_tx: Option<&tokio::sync::mpsc::Sender<String>>,
    ) -> Result<CheckResult> {
        // Helper: fire-and-forget an event to the TUI
        let emit =
            |tx: &tokio::sync::mpsc::Sender<String>, method: &str, params: serde_json::Value| {
                let msg = serde_json::json!({ "method": method, "params": params }).to_string();
                let _ = tx.try_send(msg);
            };

        // ── Pre-call compaction (aligned with Codex: check at turn start) ──
        {
            let mut memory = self.memory.write().await;
            let total_tokens = memory.history_token_usage();
            let _ = Compactor::compact_history_if_needed(
                &self.evaluator,
                DEFAULT_CONTEXT_WINDOW,
                total_tokens,
                memory.conversation_history_mut(),
            )
            .await;
        }

        let mut turn_items: Vec<serde_json::Value> = Vec::new();
        let mut executed_tool_signatures: HashSet<String> = HashSet::new();
        let mut executed_tool_calls: usize = 0;
        let mut last_thinking: Option<String> = None;
        let mut iteration = 0u32;
        const MAX_FOLLOW_UP_ROUNDS: u32 = 8;
        const MAX_TOOL_CALLS_PER_TURN: usize = 24;

        loop {
            iteration += 1;
            if iteration > MAX_FOLLOW_UP_ROUNDS {
                return self.parse_check_response(
                    r#"{"result":"ok","summary":"Supervisor reached follow-up guard limit and returned conservative OK."}"#,
                    last_thinking,
                );
            }

            // Notify TUI that we're calling the LLM
            if let Some(tx) = event_tx {
                let label = if iteration == 1 {
                    "Analyzing Codex output...".to_string()
                } else {
                    format!("Analyzing (tool follow-up #{})...", iteration - 1)
                };
                emit(
                    tx,
                    "gugugaga/thinking",
                    serde_json::json!({ "status": "thinking", "message": label }),
                );
            }

            let prompt = {
                let memory = self.memory.read().await;
                let notebook = self.notebook.read().await;
                let context_builder = ContextBuilder::new(&memory).with_notebook(&notebook);
                context_builder.for_violation_detection(agent_message)
            };

            let started = std::time::Instant::now();
            let parsed = self
                .evaluator
                .call_llm_with_structured_tools(&prompt, &turn_items)
                .await?;
            let llm_duration = started.elapsed();
            last_thinking = parsed.thinking.clone();
            let response = parsed.response.trim().to_string();

            // Emit thinking content if present
            if let (Some(tx), Some(thinking)) = (event_tx, &parsed.thinking) {
                emit(
                    tx,
                    "gugugaga/thinking",
                    serde_json::json!({
                        "status": "thought",
                        "message": thinking,
                        "duration_ms": llm_duration.as_millis() as u64,
                    }),
                );
            }

            if parsed.tool_calls.is_empty() {
                return self.parse_check_response(&response, last_thinking);
            }

            for tool_call in parsed.tool_calls {
                turn_items.push(tool_call.item.clone());
                let signature = format!("{}::{}", tool_call.tool_name, tool_call.arguments);

                if !executed_tool_signatures.insert(signature) {
                    let output = self.emit_guarded_tool_call_result(
                        &tool_call,
                        event_tx,
                        "duplicate tool call skipped in same turn; continue without re-running equivalent calls",
                        true,
                    );
                    turn_items.push(Evaluator::responses_function_output_item(
                        &output.call_id,
                        &output.tool_result,
                    ));
                    continue;
                }

                if executed_tool_calls >= MAX_TOOL_CALLS_PER_TURN {
                    let output = self.emit_guarded_tool_call_result(
                        &tool_call,
                        event_tx,
                        "tool-call limit reached for this turn; finalize with currently available evidence",
                        false,
                    );
                    turn_items.push(Evaluator::responses_function_output_item(
                        &output.call_id,
                        &output.tool_result,
                    ));
                    continue;
                }

                executed_tool_calls += 1;
                let output = self
                    .execute_tool_call_with_events(&tool_call, event_tx)
                    .await
                    .unwrap_or_else(|| ToolExecutionOutcome {
                        call_id: tool_call.call_id.clone(),
                        tool_result: "tool execution failed".to_string(),
                    });
                turn_items.push(Evaluator::responses_function_output_item(
                    &output.call_id,
                    &output.tool_result,
                ));
            }
        }
    }

    fn normalize_tool_arguments(
        tool_name: &str,
        raw_args: &str,
    ) -> std::result::Result<String, String> {
        let value: serde_json::Value = if raw_args.trim().is_empty() {
            serde_json::json!({})
        } else {
            serde_json::from_str(raw_args).map_err(|e| format!("invalid JSON: {}", e))?
        };

        let pick_str = |keys: &[&str]| -> Option<String> {
            keys.iter().find_map(|key| {
                value
                    .get(*key)
                    .and_then(|v| v.as_str())
                    .map(ToOwned::to_owned)
            })
        };
        let pick_usize = |keys: &[&str]| -> Option<usize> {
            keys.iter()
                .find_map(|key| value.get(*key).and_then(|v| v.as_u64()).map(|n| n as usize))
        };

        match tool_name {
            "search_history" => pick_str(&["query", "keyword", "text", "pattern"])
                .ok_or_else(|| "missing query".to_string()),
            "read_recent" => Ok(pick_usize(&["count", "n"]).unwrap_or(5).to_string()),
            "read_turn" => pick_usize(&["index"])
                .map(|v| v.to_string())
                .ok_or_else(|| "missing index".to_string()),
            "history_stats" | "clear_activity" => Ok(String::new()),
            "update_notebook" => {
                if value.is_object() {
                    Ok(value.to_string())
                } else {
                    Err("payload must be an object".to_string())
                }
            }
            "set_activity" => pick_str(&["activity"]).ok_or_else(|| "missing activity".to_string()),
            "add_completed" => {
                let what = pick_str(&["what"]).ok_or_else(|| "missing what".to_string())?;
                let significance = pick_str(&["significance"]).unwrap_or_default();
                Ok(format!("{}|{}", what, significance))
            }
            "add_attention" => {
                let content =
                    pick_str(&["content"]).ok_or_else(|| "missing content".to_string())?;
                let priority = pick_str(&["priority"]).unwrap_or_else(|| "medium".to_string());
                Ok(format!("{}|{}", content, priority))
            }
            "notebook_mistake" => {
                let what = pick_str(&["what"]).ok_or_else(|| "missing what".to_string())?;
                let how = pick_str(&["how_corrected"]).unwrap_or_default();
                let lesson = pick_str(&["lesson"]).ok_or_else(|| "missing lesson".to_string())?;
                Ok(format!("{}|{}|{}", what, how, lesson))
            }
            "read_file" => {
                let path = pick_str(&["path"]).ok_or_else(|| "missing path".to_string())?;
                let offset = pick_usize(&["offset"]).unwrap_or(1);
                let limit = pick_usize(&["limit"]).unwrap_or(100);
                if value.get("offset").is_some() || value.get("limit").is_some() {
                    Ok(format!("{}|{}|{}", path, offset, limit))
                } else {
                    Ok(path)
                }
            }
            "glob" => pick_str(&["pattern"]).ok_or_else(|| "missing pattern".to_string()),
            "shell" => pick_str(&["cmd", "command"]).ok_or_else(|| "missing cmd".to_string()),
            "rg" => pick_str(&["pattern"]).ok_or_else(|| "missing pattern".to_string()),
            "ls" => pick_str(&["path"]).ok_or_else(|| "missing path".to_string()),
            _ => {
                if let Some(s) = value.as_str() {
                    Ok(s.to_string())
                } else {
                    Ok(value.to_string())
                }
            }
        }
    }

    fn emit_tool_call_started_event(
        &self,
        tool_call: &StructuredToolCall,
        event_tx: Option<&tokio::sync::mpsc::Sender<String>>,
    ) {
        if let Some(tx) = event_tx {
            let msg = serde_json::json!({
                "method": "gugugaga/toolCall",
                "params": {
                    "status": "started",
                    "call_id": &tool_call.call_id,
                    "tool": &tool_call.tool_name,
                    "args": &tool_call.arguments,
                    "raw_item": &tool_call.item,
                }
            })
            .to_string();
            let _ = tx.try_send(msg);
        }
    }

    fn emit_tool_call_completed_event(
        &self,
        tool_call: &StructuredToolCall,
        event_tx: Option<&tokio::sync::mpsc::Sender<String>>,
        output: &str,
        meta: ToolCallEventMeta<'_>,
    ) {
        if let Some(tx) = event_tx {
            let display_output = if output.len() > 4000 {
                format!("{}...[truncated]", &output[..4000])
            } else {
                output.to_string()
            };

            let msg = serde_json::json!({
                "method": "gugugaga/toolCall",
                "params": {
                    "status": "completed",
                    "call_id": &tool_call.call_id,
                    "tool": &tool_call.tool_name,
                    "args": &tool_call.arguments,
                    "raw_item": &tool_call.item,
                    "normalized_args": meta.normalized_args,
                    "normalized_error": meta.normalized_error,
                    "output": display_output,
                    "duration_ms": meta.duration_ms,
                    "success": meta.success,
                    "guarded": meta.guarded,
                    "duplicate": meta.duplicate,
                }
            })
            .to_string();
            let _ = tx.try_send(msg);
        }
    }

    fn emit_guarded_tool_call_result(
        &self,
        tool_call: &StructuredToolCall,
        event_tx: Option<&tokio::sync::mpsc::Sender<String>>,
        output: &str,
        duplicate: bool,
    ) -> ToolExecutionOutcome {
        self.emit_tool_call_started_event(tool_call, event_tx);
        self.emit_tool_call_completed_event(
            tool_call,
            event_tx,
            output,
            ToolCallEventMeta {
                duration_ms: 0,
                success: false,
                normalized_args: None,
                normalized_error: None,
                guarded: true,
                duplicate,
            },
        );
        ToolExecutionOutcome {
            call_id: tool_call.call_id.clone(),
            tool_result: output.to_string(),
        }
    }

    /// Execute a structured tool call, emitting events to the TUI.
    async fn execute_tool_call_with_events(
        &self,
        tool_call: &StructuredToolCall,
        event_tx: Option<&tokio::sync::mpsc::Sender<String>>,
    ) -> Option<ToolExecutionOutcome> {
        self.emit_tool_call_started_event(tool_call, event_tx);

        let started = std::time::Instant::now();
        let normalized = Self::normalize_tool_arguments(&tool_call.tool_name, &tool_call.arguments);
        let (result, normalized_args, normalized_error) = match normalized {
            Ok(normalized_args) => (
                self.execute_tool_call(&tool_call.tool_name, &normalized_args)
                    .await,
                Some(normalized_args),
                None,
            ),
            Err(err) => (
                Some(format!(
                    "{}: Invalid arguments: {}",
                    tool_call.tool_name, err
                )),
                None,
                Some(err),
            ),
        };
        let duration = started.elapsed();

        let output_text = result.as_deref().unwrap_or("(no result)");
        self.emit_tool_call_completed_event(
            tool_call,
            event_tx,
            output_text,
            ToolCallEventMeta {
                duration_ms: duration.as_millis() as u64,
                success: result.is_some(),
                normalized_args: normalized_args.as_deref(),
                normalized_error: normalized_error.as_deref(),
                guarded: false,
                duplicate: false,
            },
        );

        result.map(|tool_result| ToolExecutionOutcome {
            call_id: tool_call.call_id.clone(),
            tool_result,
        })
    }

    /// Execute a tool call.
    async fn execute_tool_call(&self, tool_name: &str, args: &str) -> Option<String> {
        match tool_name {
            // === History query tools ===
            "search_history" => {
                let memory = self.memory.read().await;
                match memory.search_history(args).await {
                    Ok(results) => {
                        if results.is_empty() {
                            Some(format!("search_history(\"{}\"): No results", args))
                        } else {
                            let total = results.len();
                            let summaries: Vec<String> = results
                                .iter()
                                .take(10)
                                .map(|t| {
                                    let preview = &t.content[..t.content.len().min(500)];
                                    format!(
                                        "[{:?} @ {}] {}",
                                        t.role,
                                        t.timestamp.format("%H:%M"),
                                        preview
                                    )
                                })
                                .collect();
                            Some(format!(
                                "search_history(\"{}\"): {} results (showing first {})\n{}",
                                args,
                                total,
                                summaries.len(),
                                summaries.join("\n---\n")
                            ))
                        }
                    }
                    Err(_) => Some(format!("search_history(\"{}\"): Search failed", args)),
                }
            }
            "read_recent" => {
                let n: usize = args.parse().unwrap_or(5).min(20);
                let memory = self.memory.read().await;
                match memory.read_recent_turns(n).await {
                    Ok(turns) => {
                        if turns.is_empty() {
                            Some("read_recent: No history available".to_string())
                        } else {
                            let formatted: Vec<String> = turns
                                .iter()
                                .enumerate()
                                .map(|(i, t)| {
                                    format!(
                                        "#{} [{:?} @ {}] {}",
                                        i,
                                        t.role,
                                        t.timestamp.format("%H:%M"),
                                        t.content
                                    )
                                })
                                .collect();
                            Some(format!(
                                "read_recent({}): {} turns\n{}",
                                n,
                                turns.len(),
                                formatted.join("\n---\n")
                            ))
                        }
                    }
                    Err(e) => Some(format!("read_recent({}): Error: {}", n, e)),
                }
            }
            "read_turn" => {
                let index: usize = match args.parse() {
                    Ok(i) => i,
                    Err(_) => return Some(format!("read_turn(\"{}\"): Invalid index", args)),
                };
                let memory = self.memory.read().await;
                match memory.read_turn_at(index).await {
                    Ok(Some(turn)) => Some(format!(
                        "read_turn({}): [{:?} @ {}]\n{}",
                        index,
                        turn.role,
                        turn.timestamp.format("%H:%M:%S"),
                        turn.content
                    )),
                    Ok(None) => Some(format!("read_turn({}): Turn not found", index)),
                    Err(e) => Some(format!("read_turn({}): Error: {}", index, e)),
                }
            }
            "history_stats" => {
                let memory = self.memory.read().await;
                match memory.total_turns().await {
                    Ok(total) => {
                        let in_memory = memory.conversation_history.len();
                        let in_memory_tokens = memory.history_token_usage();
                        Some(format!(
                            "history_stats: {} total turns in archive, {} in memory ({} tokens)",
                            total, in_memory, in_memory_tokens
                        ))
                    }
                    Err(e) => Some(format!("history_stats: Error: {}", e)),
                }
            }
            // === Notebook tools (persistent, never compacted) ===
            "update_notebook" => self.handle_update_notebook(args).await,
            "set_activity" => {
                let mut notebook = self.notebook.write().await;
                match notebook.set_current_activity(Some(args.to_string())).await {
                    Ok(_) => Some(format!("set_activity: Activity set to '{}'", args)),
                    Err(_) => Some("set_activity: Failed to set activity".to_string()),
                }
            }
            "clear_activity" => {
                let mut notebook = self.notebook.write().await;
                match notebook.set_current_activity(None).await {
                    Ok(_) => Some("clear_activity: Activity cleared".to_string()),
                    Err(_) => Some("clear_activity: Failed".to_string()),
                }
            }
            "add_completed" => {
                // Format: what|significance
                let parts: Vec<&str> = args.splitn(2, '|').collect();
                if parts.len() == 2 {
                    let mut notebook = self.notebook.write().await;
                    match notebook
                        .add_completed(parts[0].trim().to_string(), parts[1].trim().to_string())
                        .await
                    {
                        Ok(_) => Some(format!("add_completed: Added '{}'", parts[0].trim())),
                        Err(_) => Some("add_completed: Failed".to_string()),
                    }
                } else {
                    Some("add_completed: Format error, should be what|significance".to_string())
                }
            }
            "add_attention" => {
                // Format: content|priority (priority: high, medium, low)
                let parts: Vec<&str> = args.splitn(2, '|').collect();
                let content = parts[0].trim().to_string();
                let priority = parts
                    .get(1)
                    .map(|p| match p.trim().to_lowercase().as_str() {
                        "high" => Priority::High,
                        "low" => Priority::Low,
                        _ => Priority::Medium,
                    })
                    .unwrap_or(Priority::Medium);

                let mut notebook = self.notebook.write().await;
                match notebook
                    .add_attention(content.clone(), AttentionSource::Inference, priority)
                    .await
                {
                    Ok(_) => Some(format!("add_attention: Added '{}'", content)),
                    Err(_) => Some("add_attention: Failed".to_string()),
                }
            }
            "notebook_mistake" => {
                // Format: what|how_corrected|lesson
                let parts: Vec<&str> = args.splitn(3, '|').collect();
                if parts.len() == 3 {
                    let mut notebook = self.notebook.write().await;
                    match notebook
                        .record_mistake(
                            parts[0].trim().to_string(),
                            parts[1].trim().to_string(),
                            parts[2].trim().to_string(),
                        )
                        .await
                    {
                        Ok(_) => Some(format!("notebook_mistake: Recorded '{}'", parts[0].trim())),
                        Err(_) => Some("notebook_mistake: Failed".to_string()),
                    }
                } else {
                    Some(
                        "notebook_mistake: Format error, should be what|how_corrected|lesson"
                            .to_string(),
                    )
                }
            }

            // === File system tools ===
            "read_file" => {
                // Format: path or path|offset|limit
                let parts: Vec<&str> = args.splitn(3, '|').collect();
                let path = parts[0].trim();
                let offset = parts
                    .get(1)
                    .and_then(|s| s.trim().parse::<usize>().ok())
                    .unwrap_or(1);
                let limit = parts
                    .get(2)
                    .and_then(|s| s.trim().parse::<usize>().ok())
                    .unwrap_or(100);

                match self.read_file_lines(path, offset, limit).await {
                    Ok(content) => Some(format!(
                        "read_file(\"{}\", {}, {}):\n{}",
                        path, offset, limit, content
                    )),
                    Err(e) => Some(format!("read_file(\"{}\"): Error: {}", path, e)),
                }
            }
            "glob" => {
                // Find files matching pattern
                match self.glob_files(args).await {
                    Ok(files) => {
                        if files.is_empty() {
                            Some(format!("glob(\"{}\"): No matches", args))
                        } else {
                            let display: Vec<String> = files
                                .iter()
                                .take(20)
                                .map(|p| p.display().to_string())
                                .collect();
                            let suffix = if files.len() > 20 {
                                format!("\n... and {} more", files.len() - 20)
                            } else {
                                String::new()
                            };
                            Some(format!(
                                "glob(\"{}\"): {} matches\n{}{}",
                                args,
                                files.len(),
                                display.join("\n"),
                                suffix
                            ))
                        }
                    }
                    Err(e) => Some(format!("glob(\"{}\"): Error: {}", args, e)),
                }
            }
            "shell" | "rg" | "grep" => {
                // Execute shell command (with safety restrictions)
                let cmd = if tool_name == "rg" || tool_name == "grep" {
                    // Use rg instead of grep.
                    format!("rg {}", args)
                } else {
                    args.to_string()
                };

                match self.execute_shell(&cmd).await {
                    Ok(output) => Some(format!("shell(\"{}\"):\n{}", cmd, output)),
                    Err(e) => Some(format!("shell(\"{}\"): Error: {}", cmd, e)),
                }
            }
            "ls" => {
                // List directory
                let cmd = format!("ls -la {}", args);
                match self.execute_shell(&cmd).await {
                    Ok(output) => Some(format!("ls(\"{}\"):\n{}", args, output)),
                    Err(e) => Some(format!("ls(\"{}\"): Error: {}", args, e)),
                }
            }
            _ => Some(format!("Unknown tool: {}", tool_name)),
        }
    }

    /// Read file lines with offset and limit
    async fn read_file_lines(
        &self,
        path: &str,
        offset: usize,
        limit: usize,
    ) -> std::result::Result<String, String> {
        let path = PathBuf::from(path);

        let content = tokio::fs::read_to_string(&path)
            .await
            .map_err(|e| format!("Failed to read file: {}", e))?;

        let lines: Vec<&str> = content.lines().collect();
        let start = offset.saturating_sub(1).min(lines.len());
        let end = (start + limit).min(lines.len());

        let result: Vec<String> = lines[start..end]
            .iter()
            .enumerate()
            .map(|(i, line)| format!("L{}: {}", start + i + 1, line))
            .collect();

        Ok(result.join("\n"))
    }

    /// Find files matching glob pattern
    async fn glob_files(&self, pattern: &str) -> std::result::Result<Vec<PathBuf>, String> {
        let pattern = if pattern.starts_with('/') || pattern.starts_with('.') {
            pattern.to_string()
        } else {
            format!("**/{}", pattern)
        };

        let mut files = Vec::new();
        for entry in glob(&pattern).map_err(|e| format!("Invalid pattern: {}", e))? {
            match entry {
                Ok(path) => files.push(path),
                Err(_) => continue,
            }
        }

        Ok(files)
    }

    /// Execute a shell command with safety restrictions (whitelist-based like Codex)
    async fn execute_shell(&self, cmd: &str) -> std::result::Result<String, String> {
        // Parse command into parts
        let parts: Vec<&str> = cmd.split_whitespace().collect();
        if parts.is_empty() {
            return Err("Empty command".to_string());
        }

        let program = std::path::Path::new(parts[0])
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or(parts[0]);

        // Whitelist-based safety check (like Codex's is_known_safe_command)
        if !self.is_safe_command(program, &parts) {
            return Err(format!("Command '{}' is not in the safe whitelist. Allowed: cat, ls, head, tail, wc, grep, rg, find (without -exec/-delete), git (status/log/diff/show/branch), file, stat, which, pwd, echo, tree", program));
        }

        let output = Command::new("sh")
            .arg("-c")
            .arg(cmd)
            .output()
            .await
            .map_err(|e| format!("Failed to execute: {}", e))?;

        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);

        // Limit output size
        let max_len = 4000;
        let mut result = String::new();

        if !stdout.is_empty() {
            let s = if stdout.len() > max_len {
                format!("{}...[truncated]", &stdout[..max_len])
            } else {
                stdout.to_string()
            };
            result.push_str(&s);
        }

        if !stderr.is_empty() {
            if !result.is_empty() {
                result.push_str("\n[stderr]\n");
            }
            let s = if stderr.len() > max_len {
                format!("{}...[truncated]", &stderr[..max_len])
            } else {
                stderr.to_string()
            };
            result.push_str(&s);
        }

        if result.is_empty() {
            result = "(no output)".to_string();
        }

        Ok(result)
    }

    /// Handle update_notebook tool with JSON format
    async fn handle_update_notebook(&self, args: &str) -> Option<String> {
        let trimmed = args.trim();
        // Try to parse as JSON
        let json: serde_json::Value = match serde_json::from_str(trimmed) {
            Ok(v) => v,
            Err(err) => {
                let looks_like_json = trimmed.starts_with('{')
                    || trimmed.starts_with('[')
                    || trimmed.starts_with("\"{")
                    || trimmed.starts_with("'{'");
                if looks_like_json {
                    return Some(format!(
                        "update_notebook: Invalid JSON payload (not applied): {}",
                        err
                    ));
                }
                // Fallback: treat as activity update
                let mut notebook = self.notebook.write().await;
                let _ = notebook
                    .set_current_activity(Some(trimmed.to_string()))
                    .await;
                return Some(format!("update_notebook: Activity set to '{}'", trimmed));
            }
        };

        let mut notebook = self.notebook.write().await;
        let mut results = Vec::new();

        // Handle current_activity
        if let Some(activity) = json.get("current_activity").and_then(|v| v.as_str()) {
            if notebook
                .set_current_activity(Some(activity.to_string()))
                .await
                .is_ok()
            {
                results.push(format!("activity: '{}'", activity));
            }
        }

        // Handle add_completed
        if let Some(completed) = json.get("add_completed") {
            let what = completed.get("what").and_then(|v| v.as_str()).unwrap_or("");
            let significance = completed
                .get("significance")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            if !what.is_empty()
                && notebook
                    .add_completed(what.to_string(), significance.to_string())
                    .await
                    .is_ok()
            {
                results.push(format!("completed: '{}'", what));
            }
        }

        // Handle add_attention
        if let Some(attention) = json.get("add_attention") {
            let content = attention
                .get("content")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let priority_str = attention
                .get("priority")
                .and_then(|v| v.as_str())
                .unwrap_or("medium");
            let priority = match priority_str.to_lowercase().as_str() {
                "high" => Priority::High,
                "low" => Priority::Low,
                _ => Priority::Medium,
            };
            if !content.is_empty()
                && notebook
                    .add_attention(content.to_string(), AttentionSource::Inference, priority)
                    .await
                    .is_ok()
            {
                results.push(format!("attention: '{}'", content));
            }
        }

        // Handle record_mistake
        if let Some(mistake) = json.get("record_mistake") {
            let what = mistake.get("what").and_then(|v| v.as_str()).unwrap_or("");
            let how = mistake
                .get("how_corrected")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let lesson = mistake.get("lesson").and_then(|v| v.as_str()).unwrap_or("");
            if !what.is_empty()
                && !lesson.is_empty()
                && notebook
                    .record_mistake(what.to_string(), how.to_string(), lesson.to_string())
                    .await
                    .is_ok()
            {
                results.push(format!("mistake: '{}'", what));
            }
        }

        if results.is_empty() {
            Some("update_notebook: No updates applied".to_string())
        } else {
            Some(format!("update_notebook: Updated {}", results.join(", ")))
        }
    }

    /// Check if a command is safe to execute (whitelist-based, matching Codex exactly)
    fn is_safe_command(&self, program: &str, args: &[&str]) -> bool {
        match program {
            // Codex's exact whitelist from is_known_safe_command
            "cat" | "cd" | "cut" | "echo" | "expr" | "false" | "grep" | "head" | "id" | "ls"
            | "nl" | "paste" | "pwd" | "rev" | "seq" | "stat" | "tail" | "tr" | "true"
            | "uname" | "uniq" | "wc" | "which" | "whoami" => true,

            // Linux-specific (Codex allows on Linux)
            "numfmt" | "tac" => cfg!(target_os = "linux"),

            // base64 - safe without output options
            "base64" => {
                let unsafe_opts = ["-o", "--output"];
                !args.iter().any(|arg| {
                    unsafe_opts.contains(arg)
                        || arg.starts_with("--output=")
                        || (arg.starts_with("-o") && *arg != "-o")
                })
            }

            // find - safe without exec/delete options
            "find" => {
                let unsafe_opts = [
                    "-exec", "-execdir", "-ok", "-okdir", "-delete", "-fls", "-fprint", "-fprint0",
                    "-fprintf",
                ];
                !args.iter().any(|arg| unsafe_opts.contains(arg))
            }

            // ripgrep - safe except for dangerous options
            "rg" => !args.iter().any(|arg| {
                *arg == "--search-zip"
                    || *arg == "-z"
                    || *arg == "--pre"
                    || arg.starts_with("--pre=")
                    || *arg == "--hostname-bin"
                    || arg.starts_with("--hostname-bin=")
            }),

            // git - only safe subcommands (exactly as Codex)
            "git" => {
                matches!(
                    args.get(1),
                    Some(&"branch")
                        | Some(&"status")
                        | Some(&"log")
                        | Some(&"diff")
                        | Some(&"show")
                )
            }

            // cargo - only cargo check (exactly as Codex)
            "cargo" => {
                matches!(args.get(1), Some(&"check"))
            }

            // sed - only safe patterns like `sed -n 1,5p`
            "sed" => {
                args.len() <= 4
                    && args.get(1) == Some(&"-n")
                    && args.get(2).is_some_and(|arg| {
                        // Check if it matches pattern like "1p" or "1,5p"
                        arg.ends_with('p')
                            && arg
                                .trim_end_matches('p')
                                .chars()
                                .all(|c| c.is_ascii_digit() || c == ',')
                    })
            }

            // Anything else is not allowed
            _ => false,
        }
    }

    /// Parse check response using JSON-first strategy with text fallback.
    /// Never returns Err — unparseable responses are treated as "OK".
    fn parse_check_response(
        &self,
        response: &str,
        thinking: Option<String>,
    ) -> Result<CheckResult> {
        let parsed = self.responder.parse_check_response(response);
        Ok(CheckResult {
            violation: parsed.violation,
            summary: parsed.summary,
            thinking,
        })
    }

    /// Analyze user input and extract key information
    pub async fn analyze_user_input(&self, user_input: &str) -> Result<UserInputAnalysis> {
        let memory = self.memory.read().await;
        let context_builder = ContextBuilder::new(&memory);
        let prompt = context_builder.for_user_input_analysis(user_input);

        let response = self.evaluator.call_llm(&prompt).await?;
        self.responder.parse_user_input_analysis(&response)
    }

    /// Record a correction in memory
    pub async fn record_correction(&self, action: &str) -> Result<()> {
        let mut memory = self.memory.write().await;
        memory.record_behavior(action, true).await
    }
}

/// Analysis result from user input
#[derive(Debug, Clone)]
pub struct UserInputAnalysis {
    pub main_goal: Option<String>,
    pub constraints: Vec<String>,
    pub explicit_instructions: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::GugugagaAgent;

    #[test]
    fn normalize_tool_arguments_maps_update_notebook_object() {
        let normalized = GugugagaAgent::normalize_tool_arguments(
            "update_notebook",
            r#"{"current_activity":"Reviewing"}"#,
        )
        .expect("should normalize");
        assert_eq!(normalized, r#"{"current_activity":"Reviewing"}"#);
    }

    #[test]
    fn normalize_tool_arguments_maps_read_file_window() {
        let normalized = GugugagaAgent::normalize_tool_arguments(
            "read_file",
            r#"{"path":"src/main.rs","offset":5,"limit":10}"#,
        )
        .expect("should normalize");
        assert_eq!(normalized, "src/main.rs|5|10");
    }

    #[test]
    fn normalize_tool_arguments_rejects_invalid_json() {
        let err = GugugagaAgent::normalize_tool_arguments("shell", "not-json")
            .expect_err("invalid arguments should fail");
        assert!(err.contains("invalid JSON"));
    }

    #[test]
    fn normalize_tool_arguments_maps_add_completed() {
        let normalized = GugugagaAgent::normalize_tool_arguments(
            "add_completed",
            r#"{"what":"done","significance":"important"}"#,
        )
        .expect("should normalize");
        assert_eq!(normalized, "done|important");
    }
}
