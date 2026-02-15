//! LLM Evaluator for the gugugaga agent
//!
//! Supports both API key and ChatGPT OAuth authentication modes.
//! - API key mode: uses Chat Completions API at api.openai.com/v1
//! - OAuth mode: uses Responses API at chatgpt.com/backend-api/codex
//! Token refresh is handled by Codex's AuthManager — we simply re-read
//! auth.json before each request to pick up the latest tokens.
//! Respects user's config.toml for custom model providers.

use crate::{GugugagaError, Result};
use futures::StreamExt;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::time::Duration;
use tokio::sync::mpsc;
use tracing::{debug, error, info, warn};

/// Retry config aligned with Codex: codex-rs/core/src/model_provider_info.rs
const MAX_RETRY_ATTEMPTS: u32 = 4;
const RETRY_BASE_DELAY_MS: u64 = 200;
/// Request timeout (Codex uses reqwest defaults ~30s, we set explicitly)
const REQUEST_TIMEOUT: Duration = Duration::from_secs(60);

// ─── Auth mode ───────────────────────────────────────────────────────

/// Authentication mode, aligned with Codex's AuthMode
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EvaluatorAuthMode {
    /// Standard API key authentication (api.openai.com)
    ApiKey,
    /// ChatGPT OAuth authentication (chatgpt.com/backend-api/codex)
    ChatgptOAuth,
}

/// Wire API format
#[derive(Debug, Clone, PartialEq, Eq)]
enum WireApi {
    /// OpenAI Chat Completions API (/chat/completions)
    Chat,
    /// OpenAI Responses API (/responses)
    Responses,
}

/// Auth credentials read from auth.json (fresh per request)
#[derive(Debug, Clone)]
struct AuthCredentials {
    /// Bearer token (API key or OAuth access_token)
    token: String,
    /// Auth mode
    mode: EvaluatorAuthMode,
    /// ChatGPT account ID (for OAuth, sent as header)
    account_id: Option<String>,
}

// ─── Evaluator ───────────────────────────────────────────────────────

/// Evaluator that calls LLM for gugugaga decisions.
/// Supports both API key (Chat Completions) and OAuth (Responses API) modes.
/// Token refresh is delegated to Codex — we re-read auth.json each request.
pub struct Evaluator {
    client: Client,
    model: String,
    base_url: String,
    wire_api: WireApi,
    codex_home: PathBuf,
}

/// Streaming event from gugugaga LLM
#[derive(Debug, Clone)]
pub enum GugugagaThinking {
    /// Thinking/reasoning content
    Thinking(String),
    /// Final response content
    Response(String),
    /// Completed
    Done,
    /// Error occurred
    Error(String),
}

/// Parsed LLM response with thinking and final answer separated
#[derive(Debug, Clone)]
pub struct ParsedResponse {
    /// Thinking/reasoning content (from <think> tags)
    pub thinking: Option<String>,
    /// Final response content (after </think>)
    pub response: String,
}

// ─── Chat Completions API types (API key mode) ──────────────────────

#[derive(Debug, Serialize)]
struct ChatRequest {
    model: String,
    messages: Vec<ChatMessage>,
    max_tokens: u32,
    temperature: f32,
    stream: bool,
}

#[derive(Debug, Serialize, Deserialize)]
struct ChatMessage {
    role: String,
    content: String,
}

#[derive(Debug, Deserialize)]
struct ChatResponse {
    choices: Vec<ChatChoice>,
}

#[derive(Debug, Deserialize)]
struct ChatChoice {
    message: ChatMessage,
}

#[derive(Debug, Deserialize)]
struct StreamChunk {
    choices: Vec<StreamChoice>,
}

#[derive(Debug, Deserialize)]
struct StreamChoice {
    delta: StreamDelta,
    finish_reason: Option<String>,
}

#[derive(Debug, Deserialize)]
struct StreamDelta {
    content: Option<String>,
}

// ─── Responses API types (OAuth mode) ───────────────────────────────

#[derive(Debug, Serialize)]
struct ResponsesRequest {
    model: String,
    input: Vec<ResponsesInputItem>,
    stream: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    instructions: Option<String>,
}

