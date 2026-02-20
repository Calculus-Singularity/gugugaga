//! LLM Evaluator for the gugugaga agent
//!
//! Supports both API key and ChatGPT OAuth authentication modes.
//! - API key mode: uses Chat Completions API at api.openai.com/v1
//! - OAuth mode: uses Responses API at chatgpt.com/backend-api/codex
//!
//! Token refresh is handled by Codex's AuthManager — we simply re-read
//! auth.json before each request to pick up the latest tokens.
//! Respects user's config.toml for custom model providers.

use crate::{GugugagaError, Result};
use futures::StreamExt;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Duration;
use tokio::sync::mpsc;
use tracing::{debug, info, warn};

/// Retry config aligned with Codex: codex-rs/core/src/model_provider_info.rs
const MAX_RETRY_ATTEMPTS: u32 = 4;
const RETRY_BASE_DELAY_MS: u64 = 200;
/// Request timeout (Codex uses reqwest defaults ~30s, we set explicitly)
const REQUEST_TIMEOUT: Duration = Duration::from_secs(60);
/// Responses API requires an instructions field; use a neutral fallback.
const DEFAULT_RESPONSES_INSTRUCTIONS: &str = "Follow the user's request exactly.";
const RESPONSES_REASONING_INCLUDE: &str = "reasoning.encrypted_content";
const THINKING_RESPONSE_DELIMITER: &str = "---RESPONSE---";

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
    reasoning_effort: Option<String>,
    responses_prompt_cache_key: String,
    base_url: String,
    provider_headers: Vec<(String, String)>,
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
    instructions: String,
    input: Vec<ResponsesInputItem>,
    tools: Vec<serde_json::Value>,
    tool_choice: &'static str,
    parallel_tool_calls: bool,
    store: bool,
    stream: bool,
    include: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    reasoning: Option<ResponsesReasoning>,
    #[serde(skip_serializing_if = "Option::is_none")]
    prompt_cache_key: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    text: Option<serde_json::Value>,
}

#[derive(Debug, Serialize)]
struct ResponsesReasoning {
    #[serde(skip_serializing_if = "Option::is_none")]
    effort: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    summary: Option<String>,
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
    /// Output item payload (for response.output_item.done)
    #[serde(default)]
    item: Option<serde_json::Value>,
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

/// Partial config.toml parsing for gugugaga.
/// Mirrors Codex's ConfigToml but adds gugugaga-specific fields.
#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct ConfigToml {
    /// Active model provider name (shared with Codex)
    model_provider: Option<String>,

    /// Model Codex uses (shared, used as default for gugugaga)
    model: Option<String>,

    /// Reasoning effort Codex uses.
    model_reasoning_effort: Option<String>,

    /// Gugugaga-specific model override.
    /// If set, gugugaga uses this model instead of Codex's model.
    gugugaga_model: Option<String>,

    /// Gugugaga-specific reasoning effort override.
    /// If set, gugugaga uses this effort instead of Codex's model_reasoning_effort.
    gugugaga_model_reasoning_effort: Option<String>,

    /// Gugugaga-specific model provider override.
    /// If set, gugugaga uses this provider instead of Codex's model_provider.
    gugugaga_model_provider: Option<String>,

    /// Custom model providers (shared with Codex)
    model_providers: Option<HashMap<String, ModelProviderConfig>>,
}

/// Model provider configuration — mirrors Codex's ModelProviderInfo (simplified).
#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)]
struct ModelProviderConfig {
    /// Provider display name
    name: Option<String>,

    /// Base URL for API
    base_url: Option<String>,

    /// Wire API type: "responses" or "chat"
    wire_api: Option<String>,

    /// Environment variable for API key
    env_key: Option<String>,

    /// Additional static HTTP headers
    http_headers: Option<HashMap<String, String>>,

    /// Additional HTTP headers sourced from environment variables
    env_http_headers: Option<HashMap<String, String>>,
}

/// Default Gugugaga model — same as Codex's default
const GUGUGAGA_DEFAULT_MODEL: &str = "gpt-5.2-codex";

// ─── Built-in model providers (aligned with Codex) ──────────────────

