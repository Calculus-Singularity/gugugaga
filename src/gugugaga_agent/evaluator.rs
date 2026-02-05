//! LLM Evaluator for the gugugaga agent
//!
//! Uses authentication from Codex (API key or OAuth tokens) and respects
//! user's config.toml for custom model providers (base_url, wire_api, etc.)

use crate::{Result, GugugagaError};
use futures::StreamExt;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::path::Path;
use tokio::sync::mpsc;
use tracing::{debug, error, info};

/// Evaluator that calls LLM for gugugaga decisions
pub struct Evaluator {
    client: Client,
    api_key: String,
    model: String,
    base_url: String,
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

/// Parsed LLM response with thinking and final answer separated
#[derive(Debug, Clone)]
pub struct ParsedResponse {
    /// Thinking/reasoning content (from <think> tags)
    pub thinking: Option<String>,
    /// Final response content (after </think>)
    pub response: String,
}

impl Evaluator {
    /// Parse <think>...</think> tags from LLM response, separating thinking from response
    pub fn parse_think_tags(content: &str) -> ParsedResponse {
        let re = regex::Regex::new(r"(?s)<think>(.*?)</think>").unwrap();
        
        if let Some(caps) = re.captures(content) {
            let thinking = caps.get(1).map(|m| m.as_str().trim().to_string());
            let response = re.replace_all(content, "").trim().to_string();
            ParsedResponse { thinking, response }
        } else if content.starts_with("<think>") {
            // Handle unclosed <think> tag - treat whole thing as thinking
            let thinking = content.trim_start_matches("<think>").trim().to_string();
            ParsedResponse { thinking: Some(thinking), response: String::new() }
        } else {
            ParsedResponse { thinking: None, response: content.trim().to_string() }
        }
    }

    /// Create a new evaluator, loading auth and config from codex home
    pub async fn new(codex_home: &Path) -> Result<Self> {
        let client = Client::new();

        // Load API key from Codex auth storage
        let api_key = Self::load_api_key(codex_home).await?;
        
        // Load config to get model provider settings
        let (model, base_url) = Self::load_config(codex_home).await?;

        info!("Gugugaga using model: {}, base_url: {}", model, base_url);

        Ok(Self {
            client,
            api_key,
            model,
            base_url,
        })
    }
    
