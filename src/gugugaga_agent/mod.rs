//! Gugugaga Agent module
//!
//! The gugugaga agent uses LLM to evaluate Codex behavior and decide actions.

mod evaluator;
mod responder;

pub use evaluator::{Evaluator, GugugagaThinking, ParsedResponse};
pub use responder::Responder;

use crate::memory::{
    ContextBuilder, Compactor, PersistentMemory, GugugagaNotebook,
    Priority, AttentionSource,
};
use crate::memory::compact::DEFAULT_CONTEXT_WINDOW;
use crate::rules::Violation;
use crate::Result;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::RwLock;
use tokio::process::Command;
use glob::glob;

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
    /// - After each LLM call with tool follow-up: compact tool results if too large
    ///
    /// If `event_tx` is provided, emits real-time `gugugaga/*` notifications so
    /// the TUI can display thinking and tool-call activity (like Codex does).
    pub async fn detect_violation(
        &self,
        agent_message: &str,
        event_tx: Option<&tokio::sync::mpsc::Sender<String>>,
    ) -> Result<CheckResult> {
        // Helper: fire-and-forget an event to the TUI
        let emit = |tx: &tokio::sync::mpsc::Sender<String>, method: &str, params: serde_json::Value| {
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

        let mut tool_results: Vec<String> = Vec::new();
        let mut last_thinking: Option<String>;
        // Token budget for accumulated tool results before compaction
        const TOOL_RESULTS_MAX_TOKENS: usize = 6_000;
        let mut iteration = 0u32;

        loop {
            iteration += 1;

            // Notify TUI that we're calling the LLM
            if let Some(tx) = event_tx {
                let label = if iteration == 1 {
                    "Analyzing Codex output...".to_string()
                } else {
                    format!("Analyzing (tool follow-up #{})...", iteration - 1)
                };
                emit(tx, "gugugaga/thinking", serde_json::json!({ "status": "thinking", "message": label }));
            }

            let prompt = {
                let memory = self.memory.read().await;
                let notebook = self.notebook.read().await;
                let context_builder = ContextBuilder::new(&memory).with_notebook(&notebook);
                let mut p = context_builder.for_violation_detection(agent_message);

                // Append tool results if any
                if !tool_results.is_empty() {
                    p.push_str("\n\n=== Tool call results ===\n");
                    p.push_str(&tool_results.join("\n"));
                }
                p
            };

            let started = std::time::Instant::now();
            let parsed = self.evaluator.call_llm_with_thinking(&prompt).await?;
            let llm_duration = started.elapsed();
            last_thinking = parsed.thinking.clone();
            let response = parsed.response.trim();

            // Emit thinking content if present
            if let (Some(tx), Some(thinking)) = (event_tx, &parsed.thinking) {
                emit(tx, "gugugaga/thinking", serde_json::json!({
                    "status": "thought",
                    "message": thinking,
                    "duration_ms": llm_duration.as_millis() as u64,
                }));
            }

            // Check for tool calls
            if let Some(tool_result) = self.execute_tool_call_with_events(response, event_tx).await {
                tool_results.push(tool_result);

                // ── Mid-loop compaction (aligned with Codex: after sampling, if needs follow-up) ──
                // Compact tool results if they've grown too large
                let _ = Compactor::compact_tool_results_if_needed(
                    &self.evaluator,
                    &mut tool_results,
                    TOOL_RESULTS_MAX_TOKENS,
                )
                .await;

                continue;
            }

            // Parse final response
            return self.parse_check_response(response, last_thinking);
        }
    }

    /// Execute a tool call, emitting events to the TUI if event_tx is provided.
    async fn execute_tool_call_with_events(
        &self,
        response: &str,
        event_tx: Option<&tokio::sync::mpsc::Sender<String>>,
    ) -> Option<String> {
        // Parse tool call first
        let tool_regex = regex::Regex::new(r"TOOL:\s*(\w+)\s*\((.+)\)").ok()?;
        let caps = tool_regex.captures(response)?;
        let tool_name = caps.get(1)?.as_str().to_string();
        let args = caps.get(2)?.as_str().trim().trim_matches('"').trim_matches('\'').to_string();

        // Notify TUI that a tool call is starting
        if let Some(tx) = event_tx {
            let msg = serde_json::json!({
                "method": "gugugaga/toolCall",
                "params": {
                    "status": "started",
                    "tool": tool_name,
                    "args": args,
                }
            }).to_string();
            let _ = tx.try_send(msg);
        }

        let started = std::time::Instant::now();
        let result = self.execute_tool_call(response).await;
        let duration = started.elapsed();

        // Notify TUI of tool result
        if let Some(tx) = event_tx {
            let output = result.as_deref().unwrap_or("(no result)");
            // Truncate for display
            let display_output = if output.len() > 500 {
                format!("{}...[truncated]", &output[..500])
            } else {
                output.to_string()
            };
            let msg = serde_json::json!({
                "method": "gugugaga/toolCall",
                "params": {
                    "status": "completed",
                    "tool": tool_name,
                    "args": args,
                    "output": display_output,
                    "duration_ms": duration.as_millis() as u64,
                    "success": result.is_some(),
                }
            }).to_string();
            let _ = tx.try_send(msg);
        }

        result
    }

    /// Execute a tool call if present in response
    async fn execute_tool_call(&self, response: &str) -> Option<String> {
        // Parse TOOL: command(args) format - use greedy match for args to handle nested content
        let tool_regex = regex::Regex::new(r"TOOL:\s*(\w+)\s*\((.+)\)").ok()?;
        let caps = tool_regex.captures(response)?;
        
        let tool_name = caps.get(1)?.as_str();
        let args = caps.get(2)?.as_str().trim().trim_matches('"').trim_matches('\'');

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
                                    format!("[{:?} @ {}] {}", t.role, t.timestamp.format("%H:%M"), preview)
                                })
                                .collect();
                            Some(format!(
                                "search_history(\"{}\"): {} results (showing first {})\n{}",
                                args, total, summaries.len(), summaries.join("\n---\n")
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
                                    format!("#{} [{:?} @ {}] {}", i, t.role, t.timestamp.format("%H:%M"), t.content)
                                })
                                .collect();
                            Some(format!(
                                "read_recent({}): {} turns\n{}",
                                n, turns.len(), formatted.join("\n---\n")
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
                    Ok(Some(turn)) => {
                        Some(format!(
                            "read_turn({}): [{:?} @ {}]\n{}",
                            index, turn.role, turn.timestamp.format("%H:%M:%S"), turn.content
                        ))
                    }
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
            "update_notebook" => {
                self.handle_update_notebook(args).await
            }
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
                    match notebook.add_completed(parts[0].trim().to_string(), parts[1].trim().to_string()).await {
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
                let priority = parts.get(1)
                    .map(|p| match p.trim().to_lowercase().as_str() {
                        "high" => Priority::High,
                        "low" => Priority::Low,
                        _ => Priority::Medium,
                    })
                    .unwrap_or(Priority::Medium);
                
                let mut notebook = self.notebook.write().await;
                match notebook.add_attention(content.clone(), AttentionSource::Inference, priority).await {
                    Ok(_) => Some(format!("add_attention: Added '{}'", content)),
                    Err(_) => Some("add_attention: Failed".to_string()),
                }
            }
            "notebook_mistake" => {
                // Format: what|how_corrected|lesson
                let parts: Vec<&str> = args.splitn(3, '|').collect();
                if parts.len() == 3 {
                    let mut notebook = self.notebook.write().await;
                    match notebook.record_mistake(
                        parts[0].trim().to_string(),
                        parts[1].trim().to_string(),
                        parts[2].trim().to_string(),
                    ).await {
                        Ok(_) => Some(format!("notebook_mistake: Recorded '{}'", parts[0].trim())),
                        Err(_) => Some("notebook_mistake: Failed".to_string()),
                    }
                } else {
                    Some("notebook_mistake: Format error, should be what|how_corrected|lesson".to_string())
                }
            }
            
            // === File system tools ===
            "read_file" => {
                // Format: path or path|offset|limit
                let parts: Vec<&str> = args.splitn(3, '|').collect();
                let path = parts[0].trim();
                let offset = parts.get(1).and_then(|s| s.trim().parse::<usize>().ok()).unwrap_or(1);
                let limit = parts.get(2).and_then(|s| s.trim().parse::<usize>().ok()).unwrap_or(100);
                
                match self.read_file_lines(path, offset, limit).await {
                    Ok(content) => Some(format!("read_file(\"{}\", {}, {}):\n{}", path, offset, limit, content)),
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
                            let display: Vec<String> = files.iter().take(20).map(|p| p.display().to_string()).collect();
                            let suffix = if files.len() > 20 { format!("\n... and {} more", files.len() - 20) } else { String::new() };
                            Some(format!("glob(\"{}\"): {} matches\n{}{}", args, files.len(), display.join("\n"), suffix))
                        }
                    }
                    Err(e) => Some(format!("glob(\"{}\"): Error: {}", args, e)),
                }
            }
            "shell" | "rg" | "grep" => {
                // Execute shell command (with safety restrictions)
                let cmd = if tool_name == "rg" {
                    format!("rg {}", args)
                } else if tool_name == "grep" {
                    format!("rg {}", args) // Use rg instead of grep
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
    async fn read_file_lines(&self, path: &str, offset: usize, limit: usize) -> std::result::Result<String, String> {
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
        // Try to parse as JSON
        let json: serde_json::Value = match serde_json::from_str(args) {
            Ok(v) => v,
            Err(_) => {
                // Fallback: treat as activity update
                let mut notebook = self.notebook.write().await;
                let _ = notebook.set_current_activity(Some(args.to_string())).await;
                return Some(format!("update_notebook: Activity set to '{}'", args));
            }
        };
        
        let mut notebook = self.notebook.write().await;
        let mut results = Vec::new();
        
        // Handle current_activity
        if let Some(activity) = json.get("current_activity").and_then(|v| v.as_str()) {
            if notebook.set_current_activity(Some(activity.to_string())).await.is_ok() {
                results.push(format!("activity: '{}'", activity));
            }
        }
        
        // Handle add_completed
        if let Some(completed) = json.get("add_completed") {
            let what = completed.get("what").and_then(|v| v.as_str()).unwrap_or("");
            let significance = completed.get("significance").and_then(|v| v.as_str()).unwrap_or("");
            if !what.is_empty() {
                if notebook.add_completed(what.to_string(), significance.to_string()).await.is_ok() {
                    results.push(format!("completed: '{}'", what));
                }
            }
        }
        
        // Handle add_attention
        if let Some(attention) = json.get("add_attention") {
            let content = attention.get("content").and_then(|v| v.as_str()).unwrap_or("");
            let priority_str = attention.get("priority").and_then(|v| v.as_str()).unwrap_or("medium");
            let priority = match priority_str.to_lowercase().as_str() {
                "high" => Priority::High,
                "low" => Priority::Low,
                _ => Priority::Medium,
            };
            if !content.is_empty() {
                if notebook.add_attention(content.to_string(), AttentionSource::Inference, priority).await.is_ok() {
                    results.push(format!("attention: '{}'", content));
                }
            }
        }
        
        // Handle record_mistake
        if let Some(mistake) = json.get("record_mistake") {
            let what = mistake.get("what").and_then(|v| v.as_str()).unwrap_or("");
            let how = mistake.get("how_corrected").and_then(|v| v.as_str()).unwrap_or("");
            let lesson = mistake.get("lesson").and_then(|v| v.as_str()).unwrap_or("");
            if !what.is_empty() && !lesson.is_empty() {
                if notebook.record_mistake(what.to_string(), how.to_string(), lesson.to_string()).await.is_ok() {
                    results.push(format!("mistake: '{}'", what));
                }
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
            "cat" | "cd" | "cut" | "echo" | "expr" | "false" | "grep" | "head" | 
            "id" | "ls" | "nl" | "paste" | "pwd" | "rev" | "seq" | "stat" | 
            "tail" | "tr" | "true" | "uname" | "uniq" | "wc" | "which" | "whoami" => true,
            
            // Linux-specific (Codex allows on Linux)
            "numfmt" | "tac" => cfg!(target_os = "linux"),
            
            // base64 - safe without output options
            "base64" => {
                let unsafe_opts = ["-o", "--output"];
                !args.iter().any(|arg| {
                    unsafe_opts.contains(arg) || arg.starts_with("--output=") || (arg.starts_with("-o") && *arg != "-o")
                })
            }
            
            // find - safe without exec/delete options
            "find" => {
                let unsafe_opts = ["-exec", "-execdir", "-ok", "-okdir", "-delete", "-fls", "-fprint", "-fprint0", "-fprintf"];
                !args.iter().any(|arg| unsafe_opts.contains(arg))
            }
            
            // ripgrep - safe except for dangerous options
            "rg" => {
                !args.iter().any(|arg| {
                    *arg == "--search-zip" || *arg == "-z" ||
                    *arg == "--pre" || arg.starts_with("--pre=") ||
                    *arg == "--hostname-bin" || arg.starts_with("--hostname-bin=")
                })
            }
            
            // git - only safe subcommands (exactly as Codex)
            "git" => {
                matches!(args.get(1), Some(&"branch") | Some(&"status") | Some(&"log") | Some(&"diff") | Some(&"show"))
            }
            
            // cargo - only cargo check (exactly as Codex)
            "cargo" => {
                matches!(args.get(1), Some(&"check"))
            }
            
            // sed - only safe patterns like `sed -n 1,5p`
            "sed" => {
                args.len() <= 4 && args.get(1) == Some(&"-n") && 
                args.get(2).map_or(false, |arg| {
                    // Check if it matches pattern like "1p" or "1,5p"
                    arg.ends_with('p') && arg.trim_end_matches('p').chars().all(|c| c.is_ascii_digit() || c == ',')
                })
            }
            
            // Anything else is not allowed
            _ => false,
        }
    }

    /// Parse check response using JSON-first strategy with text fallback.
    /// Never returns Err — unparseable responses are treated as "OK".
    fn parse_check_response(&self, response: &str, thinking: Option<String>) -> Result<CheckResult> {
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
