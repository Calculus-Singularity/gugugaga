//! Message interceptor for Codex app-server
//!
//! Starts app-server as a subprocess and intercepts all JSONL communication.

use crate::memory::{PersistentMemory, GugugagaNotebook, SessionStore};
use crate::memory::session_store;
use crate::protocol::{self, notifications};
use crate::rules::ViolationDetector;
use crate::gugugaga_agent::{EvaluationResult, GugugagaAgent};
use crate::{Result, GugugagaConfig, GugugagaError};
use serde_json::Value;
use std::collections::HashMap;
use std::process::Stdio;
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::{mpsc, RwLock};
use tracing::{debug, info, warn};

/// Message interceptor that wraps Codex app-server
pub struct Interceptor {
    config: GugugagaConfig,
    memory: Arc<RwLock<PersistentMemory>>,
    notebook: Arc<RwLock<GugugagaNotebook>>,
    gugugaga_agent: Arc<GugugagaAgent>,
    session_store: Arc<SessionStore>,
    /// Thread ID for the current session (set after thread/start response)
    current_thread_id: Arc<RwLock<Option<String>>>,
}

/// Action to take after intercepting a message
#[derive(Debug)]
pub enum InterceptAction {
    /// Forward the message unchanged
    Forward,
    /// Drop the message (don't forward)
    Drop,
    /// Replace with a different message
    Replace(String),
    /// Inject additional message(s) before forwarding
    InjectBefore(Vec<String>),
    /// Inject additional message(s) after forwarding
    InjectAfter(Vec<String>),
    /// Interrupt the agent with a correction (show to user)
    Interrupt(String),
    /// Send correction directly to Codex (as a new user message)
    CorrectAgent(String),
}

impl Interceptor {
    /// Get a reference to the notebook for sharing with other components (e.g. TUI)
    pub fn notebook(&self) -> Arc<RwLock<GugugagaNotebook>> {
        self.notebook.clone()
    }

    /// Create a new interceptor
    pub async fn new(config: GugugagaConfig) -> Result<Self> {
        // Initialize persistent memory — start completely clean.
        // All state lives per-thread; session store restores if resuming.
        let mut memory = PersistentMemory::new(config.memory_file.clone()).await?;
        memory.clear_all().await?;
        let memory = Arc::new(RwLock::new(memory));

        // Initialize notebook — also start completely clean.
        let notebook_path = config.memory_file.with_extension("notebook.json");
        let mut notebook = GugugagaNotebook::new(notebook_path).await?;
        notebook.clear_all().await?;
        let notebook = Arc::new(RwLock::new(notebook));

        // Initialize session store for per-thread state caching
        let project_dir = config.memory_file.parent().unwrap_or(std::path::Path::new("."));
        let session_store = SessionStore::new(project_dir).await?;
        // Clean up old sessions (keep the 50 most recent)
        let _ = session_store.cleanup(50).await;
        let session_store = Arc::new(session_store);

        // Initialize gugugaga agent
        let gugugaga_agent = GugugagaAgent::new(&config.codex_home, memory.clone(), notebook.clone()).await?;
        let gugugaga_agent = Arc::new(gugugaga_agent);

        Ok(Self {
            config,
            memory,
            notebook,
            gugugaga_agent,
            session_store,
            current_thread_id: Arc::new(RwLock::new(None)),
        })
    }