    /// Load model and base_url from config.toml
    async fn load_config(codex_home: &Path) -> Result<(String, String)> {
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
                                info!("Using custom provider '{}' with base_url: {}", provider_name, base_url);
                                return Ok((model, base_url.clone()));
                            }
                        }
                    }
                }
                
                // Default to OpenAI
                return Ok((model, "https://api.openai.com/v1".to_string()));
            }
        }
        
        // Fallback defaults
        Ok(("gpt-5.2-codex".to_string(), "https://api.openai.com/v1".to_string()))
    }

    /// Load API key from Codex auth storage
    /// 
    /// Priority (auth.json FIRST to avoid env pollution from other tools):
    /// 1. access_token from auth.json tokens (ChatGPT OAuth)
    /// 2. OPENAI_API_KEY from auth.json (any format - relay services may accept different keys)
    async fn load_api_key(codex_home: &Path) -> Result<String> {
        // 1. FIRST: Try to load from Codex auth.json (preferred source)
        let auth_file = codex_home.join("auth.json");
        if auth_file.exists() {
            let content = tokio::fs::read_to_string(&auth_file).await?;
            
            if let Ok(auth) = serde_json::from_str::<AuthDotJson>(&content) {
                // 1a. Try tokens.access_token FIRST (ChatGPT OAuth - most common)
                if let Some(tokens) = &auth.tokens {
                    let access_token = tokens.access_token.trim().to_string();
                    if !access_token.is_empty() {
                        info!("Using access_token from auth.json (ChatGPT OAuth)");
                        return Ok(access_token);
                    }
                }
                
                // 1b. Try OPENAI_API_KEY field (API key auth mode)
                // Accept any key - custom relay services may accept different formats
                if let Some(api_key) = &auth.openai_api_key {
                    let key = api_key.trim().to_string();
                    if !key.is_empty() {
                        info!("Using OPENAI_API_KEY from auth.json");
                        return Ok(key);
                    }
                }
            } else {
                debug!("Failed to parse auth.json, trying raw key extraction");
                // Fallback: try to extract key manually from JSON
                if let Ok(value) = serde_json::from_str::<serde_json::Value>(&content) {
                    // Try OPENAI_API_KEY (uppercase, as stored by Codex)
                    if let Some(key) = value.get("OPENAI_API_KEY").and_then(|v| v.as_str()) {
                        let key = key.trim().to_string();
                        if !key.is_empty() {
                            info!("Using OPENAI_API_KEY from auth.json (raw extraction)");
                            return Ok(key);
                        }
                    }
                    
                    // Try tokens.access_token
                    if let Some(access_token) = value
                        .get("tokens")
                        .and_then(|t| t.get("access_token"))
                        .and_then(|v| v.as_str())
                    {
                        let key = access_token.trim().to_string();
                        if !key.is_empty() {
                            info!("Using tokens.access_token from auth.json (raw extraction)");
                            return Ok(key);
                        }
                    }
                }
            }
        }

        Err(GugugagaError::Auth(
            "No API key found. Set OPENAI_API_KEY environment variable or login via `codex login`.".to_string(),
        ))
    }

    /// Call LLM with a prompt (non-streaming, for simple cases)
    pub async fn call_llm(&self, prompt: &str) -> Result<String> {
        debug!("Calling LLM with prompt length: {}", prompt.len());

        let request = ChatRequest {
            model: self.model.clone(),
            messages: vec![ChatMessage {
                role: "user".to_string(),
                content: prompt.to_string(),
            }],
            max_tokens: 2048,
            temperature: 0.1,
            stream: false,
        };

        // Use configured base_url (supports custom providers like relay.nf.video)
        let url = format!("{}/chat/completions", self.base_url);
        
        let response = self
            .client
            .post(&url)
            .header("Authorization", format!("Bearer {}", self.api_key))
            .header("Content-Type", "application/json")
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

        let chat_response: ChatResponse = response
            .json()
            .await
            .map_err(|e| GugugagaError::LlmEvaluation(e.to_string()))?;

        let content = chat_response
            .choices
            .first()
            .map(|c| c.message.content.clone())
            .unwrap_or_default();

        // Parse and extract response
        let parsed = Self::parse_think_tags(&content);
        if let Some(thinking) = &parsed.thinking {
            debug!("LLM thinking: {}", thinking);
        }

        debug!("LLM response: {}", parsed.response);
        Ok(parsed.response)
    }
    
    /// Call LLM and return both thinking and response
    pub async fn call_llm_with_thinking(&self, prompt: &str) -> Result<ParsedResponse> {
        debug!("Calling LLM with prompt length: {}", prompt.len());

        let request = ChatRequest {
            model: self.model.clone(),
            messages: vec![ChatMessage {
                role: "user".to_string(),
                content: prompt.to_string(),
            }],
            max_tokens: 2048,
            temperature: 0.1,
            stream: false,
        };

        let url = format!("{}/chat/completions", self.base_url);
        
        let response = self
            .client
            .post(&url)
            .header("Authorization", format!("Bearer {}", self.api_key))
            .header("Content-Type", "application/json")
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

        let chat_response: ChatResponse = response
            .json()
            .await
            .map_err(|e| GugugagaError::LlmEvaluation(e.to_string()))?;

        let content = chat_response
            .choices
            .first()
            .map(|c| c.message.content.clone())
            .unwrap_or_default();

        let parsed = Self::parse_think_tags(&content);
        if let Some(thinking) = &parsed.thinking {
            debug!("LLM thinking: {}", thinking);
        }
        debug!("LLM response: {}", parsed.response);
        
        Ok(parsed)
    }

    /// Call LLM with streaming output - returns channel for real-time thinking
    pub async fn call_llm_streaming(
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

        // Use configured base_url
        let url = format!("{}/chat/completions", self.base_url);

        let response = self
            .client
            .post(&url)
            .header("Authorization", format!("Bearer {}", self.api_key))
            .header("Content-Type", "application/json")
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

        // Spawn task to process streaming response
        let mut stream = response.bytes_stream();
        tokio::spawn(async move {
            let mut buffer = String::new();
            let mut in_thinking = true; // Start with thinking mode
            
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
                                            
                                            // Check for mode switch markers
                                            if buffer.contains("---RESPONSE---") {
                                                in_thinking = false;
                                                let parts: Vec<&str> = buffer.splitn(2, "---RESPONSE---").collect();
                                                if parts.len() == 2 {
                                                    buffer = parts[1].to_string();
                                                }
                                            }
                                            
                                            // Send the content
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
}

/// Matches Codex's auth.json format exactly
/// See: codex-rs/core/src/auth/storage.rs
#[derive(Debug, Deserialize)]
struct AuthDotJson {
    /// Auth mode indicator (optional)
    #[serde(default)]
    auth_mode: Option<String>,

    /// API key stored as OPENAI_API_KEY in the JSON
    #[serde(rename = "OPENAI_API_KEY")]
    openai_api_key: Option<String>,

    /// OAuth tokens (if using ChatGPT login)
    #[serde(default)]
    tokens: Option<TokenData>,
}

/// Token data for ChatGPT OAuth authentication
#[derive(Debug, Deserialize)]
struct TokenData {
    /// The access token used for API calls
    access_token: String,

    /// Refresh token (not used by gugugaga)
    #[serde(default)]
    refresh_token: Option<String>,

    /// ID token containing user info
    #[serde(default)]
    id_token: Option<serde_json::Value>,
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
struct ModelProviderConfig {
    /// Provider name
    name: Option<String>,
    
    /// Base URL for API
    base_url: Option<String>,
    
    /// Wire API type (responses or chat)
    wire_api: Option<String>,
}
