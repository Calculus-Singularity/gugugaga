//! Protocol definitions for communicating with Codex app-server
//!
//! This is a minimal subset of the Codex app-server protocol needed for supervision.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// JSON-RPC request structure
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcRequest {
    pub method: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub params: Option<Value>,
}

/// JSON-RPC response structure
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcResponse {
    pub id: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<JsonRpcError>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcError {
    pub code: i32,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

/// A message from the server (notification or response)
#[derive(Debug, Clone)]
pub enum ServerMessage {
    Notification(JsonRpcRequest),
    Response(JsonRpcResponse),
}

impl ServerMessage {
    pub fn from_json(json: &Value) -> Option<Self> {
        if json.get("method").is_some() {
            serde_json::from_value(json.clone())
                .ok()
                .map(ServerMessage::Notification)
        } else if json.get("id").is_some() {
            serde_json::from_value(json.clone())
                .ok()
                .map(ServerMessage::Response)
        } else {
            None
        }
    }

    pub fn method(&self) -> Option<&str> {
        match self {
            ServerMessage::Notification(req) => Some(&req.method),
            ServerMessage::Response(_) => None,
        }
    }
}

/// Client request methods
pub mod methods {
    pub const INITIALIZE: &str = "initialize";
    pub const INITIALIZED: &str = "initialized";
    pub const THREAD_START: &str = "thread/start";
    pub const TURN_START: &str = "turn/start";
    pub const TURN_INTERRUPT: &str = "turn/interrupt";
}

/// Server notification methods we care about
pub mod notifications {
    pub const TURN_STARTED: &str = "turn/started";
    pub const TURN_COMPLETED: &str = "turn/completed";
    pub const TURN_PLAN_UPDATED: &str = "turn/plan/updated";
    pub const ITEM_STARTED: &str = "item/started";
    pub const ITEM_COMPLETED: &str = "item/completed";
    pub const ITEM_AGENT_MESSAGE_DELTA: &str = "item/agentMessage/delta";
    pub const ITEM_PLAN_DELTA: &str = "item/plan/delta";
    pub const REQUEST_USER_INPUT: &str = "item/tool/requestUserInput";
    pub const REQUEST_APPROVAL: &str = "item/commandExecution/requestApproval";
    pub const FILE_CHANGE_APPROVAL: &str = "item/fileChange/requestApproval";
    pub const ERROR: &str = "error";
}

/// Create an initialize request
pub fn create_initialize_request(id: u64, client_name: &str, version: &str) -> JsonRpcRequest {
    JsonRpcRequest {
        method: methods::INITIALIZE.to_string(),
        id: Some(id),
        params: Some(serde_json::json!({
            "clientInfo": {
                "name": client_name,
                "title": "Gugugaga",
                "version": version
            }
        })),
    }
}

/// Create an initialized notification
pub fn create_initialized_notification() -> JsonRpcRequest {
    JsonRpcRequest {
        method: methods::INITIALIZED.to_string(),
        id: None,
        params: None,
    }
}

/// Create a turn/start request
pub fn create_turn_start_request(id: u64, thread_id: &str, text: &str) -> JsonRpcRequest {
    JsonRpcRequest {
        method: methods::TURN_START.to_string(),
        id: Some(id),
        params: Some(serde_json::json!({
            "threadId": thread_id,
            "input": [{
                "type": "text",
                "text": text,
                "textElements": []
            }]
        })),
    }
}

/// Create a turn/interrupt request
pub fn create_turn_interrupt_request(id: u64, thread_id: &str) -> JsonRpcRequest {
    JsonRpcRequest {
        method: methods::TURN_INTERRUPT.to_string(),
        id: Some(id),
        params: Some(serde_json::json!({
            "threadId": thread_id
        })),
    }
}

/// Extract text from agent message delta
pub fn extract_agent_message_text(params: &Value) -> Option<String> {
    // The field is "delta", not "text"
    params.get("delta").and_then(|v| v.as_str()).map(String::from)
}

/// Extract questions from user input request
pub fn extract_user_input_questions(params: &Value) -> Option<Vec<Value>> {
    params
        .get("questions")
        .and_then(|v| v.as_array())
        .cloned()
}

/// Check if this is a plan update notification
pub fn is_plan_update(method: &str) -> bool {
    method == notifications::TURN_PLAN_UPDATED || method == notifications::ITEM_PLAN_DELTA
}