#[derive(Debug, Serialize)]
struct ResponsesInputItem {
    #[serde(rename = "type")]
    item_type: String,
    role: String,
    content: Vec<ResponsesContentItem>,
}

#[derive(Debug, Serialize)]
struct ResponsesContentItem {
    #[serde(rename = "type")]
    content_type: String,
    text: String,
}

// ─── Responses API SSE event types ──────────────────────────────────

#[derive(Debug, Deserialize)]
struct ResponsesSseEvent {
    #[serde(rename = "type")]
    event_type: String,
    /// Text delta (for response.output_text.delta)
    #[serde(default)]
    delta: Option<String>,
    /// Response wrapper (for response.completed / response.failed)
    #[serde(default)]
    response: Option<serde_json::Value>,
}

// ─── auth.json types ────────────────────────────────────────────────

/// Matches Codex's auth.json format exactly
/// See: codex-rs/core/src/auth/storage.rs
#[derive(Debug, Deserialize)]
struct AuthDotJson {
    /// Auth mode indicator (optional): "api_key", "chatgpt", "chatgpt_auth_tokens"
    #[serde(default)]
    auth_mode: Option<String>,

    /// API key stored as OPENAI_API_KEY in the JSON
    #[serde(rename = "OPENAI_API_KEY")]
    openai_api_key: Option<String>,

    /// OAuth tokens (if using ChatGPT login)
    #[serde(default)]
    tokens: Option<TokenData>,
}

impl AuthDotJson {
    /// Resolve auth mode, aligned with Codex's resolved_mode()
    fn resolved_mode(&self) -> EvaluatorAuthMode {
        if let Some(mode) = &self.auth_mode {
            match mode.as_str() {
                "api_key" => return EvaluatorAuthMode::ApiKey,
                "chatgpt" | "chatgpt_auth_tokens" => return EvaluatorAuthMode::ChatgptOAuth,
                _ => {}
            }
        }
        if self.openai_api_key.is_some() {
            return EvaluatorAuthMode::ApiKey;
        }
        // Default to ChatGPT OAuth (same as Codex)
        EvaluatorAuthMode::ChatgptOAuth
    }
}

/// Token data for ChatGPT OAuth authentication
#[derive(Debug, Deserialize)]
struct TokenData {
    /// The access token used for API calls
    access_token: String,

    /// Account ID
    #[serde(default)]
    account_id: Option<String>,
}

/// Partial config.toml parsing for gugugaga
#[derive(Debug, Deserialize)]
struct ConfigToml {
    /// Active model provider name
    model_provider: Option<String>,

    /// Model to use
    model: Option<String>,

    /// Custom model providers
    model_providers: Option<std::collections::HashMap<String, ModelProviderConfig>>,
}

/// Model provider configuration
#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct ModelProviderConfig {
    /// Provider name
    name: Option<String>,

    /// Base URL for API
    base_url: Option<String>,

    /// Wire API type (responses or chat)
    wire_api: Option<String>,
}

// ─── Implementation ─────────────────────────────────────────────────

impl Evaluator {
    /// Parse <think>...</think> tags from LLM response, separating thinking from response
    pub fn parse_think_tags(content: &str) -> ParsedResponse {
        let re = regex::Regex::new(r"(?s)<think>(.*?)</think>").unwrap();

        if let Some(caps) = re.captures(content) {
            let thinking = caps.get(1).map(|m| m.as_str().trim().to_string());
            let response = re.replace_all(content, "").trim().to_string();
            ParsedResponse { thinking, response }
        } else if content.starts_with("<think>") {
            let thinking = content.trim_start_matches("<think>").trim().to_string();
            ParsedResponse {
                thinking: Some(thinking),
                response: String::new(),
            }
        } else {
            ParsedResponse {
                thinking: None,
                response: content.trim().to_string(),
            }
        }
    }