/// Returns built-in model providers, mirroring Codex's model_provider_info.rs.
fn built_in_model_providers() -> HashMap<String, ModelProviderConfig> {
    let mut map = HashMap::new();

    // OpenAI — default provider
    map.insert(
        "openai".to_string(),
        ModelProviderConfig {
            name: Some("OpenAI".to_string()),
            base_url: Some(
                std::env::var("OPENAI_BASE_URL")
                    .unwrap_or_else(|_| "https://api.openai.com/v1".to_string()),
            ),
            wire_api: Some("responses".to_string()),
            env_key: Some("OPENAI_API_KEY".to_string()),
            http_headers: Some(HashMap::from([
                ("version".to_string(), env!("CARGO_PKG_VERSION").to_string()),
                ("originator".to_string(), "codex_cli_rs".to_string()),
            ])),
            env_http_headers: Some(HashMap::from([
                (
                    "OpenAI-Organization".to_string(),
                    "OPENAI_ORGANIZATION".to_string(),
                ),
                ("OpenAI-Project".to_string(), "OPENAI_PROJECT".to_string()),
            ])),
        },
    );

    // Ollama (Responses API)
    map.insert(
        "ollama".to_string(),
        ModelProviderConfig {
            name: Some("Ollama".to_string()),
            base_url: Some(std::env::var("CODEX_OSS_BASE_URL").unwrap_or_else(|_| {
                let port = std::env::var("CODEX_OSS_PORT").unwrap_or_else(|_| "11434".to_string());
                format!("http://localhost:{}/v1", port)
            })),
            wire_api: Some("responses".to_string()),
            env_key: None,
            http_headers: None,
            env_http_headers: None,
        },
    );

    // Ollama (Chat Completions API)
    map.insert(
        "ollama-chat".to_string(),
        ModelProviderConfig {
            name: Some("Ollama (Chat)".to_string()),
            base_url: Some(std::env::var("CODEX_OSS_BASE_URL").unwrap_or_else(|_| {
                let port = std::env::var("CODEX_OSS_PORT").unwrap_or_else(|_| "11434".to_string());
                format!("http://localhost:{}/v1", port)
            })),
            wire_api: Some("chat".to_string()),
            env_key: None,
            http_headers: None,
            env_http_headers: None,
        },
    );

    // LM Studio
    map.insert(
        "lmstudio".to_string(),
        ModelProviderConfig {
            name: Some("LM Studio".to_string()),
            base_url: Some("http://localhost:1234/v1".to_string()),
            wire_api: Some("chat".to_string()),
            env_key: None,
            http_headers: None,
            env_http_headers: None,
        },
    );

    map
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
        let (model, reasoning_effort, base_url, provider_headers, wire_api) =
            Self::load_config(codex_home, &auth_mode).await?;

        info!(
            "Gugugaga evaluator: mode={:?}, model={}, effort={:?}, base_url={}, headers={}, wire={:?}",
            auth_mode,
            model,
            reasoning_effort,
            base_url,
            provider_headers.len(),
            wire_api
        );

        Ok(Self {
            client,
            model,
            reasoning_effort,
            responses_prompt_cache_key: format!("gugugaga-evaluator-{}", std::process::id()),
            base_url,
            provider_headers,
            wire_api,
            codex_home: codex_home.to_path_buf(),
        })
    }

    /// Load model, base_url, and wire API from config.toml.
    ///
    /// Resolution mirrors Codex (codex-rs/core/src/config/mod.rs) but with
    /// gugugaga-specific overrides:
    ///
    /// **Model** (precedence):
    ///   1. `gugugaga_model` in config.toml
    ///   2. `model` in config.toml (same as Codex)
    ///   3. `GUGUGAGA_MODEL` environment variable
    ///   4. default: `gpt-5.2-codex`
    ///
    /// **Provider** (precedence):
    ///   1. `gugugaga_model_provider` in config.toml
    ///   2. `model_provider` in config.toml (shared with Codex)
    ///   3. default: `"openai"`
    ///
    /// **Providers map**: built-in providers (openai, ollama, ollama-chat,
    /// lmstudio) are merged with user-defined `[model_providers.*]` entries,
    /// exactly like Codex.
    async fn load_config(
        codex_home: &Path,
        auth_mode: &EvaluatorAuthMode,
    ) -> Result<(
        String,
        Option<String>,
        String,
        Vec<(String, String)>,
        WireApi,
    )> {
        let config_file = codex_home.join("config.toml");

        // Start with built-in providers
        let mut providers = built_in_model_providers();

        // Defaults before config
        let mut model =
            std::env::var("GUGUGAGA_MODEL").unwrap_or_else(|_| GUGUGAGA_DEFAULT_MODEL.to_string());
        let mut reasoning_effort: Option<String> = None;
        let mut provider_id = "openai".to_string();

        if config_file.exists() {
            if let Ok(content) = tokio::fs::read_to_string(&config_file).await {
                if let Ok(config) = toml::from_str::<ConfigToml>(&content) {
                    // Merge user-defined providers into built-in (user can override)
                    if let Some(user_providers) = config.model_providers {
                        for (key, prov) in user_providers {
                            providers.insert(key, prov);
                        }
                    }

                    // Resolve model: gugugaga_model > model (Codex's) > env > default
                    if let Some(gm) = config.gugugaga_model {
                        model = gm;
                    } else if let Some(cm) = config.model {
                        model = cm;
                    }

                    // Resolve reasoning effort:
                    // gugugaga_model_reasoning_effort > model_reasoning_effort
                    if let Some(ge) = config.gugugaga_model_reasoning_effort {
                        reasoning_effort = Some(ge);
                    } else if let Some(ce) = config.model_reasoning_effort {
                        reasoning_effort = Some(ce);
                    }

                    // Resolve provider: gugugaga_model_provider > model_provider > "openai"
                    if let Some(gp) = config.gugugaga_model_provider {
                        provider_id = gp;
                    } else if let Some(mp) = config.model_provider {
                        provider_id = mp;
                    }
                }
            }
        }

        // Look up the resolved provider
        let (base_url, provider_headers, wire_api) =
            if let Some(provider) = providers.get(&provider_id) {
                let base_url = provider
                    .base_url
                    .clone()
                    .unwrap_or_else(|| Self::default_base_url(auth_mode));
                let mut provider_headers = Self::resolve_provider_headers(provider);
                if provider_id == "openai" {
                    if let Some(codex_version) = Self::resolve_codex_cli_version(codex_home).await {
                        Self::upsert_header(&mut provider_headers, "version", &codex_version);
                    } else {
                        // Avoid sending gugugaga's own crate version (e.g. 0.1.0),
                        // which can fail model min-version checks on ChatGPT backend.
                        provider_headers.retain(|(name, _)| !name.eq_ignore_ascii_case("version"));
                    }
                }

                let wire = match provider.wire_api.as_deref() {
                    Some("chat") => WireApi::Chat,
                    Some("responses") => WireApi::Responses,
                    _ => Self::default_wire_api(auth_mode),
                };

                // Special case: "openai" provider with OAuth → route to ChatGPT backend
                if provider_id == "openai" && *auth_mode == EvaluatorAuthMode::ChatgptOAuth {
                    (
                        "https://chatgpt.com/backend-api/codex".to_string(),
                        provider_headers,
                        WireApi::Responses,
                    )
                } else {
                    (base_url, provider_headers, wire)
                }
            } else {
                warn!(
                    "Model provider '{}' not found, falling back to defaults",
                    provider_id
                );
                (
                    Self::default_base_url(auth_mode),
                    Vec::new(),
                    Self::default_wire_api(auth_mode),
                )
            };

        info!(
            "Config resolved: provider='{}', model='{}', effort={:?}, base_url='{}', headers={}, wire={:?}",
            provider_id,
            model,
            reasoning_effort,
            base_url,
            provider_headers.len(),
            wire_api
        );

        Ok((
            model,
            reasoning_effort,
            base_url,
            provider_headers,
            wire_api,
        ))
    }

    fn resolve_provider_headers(provider: &ModelProviderConfig) -> Vec<(String, String)> {
        let mut headers = Vec::new();

        if let Some(static_headers) = &provider.http_headers {
            for (name, value) in static_headers {
                let trimmed = value.trim();
                if !trimmed.is_empty() {
                    headers.push((name.clone(), trimmed.to_string()));
                }
            }
        }

        if let Some(env_headers) = &provider.env_http_headers {
            for (name, env_var) in env_headers {
                if let Ok(value) = std::env::var(env_var) {
                    let trimmed = value.trim();
                    if !trimmed.is_empty() {
                        headers.push((name.clone(), trimmed.to_string()));
                    }
                }
            }
        }

        headers
    }

    async fn resolve_codex_cli_version(codex_home: &Path) -> Option<String> {
        let version_file = codex_home.join("version.json");
        let content = tokio::fs::read_to_string(version_file).await.ok()?;
        let value = serde_json::from_str::<serde_json::Value>(&content).ok()?;
        let version = value.get("latest_version")?.as_str()?.trim();
        if version.is_empty() {
            return None;
        }
        Some(version.to_string())
    }

    fn upsert_header(headers: &mut Vec<(String, String)>, name: &str, value: &str) {
        if let Some((_, existing_value)) = headers
            .iter_mut()
            .find(|(existing_name, _)| existing_name.eq_ignore_ascii_case(name))
        {
            *existing_value = value.to_string();
            return;
        }
        headers.push((name.to_string(), value.to_string()));
    }

    /// Default base URL based on auth mode.
    fn default_base_url(auth_mode: &EvaluatorAuthMode) -> String {
        match auth_mode {
            EvaluatorAuthMode::ChatgptOAuth => "https://chatgpt.com/backend-api/codex".to_string(),
            EvaluatorAuthMode::ApiKey => "https://api.openai.com/v1".to_string(),
        }
    }

    /// Default wire API based on auth mode.
    /// Aligned with Codex's model_provider_info.rs:
    /// - ChatGPT OAuth → Responses API
    /// - API key with openai → Responses API (Codex default)
    /// - Fallback → Chat API
    fn default_wire_api(auth_mode: &EvaluatorAuthMode) -> WireApi {
        match auth_mode {
            EvaluatorAuthMode::ChatgptOAuth => WireApi::Responses,
            EvaluatorAuthMode::ApiKey => WireApi::Responses,
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
    async fn send_request(&self, system_prompt: Option<&str>, user_prompt: &str) -> Result<String> {
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

        for (name, value) in &self.provider_headers {
            req_builder = req_builder.header(name.as_str(), value.as_str());
        }
        for (name, value) in &headers {
            req_builder = req_builder.header(name.as_str(), value.as_str());
        }

        let response = req_builder
            .json(&request)
            .send()
            .await
            .map_err(Self::map_reqwest_error)?;

        let response = Self::check_response_status(response).await?;

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

        let instructions = system_prompt
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .unwrap_or(DEFAULT_RESPONSES_INSTRUCTIONS)
            .to_string();

        let request = self.build_responses_request(instructions, user_prompt);

        let headers = self.fresh_auth_headers().await?;
        let mut req_builder = self
            .client
            .post(&url)
            .header("Content-Type", "application/json")
            .header("Accept", "text/event-stream");

        for (name, value) in &self.provider_headers {
            req_builder = req_builder.header(name.as_str(), value.as_str());
        }
        for (name, value) in &headers {
            req_builder = req_builder.header(name.as_str(), value.as_str());
        }

        let response = req_builder
            .json(&request)
            .send()
            .await
            .map_err(Self::map_reqwest_error)?;

        let response = Self::check_response_status(response).await?;

        Self::collect_responses_stream(response).await
    }

    fn build_responses_request(&self, instructions: String, user_prompt: &str) -> ResponsesRequest {
        let input = vec![ResponsesInputItem {
            item_type: "message".to_string(),
            role: "user".to_string(),
            content: vec![ResponsesContentItem {
                content_type: "input_text".to_string(),
                text: user_prompt.to_string(),
            }],
        }];

        let reasoning = self
            .reasoning_effort
            .as_ref()
            .map(|effort| ResponsesReasoning {
                effort: Some(effort.clone()),
                summary: None,
            });
        let include = if reasoning.is_some() {
            vec![RESPONSES_REASONING_INCLUDE.to_string()]
        } else {
            Vec::new()
        };

        ResponsesRequest {
            model: self.model.clone(),
            instructions,
            input,
            tools: Vec::new(),
            tool_choice: "auto",
            parallel_tool_calls: false,
            store: false,
            stream: true, // Responses API is streaming-only
            include,
            reasoning,
            prompt_cache_key: Some(self.responses_prompt_cache_key.clone()),
            text: None,
        }
    }

    fn responses_event_error(event: &ResponsesSseEvent, fallback: &str) -> String {
        event
            .response
            .as_ref()
            .and_then(|r| r.get("error"))
            .and_then(|e| e.get("message"))
            .and_then(|m| m.as_str())
            .unwrap_or(fallback)
            .to_string()
    }

    fn responses_event_incomplete_reason(event: &ResponsesSseEvent) -> Option<String> {
        event.response.as_ref().and_then(|r| {
            r.get("incomplete_details")
                .and_then(|d| d.get("reason"))
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
        })
    }

    fn responses_output_item_text(event: &ResponsesSseEvent) -> Option<String> {
        event
            .item
            .as_ref()
            .and_then(Self::extract_response_item_text)
    }

    fn extract_response_item_text(item: &serde_json::Value) -> Option<String> {
        let mut collected = String::new();
        if let Some(content) = item.get("content").and_then(|c| c.as_array()) {
            for block in content {
                let block_type = block
                    .get("type")
                    .and_then(|t| t.as_str())
                    .unwrap_or_default();
                if block_type == "output_text" || block_type == "text" {
                    if let Some(text) = block.get("text").and_then(|t| t.as_str()) {
                        collected.push_str(text);
                    } else if let Some(text) = block.get("value").and_then(|t| t.as_str()) {
                        collected.push_str(text);
                    }
                }
            }
        }
        if !collected.is_empty() {
            return Some(collected);
        }

        item.get("text")
            .and_then(|text| text.as_str())
            .map(|s| s.to_string())
    }

    /// Collect a Responses API SSE stream into the full text response.
    async fn collect_responses_stream(response: reqwest::Response) -> Result<String> {
        let mut stream = response.bytes_stream();
        let mut result_text = String::new();
        let mut line_buffer = String::new();
        let mut saw_output_text_delta = false;

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
                            saw_output_text_delta = true;
                            if let Some(delta) = &event.delta {
                                result_text.push_str(delta);
                            }
                        }
                        "response.output_item.done" => {
                            if !saw_output_text_delta {
                                if let Some(text) = Self::responses_output_item_text(&event) {
                                    result_text.push_str(&text);
                                }
                            }
                        }
                        "response.completed" | "response.done" => {
                            return Ok(result_text);
                        }
                        "response.failed" => {
                            let error_msg = Self::responses_event_error(&event, "response.failed");
                            return Err(GugugagaError::LlmEvaluation(format!(
                                "Responses API error: {error_msg}"
                            )));
                        }
                        "response.incomplete" => {
                            let reason = Self::responses_event_incomplete_reason(&event)
                                .unwrap_or_else(|| "unknown".to_string());
                            return Err(GugugagaError::LlmEvaluation(format!(
                                "Responses API incomplete: {reason}"
                            )));
                        }
                        _ => {}
                    }
                }
            }
        }

        Ok(result_text)
    }

    async fn check_response_status(response: reqwest::Response) -> Result<reqwest::Response> {
        let status = response.status();
        if status.is_success() {
            return Ok(response);
        }
        let body = response.text().await.unwrap_or_default();
        let detail = Self::extract_error_detail(&body);
        let detail = Self::truncate_error_detail(&detail, 500);
        if Self::is_retryable_status(status) {
            if !detail.is_empty() {
                return Err(GugugagaError::LlmEvaluation(format!(
                    "retryable API error {status}: {detail}"
                )));
            }
            return Err(GugugagaError::LlmEvaluation(format!(
                "retryable API error {status}"
            )));
        }
        if !detail.is_empty() {
            return Err(GugugagaError::LlmEvaluation(format!(
                "API error {status}: {detail}"
            )));
        }
        Err(GugugagaError::LlmEvaluation(format!("API error {status}")))
    }

    fn extract_error_detail(body: &str) -> String {
        let trimmed = body.trim();
        if trimmed.is_empty() {
            return String::new();
        }

        if let Ok(value) = serde_json::from_str::<serde_json::Value>(trimmed) {
            if let Some(msg) = value
                .get("error")
                .and_then(|e| e.get("message"))
                .and_then(|m| m.as_str())
            {
                return msg.to_string();
            }
            if let Some(msg) = value.get("message").and_then(|m| m.as_str()) {
                return msg.to_string();
            }
            if let Some(msg) = value
                .get("response")
                .and_then(|r| r.get("error"))
                .and_then(|e| e.get("message"))
                .and_then(|m| m.as_str())
            {
                return msg.to_string();
            }
        }

        trimmed.to_string()
    }

    fn truncate_error_detail(detail: &str, max_chars: usize) -> String {
        if detail.chars().count() <= max_chars {
            return detail.to_string();
        }

        let mut truncated = detail.chars().take(max_chars).collect::<String>();
        truncated.push_str("... [truncated]");
        truncated
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
        debug!(
            "Calling LLM (with thinking) prompt length: {}",
            prompt.len()
        );

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
            WireApi::Chat => {
                self.stream_chat_completions(system_prompt, user_prompt)
                    .await
            }
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

        for (name, value) in &self.provider_headers {
            req_builder = req_builder.header(name.as_str(), value.as_str());
        }
        for (name, value) in &headers {
            req_builder = req_builder.header(name.as_str(), value.as_str());
        }

        let response = req_builder
            .json(&request)
            .send()
            .await
            .map_err(|e| GugugagaError::LlmEvaluation(e.to_string()))?;

        let response = Self::check_response_status(response).await?;

        let mut stream = response.bytes_stream();
        tokio::spawn(async move {
            let mut buffer = String::new();
            let mut in_thinking = true;

            while let Some(chunk_result) = stream.next().await {
                match chunk_result {
                    Ok(bytes) => {
                        let text = String::from_utf8_lossy(&bytes);
                        for line in text.lines() {
                            if let Some(data) = line.strip_prefix("data: ") {
                                if data == "[DONE]" {
                                    let _ = tx.send(GugugagaThinking::Done).await;
                                    return;
                                }

                                if let Ok(chunk) = serde_json::from_str::<StreamChunk>(data) {
                                    if let Some(choice) = chunk.choices.first() {
                                        if let Some(content) = &choice.delta.content {
                                            buffer.push_str(content);

                                            if buffer.contains(THINKING_RESPONSE_DELIMITER) {
                                                in_thinking = false;
                                                let parts: Vec<&str> = buffer
                                                    .splitn(2, THINKING_RESPONSE_DELIMITER)
                                                    .collect();
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

        let instructions = if system_prompt.trim().is_empty() {
            DEFAULT_RESPONSES_INSTRUCTIONS.to_string()
        } else {
            system_prompt.to_string()
        };

        let request = self.build_responses_request(instructions, user_prompt);

        let headers = self.fresh_auth_headers().await?;
        let mut req_builder = self
            .client
            .post(&url)
            .header("Content-Type", "application/json")
            .header("Accept", "text/event-stream");

        for (name, value) in &self.provider_headers {
            req_builder = req_builder.header(name.as_str(), value.as_str());
        }
        for (name, value) in &headers {
            req_builder = req_builder.header(name.as_str(), value.as_str());
        }

        let response = req_builder
            .json(&request)
            .send()
            .await
            .map_err(|e| GugugagaError::LlmEvaluation(e.to_string()))?;

        let response = Self::check_response_status(response).await?;

        let mut stream = response.bytes_stream();
        tokio::spawn(async move {
            let mut in_thinking = true;
            let mut buffer = String::new();
            let mut line_buffer = String::new();
            let mut saw_output_text_delta = false;

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
                                        saw_output_text_delta = true;
                                        if let Some(delta) = &event.delta {
                                            buffer.push_str(delta);

                                            if buffer.contains(THINKING_RESPONSE_DELIMITER) {
                                                in_thinking = false;
                                                let parts: Vec<&str> = buffer
                                                    .splitn(2, THINKING_RESPONSE_DELIMITER)
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
                                    "response.output_item.done" => {
                                        if !saw_output_text_delta {
                                            if let Some(text) =
                                                Self::responses_output_item_text(&event)
                                            {
                                                buffer.push_str(&text);

                                                if buffer.contains(THINKING_RESPONSE_DELIMITER) {
                                                    in_thinking = false;
                                                    let parts: Vec<&str> = buffer
                                                        .splitn(2, THINKING_RESPONSE_DELIMITER)
                                                        .collect();
                                                    if parts.len() == 2 {
                                                        buffer = parts[1].to_string();
                                                    }
                                                }

                                                let stream_event = if in_thinking {
                                                    GugugagaThinking::Thinking(text)
                                                } else {
                                                    GugugagaThinking::Response(text)
                                                };
                                                let _ = tx.send(stream_event).await;
                                            }
                                        }
                                    }
                                    "response.completed" | "response.done" => {
                                        let _ = tx.send(GugugagaThinking::Done).await;
                                        return;
                                    }
                                    "response.failed" => {
                                        let error_msg =
                                            Self::responses_event_error(&event, "response.failed");
                                        let _ = tx.send(GugugagaThinking::Error(error_msg)).await;
                                        return;
                                    }
                                    "response.incomplete" => {
                                        let reason =
                                            Self::responses_event_incomplete_reason(&event)
                                                .unwrap_or_else(|| "unknown".to_string());
                                        let _ = tx
                                            .send(GugugagaThinking::Error(format!(
                                                "Responses API incomplete: {reason}"
                                            )))
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