    /// Start the interceptor, spawning app-server and handling messages
    pub async fn run(
        &self,
        mut user_input_rx: mpsc::Receiver<String>,
        output_tx: mpsc::Sender<String>,
    ) -> Result<()> {
        // Start app-server subprocess
        let mut child = self.spawn_app_server().await?;

        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| GugugagaError::AppServerStart("Failed to get stdin".to_string()))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| GugugagaError::AppServerStart("Failed to get stdout".to_string()))?;

        let mut stdin = tokio::io::BufWriter::new(stdin);
        let mut stdout_reader = BufReader::new(stdout).lines();

        // Channel for messages to send to app-server
        let (to_server_tx, mut to_server_rx) = mpsc::channel::<String>(32);
        
        // Notify TUI that gugugaga is active
        let _ = output_tx.send(serde_json::json!({
            "method": "gugugaga/status",
            "params": {
                "message": "Gugugaga active. Monitoring Codex behavior.",
                "strictMode": self.config.strict_mode
            }
        }).to_string()).await;

        // Spawn task to read from app-server stdout
        let output_tx_clone = output_tx.clone();
        let to_server_tx_clone = to_server_tx.clone();
        let memory = self.memory.clone();
        let notebook = self.notebook.clone();
        let gugugaga_agent = self.gugugaga_agent.clone();
        let config = self.config.clone();
        let session_store = self.session_store.clone();
        let shared_thread_id = self.current_thread_id.clone();

        let stdout_task = tokio::spawn(async move {
            let violation_detector = ViolationDetector::new();
            // Accumulate agent message content per thread
            let mut thread_turn_content: HashMap<String, String> = HashMap::new();
            // Track current (main) thread ID for corrections
            let mut current_thread_id: Option<String> = None;
            // Whether we've already handled session init for the first thread_id
            let mut session_initialized = false;
            // Track all active threads (main + sub-agents)
            let mut active_threads: HashMap<String, String> = HashMap::new(); // id -> source/label
            // Legacy: single accumulator for when we don't know the thread
            let mut current_turn_content = String::new();
            // Channel to send corrections to Codex
            let correction_tx = to_server_tx_clone;

            while let Ok(Some(line)) = stdout_reader.next_line().await {
                if line.trim().is_empty() {
                    continue;
                }

                debug!("From app-server: {}", &line[..line.len().min(100)]);

                // Parse and process the message
                match serde_json::from_str::<Value>(&line) {
                    Ok(msg) => {
                        // Track threadId from thread/start or thread/resume response
                        // Format: { "result": { "thread": { "id": "xxx" } } }
                        if let Some(thread_id) = msg
                            .get("result")
                            .and_then(|r| r.get("thread"))
                            .and_then(|t| t.get("id"))
                            .and_then(|id| id.as_str())
                        {
                            // Per-thread session management: on first thread_id detection,
                            // either restore a cached session or start clean.
                            if !session_initialized {
                                session_initialized = true;
                                let tid = thread_id.to_string();

                                match session_store.load(&tid).await {
                                    Ok(Some(snapshot)) => {
                                        // Resuming an existing thread — restore its state
                                        let mut mem = memory.write().await;
                                        let mut nb = notebook.write().await;
                                        if let Err(e) = session_store::restore_snapshot(
                                            &mut mem, &mut nb, snapshot,
                                        ).await {
                                            warn!("Failed to restore session for {}: {}", tid, e);
                                        }
                                    }
                                    Ok(None) => {
                                        // New thread — reset session-scoped state so old
                                        // context doesn't bleed in.
                                        let mut mem = memory.write().await;
                                        let mut nb = notebook.write().await;
                                        let _ = mem.clear_all().await;
                                        let _ = nb.clear_all().await;
                                    }
                                    Err(e) => {
                                        warn!("Session store load error: {}", e);
                                    }
                                }

                                // Share the thread_id for session saving on exit
                                *shared_thread_id.write().await = Some(tid);
                            }

                            current_thread_id = Some(thread_id.to_string());
                            active_threads.insert(thread_id.to_string(), "main".to_string());
                            debug!("Tracking threadId: {}", thread_id);
                        }

                        // Extract per-notification threadId (many notifications include it)
                        let notif_thread_id = msg
                            .get("params")
                            .and_then(|p| p.get("threadId"))
                            .and_then(|t| t.as_str())
                            .map(|s| s.to_string());
                        
                        // Accumulate agent message content
                        let method = msg.get("method").and_then(|m| m.as_str()).unwrap_or("");
                        match method {
                            "turn/started" => {
                                // Reset accumulator at turn start (per-thread)
                                if let Some(tid) = &notif_thread_id {
                                    thread_turn_content.insert(tid.clone(), String::new());
                                } else {
                                    current_turn_content.clear();
                                }
                            }
                            "item/agentMessage/delta" => {
                                // Accumulate agent output (per-thread)
                                if let Some(delta) = msg.get("params").and_then(|p| p.get("delta")).and_then(|d| d.as_str()) {
                                    if let Some(tid) = &notif_thread_id {
                                        thread_turn_content.entry(tid.clone()).or_default().push_str(delta);
                                    }
                                    current_turn_content.push_str(delta);
                                }
                            }
                            // Track sub-agent spawns via item/completed with collabAgentToolCall
                            "item/completed" => {
                                if let Some(item) = msg.get("params").and_then(|p| p.get("item")) {
                                    if let Some(details) = item.get("details") {
                                        // Check for collab agent tool call
                                        if let Some(tool) = details.get("tool").and_then(|t| t.as_str()) {
                                            if tool == "spawnAgent" {
                                                if let Some(ids) = details.get("receiverThreadIds").and_then(|r| r.as_array()) {
                                                    for id in ids {
                                                        if let Some(tid) = id.as_str() {
                                                            active_threads.insert(tid.to_string(), "sub-agent".to_string());
                                                            debug!("Sub-agent spawned: {}", tid);
                                                        }
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                            // Track thread lifecycle
                            "thread/started" => {
                                if let Some(tid) = &notif_thread_id {
                                    if !active_threads.contains_key(tid.as_str()) {
                                        active_threads.insert(tid.clone(), "thread".to_string());
                                    }
                                }
                            }
                            _ => {}
                        }
                        
                        // Resolve per-thread content: prefer thread-specific, fallback to global
                        let effective_content = notif_thread_id
                            .as_ref()
                            .and_then(|tid| thread_turn_content.get(tid))
                            .map(|s| s.as_str())
                            .unwrap_or(&current_turn_content);

                        let action = Self::process_server_message(
                            &msg,
                            &memory,
                            &notebook,
                            &gugugaga_agent,
                            &violation_detector,
                            &config,
                            effective_content,
                        )
                        .await;

                        match action {
                            InterceptAction::Forward => {
                                if output_tx_clone.send(line).await.is_err() {
                                    break;
                                }
                            }
                            InterceptAction::Drop => {
                                debug!("Dropping message");
                            }
                            InterceptAction::Replace(new_msg) => {
                                if output_tx_clone.send(new_msg).await.is_err() {
                                    break;
                                }
                            }
                            InterceptAction::InjectBefore(msgs) => {
                                for m in msgs {
                                    if output_tx_clone.send(m).await.is_err() {
                                        break;
                                    }
                                }
                                if output_tx_clone.send(line).await.is_err() {
                                    break;
                                }
                            }
                            InterceptAction::InjectAfter(msgs) => {
                                // Forward original first
                                if output_tx_clone.send(line).await.is_err() {
                                    break;
                                }
                                // Then inject additional messages
                                for m in msgs {
                                    if output_tx_clone.send(m).await.is_err() {
                                        break;
                                    }
                                }
                            }
                            InterceptAction::Interrupt(correction) => {
                                // Send correction to user for display
                                let correction_msg = serde_json::json!({
                                    "method": "gugugaga/correction",
                                    "params": {
                                        "message": correction
                                    }
                                });
                                let _ = output_tx_clone
                                    .send(serde_json::to_string(&correction_msg).unwrap_or_default())
                                    .await;
                            }
                            InterceptAction::CorrectAgent(correction) => {
                                // Forward the original message first
                                if output_tx_clone.send(line).await.is_err() {
                                    break;
                                }
                                
                                // Need threadId to send correction
                                if let Some(thread_id) = &current_thread_id {
                                    // Send correction directly to Codex as a user message
                                    let correction_turn = serde_json::json!({
                                        "jsonrpc": "2.0",
                                        "method": "turn/start",
                                        "params": {
                                            "threadId": thread_id,
                                            "input": [{
                                                "type": "text",
                                                "text": correction,
                                                "textElements": []
                                            }]
                                        },
                                        "id": 9999
                                    });
                                    let _ = correction_tx.send(correction_turn.to_string()).await;
                                    // Also notify TUI briefly
                                    let notify = serde_json::json!({
                                        "method": "gugugaga/correction",
                                        "params": {
                                            "message": format!("🛡️ Corrected: {}", correction)
                                        }
                                    });
                                    let _ = output_tx_clone.send(notify.to_string()).await;
                                } else {
                                    // No threadId - just notify TUI
                                    let notify = serde_json::json!({
                                        "method": "gugugaga/correction",
                                        "params": {
                                            "message": format!("🛡️ Issue detected but cannot send correction（no threadId）: {}", correction)
                                        }
                                    });
                                    let _ = output_tx_clone.send(notify.to_string()).await;
                                }
                            }
                        }
                    }
                    Err(_) => {
                        // Non-JSON lines (e.g. tracing log output from app-server)
                        // — silently drop, do NOT forward to TUI.
                        debug!("Dropping non-JSON server output: {}", &line[..line.len().min(120)]);
                    }
                }
            }
        });

        // Spawn task to write to app-server stdin
        let stdin_task = tokio::spawn(async move {
            while let Some(msg) = to_server_rx.recv().await {
                if stdin.write_all(msg.as_bytes()).await.is_err() {
                    break;
                }
                if stdin.write_all(b"\n").await.is_err() {
                    break;
                }
                if stdin.flush().await.is_err() {
                    break;
                }
            }
        });

        // Main loop: receive user input and forward to app-server
        let to_server_tx_clone = to_server_tx.clone();
        let memory = self.memory.clone();

        while let Some(input) = user_input_rx.recv().await {
            // Process user input
            match serde_json::from_str::<Value>(&input) {
                Ok(msg) => {
                    // Record user message to memory if it's a turn start
                    if let Some(method) = msg.get("method").and_then(|m| m.as_str()) {
                        if method == protocol::methods::TURN_START {
                            if let Some(params) = msg.get("params") {
                                if let Some(input_arr) = params.get("input").and_then(|i| i.as_array()) {
                                    let mut mem = memory.write().await;
                                    for item in input_arr {
                                        if let Some(text) = item.get("text").and_then(|t| t.as_str()) {
                                            // Record to conversation history
                                            let _ = mem.add_turn(
                                                crate::memory::TurnRole::User,
                                                text.to_string()
                                            ).await;
                                            // Also record as instruction if applicable
                                            let _ = mem.record_user_instruction(text).await;
                                        }
                                    }
                                }
                            }
                        }
                    }

                    // Forward to app-server
                    if to_server_tx_clone.send(input).await.is_err() {
                        break;
                    }
                }
                Err(e) => {
                    warn!("Failed to parse user input: {}", e);
                    // Forward anyway
                    if to_server_tx_clone.send(input).await.is_err() {
                        break;
                    }
                }
            }
        }

        // Signal tasks to stop, then wait for them to finish
        drop(to_server_tx);
        let _ = stdout_task.await;
        let _ = stdin_task.await;

        // Save session AFTER all tasks are done, so we capture the final state
        // (including any mistakes/corrections from the last turn).
        {
            let thread_id = self.current_thread_id.read().await;
            if let Some(tid) = thread_id.as_ref() {
                let mem = self.memory.read().await;
                let nb = self.notebook.read().await;
                if let Err(e) = self.session_store.save(tid, &mem, &nb).await {
                    warn!("Failed to save session for {}: {}", tid, e);
                } else {
                    info!("Saved session state for thread {}", tid);
                }
            }
        }

        let _ = child.kill().await;

        Ok(())
    }

    /// Spawn the app-server subprocess
    async fn spawn_app_server(&self) -> Result<Child> {
        info!("Starting codex app-server...");

        let child = Command::new("codex")
            .args(["app-server"])
            .current_dir(&self.config.cwd)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| GugugagaError::AppServerStart(e.to_string()))?;

        info!("codex app-server started with pid {:?}", child.id());
        Ok(child)
    }

    /// Process a message from the server
    async fn process_server_message(
        msg: &Value,
        memory: &Arc<RwLock<PersistentMemory>>,
        notebook: &Arc<RwLock<GugugagaNotebook>>,
        gugugaga_agent: &Arc<GugugagaAgent>,
        violation_detector: &ViolationDetector,
        config: &GugugagaConfig,
        current_turn_content: &str,
    ) -> InterceptAction {
        let method = msg
            .get("method")
            .and_then(|m| m.as_str())
            .unwrap_or("");

        match method {
            // Check for plan updates
            m if protocol::is_plan_update(m) => {
                InterceptAction::Forward
            }

            // Check agent messages for violations
            notifications::ITEM_AGENT_MESSAGE_DELTA => {
                if let Some(params) = msg.get("params") {
                    if let Some(text) = protocol::extract_agent_message_text(params) {
                        // Quick pattern check
                        let violations = violation_detector.check(&text);
                        if !violations.is_empty() {
                            let violation = &violations[0];
                            if config.strict_mode {
                                return InterceptAction::Interrupt(violation.correction.clone());
                            }
                            // In non-strict mode, notify user but don't interrupt
                            let mut mem = memory.write().await;
                            let _ = mem
                                .record_behavior(&format!("Violation: {}", violation.description), false)
                                .await;
                            
                            // Send violation notification so TUI can display it
                            return InterceptAction::InjectBefore(vec![
                                serde_json::json!({
                                    "method": "gugugaga/violation",
                                    "params": {
                                        "message": violation.description.clone()
                                    }
                                }).to_string()
                            ]);
                        }
                    }
                }
                InterceptAction::Forward
            }

            // Monitor turn completion - perform LLM-based evaluation (NO FALLBACK)
            "turn/completed" => {
                // Only evaluate if there's actual content (avoid empty evaluations)
                if current_turn_content.trim().len() < 20 {
                    // Too short to evaluate meaningfully
                    return InterceptAction::Forward;
                }

                // Record Codex output to conversation history
                {
                    let mut mem = memory.write().await;
                    let _ = mem.add_turn(
                        crate::memory::TurnRole::Codex,
                        current_turn_content.to_string()
                    ).await;
                }
                
                // Perform LLM evaluation with actual turn content (silently)
                let eval_result = gugugaga_agent.detect_violation(current_turn_content).await;
                
                match eval_result {
                    Ok(result) => {
                        if let Some(violation) = result.violation {
                            // Record mistake to GugugagaNotebook (not PersistentMemory)
                            {
                                let mut nb = notebook.write().await;
                                let _ = nb.record_mistake(
                                    violation.description.clone(),
                                    violation.correction.clone(),
                                    format!("Codex violated: {}", violation.description),
                                ).await;
                            }
                            // Found a violation - send correction directly to Codex
                            // Use LLM's correction as-is, no template wrapper
                            return InterceptAction::CorrectAgent(violation.correction);
                        } else {
                            // No violation - show what was analyzed
                            let mut params = serde_json::json!({
                                "status": "ok",
                                "message": result.summary
                            });
                            // Include thinking if present
                            if let Some(thinking) = result.thinking {
                                params["thinking"] = serde_json::Value::String(thinking);
                            }
                            let msg = serde_json::json!({
                                "method": "gugugaga/check",
                                "params": params
                            }).to_string();
                            return InterceptAction::InjectAfter(vec![msg]);
                        }
                    }
                    Err(e) => {
                        // LLM evaluation failed — tell the user so they know
                        // gugugaga is not working (API down, rate limited, etc.)
                        warn!("LLM evaluation failed: {}", e);
                        let msg = serde_json::json!({
                            "method": "gugugaga/check",
                            "params": { "status": "error", "message": format!("Evaluation failed: {}", e) }
                        }).to_string();
                        return InterceptAction::InjectAfter(vec![msg]);
                    }
                }
            }

            // Smart filter for user input requests
            notifications::REQUEST_USER_INPUT => {
                if let Some(params) = msg.get("params") {
                    let request_str = serde_json::to_string(params).unwrap_or_default();

                    // Evaluate with gugugaga agent
                    match gugugaga_agent.evaluate_request(&request_str).await {
                        Ok(EvaluationResult::AutoReply(reply)) => {
                            info!("Auto-replying to request: {}", reply);
                            // TODO: Send auto-reply back to app-server
                            // For now, forward to user with a hint
                            InterceptAction::Forward
                        }
                        Ok(EvaluationResult::Correct(correction)) => {
                            InterceptAction::Interrupt(correction)
                        }
                        Ok(EvaluationResult::ForwardToUser) => InterceptAction::Forward,
                        Err(e) => {
                            warn!("Evaluation failed: {}", e);
                            InterceptAction::Forward
                        }
                    }
                } else {
                    InterceptAction::Forward
                }
            }

            // Forward everything else
            _ => InterceptAction::Forward,
        }
    }

    /// Get reference to memory
    pub fn memory(&self) -> Arc<RwLock<PersistentMemory>> {
        self.memory.clone()
    }

    /// Get reference to gugugaga agent
    pub fn gugugaga_agent(&self) -> Arc<GugugagaAgent> {
        self.gugugaga_agent.clone()
    }
}