    /// Compute exponential backoff with jitter, aligned with Codex's retry.rs
    fn retry_backoff(attempt: u32) -> Duration {
        let exp = 2u64.saturating_pow(attempt.saturating_sub(1));
        let base_ms = RETRY_BASE_DELAY_MS.saturating_mul(exp);
        let jitter = 1.0 + ((attempt as f64 * 0.37).sin() * 0.1);
        Duration::from_millis((base_ms as f64 * jitter) as u64)
    }

    /// Check if an error is retryable (network/timeout/5xx)
    fn is_retryable_status(status: reqwest::StatusCode) -> bool {
        status.is_server_error() // 5xx
    }

    /// Check if an error message indicates a retryable condition
    fn is_retryable_error(msg: &str) -> bool {
        msg.contains("timeout")
            || msg.contains("network")
            || msg.contains("retryable")
            || msg.contains("error sending request")
            || msg.contains("connection")
    }

    /// Create a new evaluator, loading auth and config from codex home.
    /// Automatically detects OAuth vs API key mode and configures accordingly.
    pub async fn new(codex_home: &Path) -> Result<Self> {
        let client = Client::builder()
            .timeout(REQUEST_TIMEOUT)
            .connect_timeout(Duration::from_secs(15))
            .user_agent("gugugaga/0.1.0")
            .build()
            .unwrap_or_else(|_| Client::new());

        // Read auth to detect mode (OAuth vs API key)
        let creds = Self::read_auth(codex_home).await?;
        let auth_mode = creds.mode.clone();

        // Load config to get model provider settings
        let (model, base_url, wire_api) = Self::load_config(codex_home, &auth_mode).await?;

        info!(
            "Gugugaga evaluator: mode={:?}, model={}, base_url={}, wire={:?}",
            auth_mode, model, base_url, wire_api
        );

        Ok(Self {
            client,
            model,
            base_url,
            wire_api,
            codex_home: codex_home.to_path_buf(),
        })
    }

    /// Load model, base_url, and wire API from config.toml.
    /// Defaults depend on auth mode (OAuth → chatgpt.com, API key → api.openai.com).
    async fn load_config(
        codex_home: &Path,
        auth_mode: &EvaluatorAuthMode,
    ) -> Result<(String, String, WireApi)> {
        let config_file = codex_home.join("config.toml");

        if config_file.exists() {
            let content = tokio::fs::read_to_string(&config_file).await?;

            if let Ok(config) = toml::from_str::<ConfigToml>(&content) {
                let model = config.model.unwrap_or_else(|| "gpt-5.2-codex".to_string());

                // Check for custom model provider
                if let Some(provider_name) = &config.model_provider {
                    if let Some(providers) = &config.model_providers {
                        if let Some(provider) = providers.get(provider_name) {
                            if let Some(base_url) = &provider.base_url {
                                info!(
                                    "Using custom provider '{}' with base_url: {}",
                                    provider_name, base_url
                                );
                                let wire = match provider.wire_api.as_deref() {
                                    Some("chat") => WireApi::Chat,
                                    Some("responses") => WireApi::Responses,
                                    _ => Self::default_wire_api(auth_mode),
                                };
                                return Ok((model, base_url.clone(), wire));
                            }
                        }
                    }
                }

                let (base_url, wire) = Self::default_endpoint(auth_mode);
                return Ok((model, base_url, wire));
            }
        }

        let (base_url, wire) = Self::default_endpoint(auth_mode);
        Ok(("gpt-5.2-codex".to_string(), base_url, wire))
    }

    /// Default base URL and wire API based on auth mode.
    /// Aligned with Codex's model_provider_info.rs:
    /// - ChatGPT OAuth → chatgpt.com/backend-api/codex (Responses API)
    /// - API key → api.openai.com/v1 (Chat Completions API)
    fn default_endpoint(auth_mode: &EvaluatorAuthMode) -> (String, WireApi) {
        match auth_mode {
            EvaluatorAuthMode::ChatgptOAuth => (
                "https://chatgpt.com/backend-api/codex".to_string(),
                WireApi::Responses,
            ),
            EvaluatorAuthMode::ApiKey => (
                "https://api.openai.com/v1".to_string(),
                WireApi::Chat,
            ),
        }
    }

    fn default_wire_api(auth_mode: &EvaluatorAuthMode) -> WireApi {
        match auth_mode {
            EvaluatorAuthMode::ChatgptOAuth => WireApi::Responses,
            EvaluatorAuthMode::ApiKey => WireApi::Chat,
        }
    }

    // ─── Auth reading (fresh from disk each request) ────────────────

    /// Read auth credentials from Codex's auth.json.
    /// Called before each request so we always use the latest token
    /// (Codex's AuthManager handles refresh and writes back to auth.json).
    async fn read_auth(codex_home: &Path) -> Result<AuthCredentials> {
        let auth_file = codex_home.join("auth.json");
        if !auth_file.exists() {
            return Err(GugugagaError::Auth(
                "No auth.json found. Login via `codex login` first.".to_string(),
            ));
        }

        let content = tokio::fs::read_to_string(&auth_file).await?;

        // Try structured parsing first
        if let Ok(auth) = serde_json::from_str::<AuthDotJson>(&content) {
            let mode = auth.resolved_mode();

            match mode {
                EvaluatorAuthMode::ChatgptOAuth => {
                    if let Some(tokens) = &auth.tokens {
                        let access_token = tokens.access_token.trim().to_string();
                        if !access_token.is_empty() {
                            return Ok(AuthCredentials {
                                token: access_token,
                                mode: EvaluatorAuthMode::ChatgptOAuth,
                                account_id: tokens.account_id.clone(),
                            });
                        }
                    }
                    return Err(GugugagaError::Auth(
                        "ChatGPT OAuth mode but no access_token in auth.json.".to_string(),
                    ));
                }
                EvaluatorAuthMode::ApiKey => {
                    if let Some(api_key) = &auth.openai_api_key {
                        let key = api_key.trim().to_string();
                        if !key.is_empty() {
                            return Ok(AuthCredentials {
                                token: key,
                                mode: EvaluatorAuthMode::ApiKey,
                                account_id: None,
                            });
                        }
                    }
                    return Err(GugugagaError::Auth(
                        "API key mode but no OPENAI_API_KEY in auth.json.".to_string(),
                    ));
                }
            }
        }

        // Fallback: raw JSON extraction
        debug!("Failed to parse auth.json with struct, trying raw extraction");
        if let Ok(value) = serde_json::from_str::<serde_json::Value>(&content) {
            if let Some(access_token) = value
                .get("tokens")
                .and_then(|t| t.get("access_token"))
                .and_then(|v| v.as_str())
            {
                let token = access_token.trim().to_string();
                if !token.is_empty() {
                    let account_id = value
                        .get("tokens")
                        .and_then(|t| t.get("account_id"))
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string());
                    return Ok(AuthCredentials {
                        token,
                        mode: EvaluatorAuthMode::ChatgptOAuth,
                        account_id,
                    });
                }
            }

            if let Some(key) = value.get("OPENAI_API_KEY").and_then(|v| v.as_str()) {
                let key = key.trim().to_string();
                if !key.is_empty() {
                    return Ok(AuthCredentials {
                        token: key,
                        mode: EvaluatorAuthMode::ApiKey,
                        account_id: None,
                    });
                }
            }
        }

        Err(GugugagaError::Auth(
            "No valid credentials in auth.json. Login via `codex login` first.".to_string(),
        ))
    }

    /// Read fresh credentials and build auth headers for a request.
    async fn fresh_auth_headers(&self) -> Result<Vec<(String, String)>> {
        let creds = Self::read_auth(&self.codex_home).await?;
        let mut headers = vec![(
            "Authorization".to_string(),
            format!("Bearer {}", creds.token),
        )];

        if creds.mode == EvaluatorAuthMode::ChatgptOAuth {
            if let Some(account_id) = &creds.account_id {
                headers.push(("ChatGPT-Account-ID".to_string(), account_id.clone()));
            }
        }

        Ok(headers)
    }

    // ─── Request building ───────────────────────────────────────────

    /// Build a request and send it based on the wire API type.
    async fn send_request(
        &self,
        system_prompt: Option<&str>,
        user_prompt: &str,
    ) -> Result<String> {
        match self.wire_api {
            WireApi::Chat => {
                self.send_chat_completions_request(system_prompt, user_prompt)
                    .await
            }
            WireApi::Responses => {
                self.send_responses_api_request(system_prompt, user_prompt)
                    .await
            }
        }
    }

    /// Send a Chat Completions API request (for API key mode).
    async fn send_chat_completions_request(
        &self,
        system_prompt: Option<&str>,
        user_prompt: &str,
    ) -> Result<String> {
        let url = format!("{}/chat/completions", self.base_url);

        let mut messages = Vec::new();
        if let Some(sys) = system_prompt {
            messages.push(ChatMessage {
                role: "system".to_string(),
                content: sys.to_string(),
            });
        }
        messages.push(ChatMessage {
            role: "user".to_string(),
            content: user_prompt.to_string(),
        });

        let request = ChatRequest {
            model: self.model.clone(),
            messages,
            max_tokens: 2048,
            temperature: 0.1,
            stream: false,
        };

        let headers = self.fresh_auth_headers().await?;
        let mut req_builder = self
            .client
            .post(&url)
            .header("Content-Type", "application/json");

        for (name, value) in &headers {
            req_builder = req_builder.header(name.as_str(), value.as_str());
        }

        let response = req_builder
            .json(&request)
            .send()
            .await
            .map_err(Self::map_reqwest_error)?;

        Self::check_response_status(&response)?;

        let chat_response: ChatResponse = response
            .json()
            .await
            .map_err(|e| GugugagaError::LlmEvaluation(e.to_string()))?;

        Ok(chat_response
            .choices
            .first()
            .map(|c| c.message.content.clone())
            .unwrap_or_default())
    }

    /// Send a Responses API request (for OAuth mode).
    /// Uses streaming internally and collects the full response.
    async fn send_responses_api_request(
        &self,
        system_prompt: Option<&str>,
        user_prompt: &str,
    ) -> Result<String> {
        let url = format!("{}/responses", self.base_url.trim_end_matches('/'));

        let mut input = Vec::new();
        if let Some(sys) = system_prompt {
            input.push(ResponsesInputItem {
                item_type: "message".to_string(),
                role: "developer".to_string(),
                content: vec![ResponsesContentItem {
                    content_type: "input_text".to_string(),
                    text: sys.to_string(),
                }],
            });
        }
        input.push(ResponsesInputItem {
            item_type: "message".to_string(),
            role: "user".to_string(),
            content: vec![ResponsesContentItem {
                content_type: "input_text".to_string(),
                text: user_prompt.to_string(),
            }],
        });

        let request = ResponsesRequest {
            model: self.model.clone(),
            input,
            stream: true, // Responses API is streaming-only
            instructions: None,
        };

        let headers = self.fresh_auth_headers().await?;
        let mut req_builder = self
            .client
            .post(&url)
            .header("Content-Type", "application/json")
            .header("Accept", "text/event-stream");

        for (name, value) in &headers {
            req_builder = req_builder.header(name.as_str(), value.as_str());
        }

        let response = req_builder
            .json(&request)
            .send()
            .await
            .map_err(Self::map_reqwest_error)?;

        Self::check_response_status(&response)?;

        Self::collect_responses_stream(response).await
    }

    /// Collect a Responses API SSE stream into the full text response.
    async fn collect_responses_stream(response: reqwest::Response) -> Result<String> {
        let mut stream = response.bytes_stream();
        let mut result_text = String::new();
        let mut line_buffer = String::new();

        while let Some(chunk_result) = stream.next().await {
            let bytes = chunk_result
                .map_err(|e| GugugagaError::LlmEvaluation(format!("stream error: {e}")))?;
            let text = String::from_utf8_lossy(&bytes);
            line_buffer.push_str(&text);

            while let Some(newline_pos) = line_buffer.find('\n') {
                let line = line_buffer[..newline_pos].to_string();
                line_buffer = line_buffer[newline_pos + 1..].to_string();

                let line = line.trim();
                if !line.starts_with("data: ") {
                    continue;
                }
                let data = &line[6..];
                if data == "[DONE]" {
                    return Ok(result_text);
                }

                if let Ok(event) = serde_json::from_str::<ResponsesSseEvent>(data) {
                    match event.event_type.as_str() {
                        "response.output_text.delta" => {
                            if let Some(delta) = &event.delta {
                                result_text.push_str(delta);
                            }
                        }
                        "response.completed" | "response.done" => {
                            return Ok(result_text);
                        }
                        "response.failed" => {
                            let error_msg = event
                                .response
                                .as_ref()
                                .and_then(|r| r.get("error"))
                                .and_then(|e| e.get("message"))
                                .and_then(|m| m.as_str())
                                .unwrap_or("unknown error");
                            return Err(GugugagaError::LlmEvaluation(format!(
                                "Responses API error: {error_msg}"
                            )));
                        }
                        _ => {}
                    }
                }
            }
        }

        Ok(result_text)
    }

    fn check_response_status(response: &reqwest::Response) -> Result<()> {
        if response.status().is_success() {
            return Ok(());
        }
        let status = response.status();
        if Self::is_retryable_status(status) {
            return Err(GugugagaError::LlmEvaluation(format!(
                "retryable API error {status}"
            )));
        }
        Err(GugugagaError::LlmEvaluation(format!("API error {status}")))
    }

    fn map_reqwest_error(e: reqwest::Error) -> GugugagaError {
        if e.is_timeout() {
            GugugagaError::LlmEvaluation(format!("timeout: {e}"))
        } else if e.is_connect() {
            GugugagaError::LlmEvaluation(format!("network: {e}"))
        } else {
            GugugagaError::LlmEvaluation(e.to_string())
        }
    }

    // ─── Public API (same interface as before) ──────────────────────

    /// Call LLM with retry logic aligned with Codex (max 4 attempts, exponential backoff).
    pub async fn call_llm(&self, prompt: &str) -> Result<String> {
        debug!("Calling LLM with prompt length: {}", prompt.len());

        let mut last_err = None;
        for attempt in 0..MAX_RETRY_ATTEMPTS {
            if attempt > 0 {
                let delay = Self::retry_backoff(attempt);
                warn!(
                    "LLM request failed (attempt {}/{}), retrying in {:?}...",
                    attempt, MAX_RETRY_ATTEMPTS, delay
                );
                tokio::time::sleep(delay).await;
            }

            match self.send_request(None, prompt).await {
                Ok(content) => {
                    let parsed = Self::parse_think_tags(&content);
                    if let Some(thinking) = &parsed.thinking {
                        debug!("LLM thinking: {}", thinking);
                    }
                    debug!("LLM response: {}", parsed.response);
                    return Ok(parsed.response);
                }
                Err(e) => {
                    let msg = e.to_string();
                    if Self::is_retryable_error(&msg) && attempt + 1 < MAX_RETRY_ATTEMPTS {
                        last_err = Some(e);
                        continue;
                    }
                    return Err(e);
                }
            }
        }

        Err(last_err.unwrap_or_else(|| {
            GugugagaError::LlmEvaluation("all retry attempts exhausted".to_string())
        }))
    }

    /// Call LLM and return both thinking and response (with retry).
    pub async fn call_llm_with_thinking(&self, prompt: &str) -> Result<ParsedResponse> {
        debug!("Calling LLM (with thinking) prompt length: {}", prompt.len());

        let mut last_err = None;
        for attempt in 0..MAX_RETRY_ATTEMPTS {
            if attempt > 0 {
                let delay = Self::retry_backoff(attempt);
                warn!(
                    "LLM request failed (attempt {}/{}), retrying in {:?}...",
                    attempt, MAX_RETRY_ATTEMPTS, delay
                );
                tokio::time::sleep(delay).await;
            }

            match self.send_request(None, prompt).await {
                Ok(content) => {
                    let parsed = Self::parse_think_tags(&content);
                    if let Some(thinking) = &parsed.thinking {
                        debug!("LLM thinking: {}", thinking);
                    }
                    debug!("LLM response: {}", parsed.response);
                    return Ok(parsed);
                }
                Err(e) => {
                    let msg = e.to_string();
                    if Self::is_retryable_error(&msg) && attempt + 1 < MAX_RETRY_ATTEMPTS {
                        last_err = Some(e);
                        continue;
                    }
                    return Err(e);
                }
            }
        }

        Err(last_err.unwrap_or_else(|| {
            GugugagaError::LlmEvaluation("all retry attempts exhausted".to_string())
        }))
    }

    /// Call LLM with streaming output - returns channel for real-time thinking.
    /// Supports both Chat Completions SSE and Responses API SSE formats.
    pub async fn call_llm_streaming(
        &self,
        system_prompt: &str,
        user_prompt: &str,
    ) -> Result<mpsc::Receiver<GugugagaThinking>> {
        match self.wire_api {
            WireApi::Chat => self.stream_chat_completions(system_prompt, user_prompt).await,
            WireApi::Responses => self.stream_responses_api(system_prompt, user_prompt).await,
        }
    }

    /// Stream via Chat Completions API (API key mode).
    async fn stream_chat_completions(
        &self,
        system_prompt: &str,
        user_prompt: &str,
    ) -> Result<mpsc::Receiver<GugugagaThinking>> {
        let (tx, rx) = mpsc::channel(32);

        let request = serde_json::json!({
            "model": self.model,
            "messages": [
                {"role": "system", "content": system_prompt},
                {"role": "user", "content": user_prompt}
            ],
            "max_tokens": 2048,
            "temperature": 0.1,
            "stream": true
        });

        let url = format!("{}/chat/completions", self.base_url);
        let headers = self.fresh_auth_headers().await?;

        let mut req_builder = self
            .client
            .post(&url)
            .header("Content-Type", "application/json");

        for (name, value) in &headers {
            req_builder = req_builder.header(name.as_str(), value.as_str());
        }

        let response = req_builder
            .json(&request)
            .send()
            .await
            .map_err(|e| GugugagaError::LlmEvaluation(e.to_string()))?;

        if !response.status().is_success() {
            let status = response.status();
            let text = response.text().await.unwrap_or_default();
            error!("LLM API error: {} - {}", status, text);
            return Err(GugugagaError::LlmEvaluation(format!(
                "API error {}: {}",
                status, text
            )));
        }

        let mut stream = response.bytes_stream();
        tokio::spawn(async move {
            let mut buffer = String::new();
            let mut in_thinking = true;

            while let Some(chunk_result) = stream.next().await {
                match chunk_result {
                    Ok(bytes) => {
                        let text = String::from_utf8_lossy(&bytes);
                        for line in text.lines() {
                            if line.starts_with("data: ") {
                                let data = &line[6..];
                                if data == "[DONE]" {
                                    let _ = tx.send(GugugagaThinking::Done).await;
                                    return;
                                }

                                if let Ok(chunk) = serde_json::from_str::<StreamChunk>(data) {
                                    if let Some(choice) = chunk.choices.first() {
                                        if let Some(content) = &choice.delta.content {
                                            buffer.push_str(content);

                                            if buffer.contains("---RESPONSE---") {
                                                in_thinking = false;
                                                let parts: Vec<&str> =
                                                    buffer.splitn(2, "---RESPONSE---").collect();
                                                if parts.len() == 2 {
                                                    buffer = parts[1].to_string();
                                                }
                                            }

                                            let event = if in_thinking {
                                                GugugagaThinking::Thinking(content.clone())
                                            } else {
                                                GugugagaThinking::Response(content.clone())
                                            };
                                            let _ = tx.send(event).await;
                                        }

                                        if choice.finish_reason.is_some() {
                                            let _ = tx.send(GugugagaThinking::Done).await;
                                            return;
                                        }
                                    }
                                }
                            }
                        }
                    }
                    Err(e) => {
                        let _ = tx.send(GugugagaThinking::Error(e.to_string())).await;
                        return;
                    }
                }
            }
            let _ = tx.send(GugugagaThinking::Done).await;
        });

        Ok(rx)
    }

    /// Stream via Responses API (OAuth mode).
    async fn stream_responses_api(
        &self,
        system_prompt: &str,
        user_prompt: &str,
    ) -> Result<mpsc::Receiver<GugugagaThinking>> {
        let (tx, rx) = mpsc::channel(32);

        let url = format!("{}/responses", self.base_url.trim_end_matches('/'));

        let input = vec![
            ResponsesInputItem {
                item_type: "message".to_string(),
                role: "developer".to_string(),
                content: vec![ResponsesContentItem {
                    content_type: "input_text".to_string(),
                    text: system_prompt.to_string(),
                }],
            },
            ResponsesInputItem {
                item_type: "message".to_string(),
                role: "user".to_string(),
                content: vec![ResponsesContentItem {
                    content_type: "input_text".to_string(),
                    text: user_prompt.to_string(),
                }],
            },
        ];

        let request = ResponsesRequest {
            model: self.model.clone(),
            input,
            stream: true,
            instructions: None,
        };

        let headers = self.fresh_auth_headers().await?;
        let mut req_builder = self
            .client
            .post(&url)
            .header("Content-Type", "application/json")
            .header("Accept", "text/event-stream");

        for (name, value) in &headers {
            req_builder = req_builder.header(name.as_str(), value.as_str());
        }

        let response = req_builder
            .json(&request)
            .send()
            .await
            .map_err(|e| GugugagaError::LlmEvaluation(e.to_string()))?;

        if !response.status().is_success() {
            let status = response.status();
            let text = response.text().await.unwrap_or_default();
            error!("LLM API error: {} - {}", status, text);
            return Err(GugugagaError::LlmEvaluation(format!(
                "API error {}: {}",
                status, text
            )));
        }

        let mut stream = response.bytes_stream();
        tokio::spawn(async move {
            let mut in_thinking = true;
            let mut buffer = String::new();
            let mut line_buffer = String::new();

            while let Some(chunk_result) = stream.next().await {
                match chunk_result {
                    Ok(bytes) => {
                        let text = String::from_utf8_lossy(&bytes);
                        line_buffer.push_str(&text);

                        while let Some(newline_pos) = line_buffer.find('\n') {
                            let line = line_buffer[..newline_pos].to_string();
                            line_buffer = line_buffer[newline_pos + 1..].to_string();

                            let line = line.trim();
                            if !line.starts_with("data: ") {
                                continue;
                            }
                            let data = &line[6..];
                            if data == "[DONE]" {
                                let _ = tx.send(GugugagaThinking::Done).await;
                                return;
                            }

                            if let Ok(event) = serde_json::from_str::<ResponsesSseEvent>(data) {
                                match event.event_type.as_str() {
                                    "response.output_text.delta" => {
                                        if let Some(delta) = &event.delta {
                                            buffer.push_str(delta);

                                            if buffer.contains("---RESPONSE---") {
                                                in_thinking = false;
                                                let parts: Vec<&str> = buffer
                                                    .splitn(2, "---RESPONSE---")
                                                    .collect();
                                                if parts.len() == 2 {
                                                    buffer = parts[1].to_string();
                                                }
                                            }

                                            let event = if in_thinking {
                                                GugugagaThinking::Thinking(delta.clone())
                                            } else {
                                                GugugagaThinking::Response(delta.clone())
                                            };
                                            let _ = tx.send(event).await;
                                        }
                                    }
                                    "response.completed" | "response.done" => {
                                        let _ = tx.send(GugugagaThinking::Done).await;
                                        return;
                                    }
                                    "response.failed" => {
                                        let error_msg = event
                                            .response
                                            .as_ref()
                                            .and_then(|r| r.get("error"))
                                            .and_then(|e| e.get("message"))
                                            .and_then(|m| m.as_str())
                                            .unwrap_or("unknown error");
                                        let _ = tx
                                            .send(GugugagaThinking::Error(error_msg.to_string()))
                                            .await;
                                        return;
                                    }
                                    _ => {}
                                }
                            }
                        }
                    }
                    Err(e) => {
                        let _ = tx.send(GugugagaThinking::Error(e.to_string())).await;
                        return;
                    }
                }
            }
            let _ = tx.send(GugugagaThinking::Done).await;
        });

        Ok(rx)
    }
}
