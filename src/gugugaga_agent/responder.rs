//! Response parser and auto-responder for the gugugaga

use super::{EvaluationResult, UserInputAnalysis};
use crate::rules::{Violation, ViolationType};
use crate::{Result, GugugagaError};
use regex::Regex;

/// Parses LLM responses and generates automatic replies
pub struct Responder {
    action_pattern: Regex,
    violation_pattern: Regex,
}

impl Responder {
    pub fn new() -> Self {
        Self {
            action_pattern: Regex::new(r"(?i)(AUTO_REPLY|CORRECT|FORWARD_TO_USER):\s*(.*)").unwrap(),
            // Match: VIOLATION: [type] - [description] - [correction]
            violation_pattern: Regex::new(r"(?i)VIOLATION:\s*(\w+)\s*-\s*([^-]+)\s*-\s*(.*)").unwrap(),
        }
    }

    /// Parse evaluation response from LLM
    pub fn parse_evaluation_response(&self, response: &str) -> Result<EvaluationResult> {
        let response = response.trim();

        // Check for explicit action patterns
        if let Some(caps) = self.action_pattern.captures(response) {
            let action = caps.get(1).map(|m| m.as_str()).unwrap_or("");
            let content = caps.get(2).map(|m| m.as_str()).unwrap_or("").trim();

            return match action.to_uppercase().as_str() {
                "AUTO_REPLY" => Ok(EvaluationResult::AutoReply(content.to_string())),
                "CORRECT" => Ok(EvaluationResult::Correct(content.to_string())),
                "FORWARD_TO_USER" => Ok(EvaluationResult::ForwardToUser),
                _ => Ok(EvaluationResult::ForwardToUser),
            };
        }

        // No pattern match - forward to user for decision
        Ok(EvaluationResult::ForwardToUser)
    }

    /// Parse violation detection response from LLM
    /// Format: VIOLATION: [type] - [description] - [correction]
    pub fn parse_violation_response(&self, response: &str) -> Result<Violation> {
        if let Some(caps) = self.violation_pattern.captures(response) {
            let type_str = caps.get(1).map(|m| m.as_str()).unwrap_or("");
            let description = caps.get(2).map(|m| m.as_str()).unwrap_or("").trim().to_string();
            let correction = caps.get(3).map(|m| m.as_str()).unwrap_or("").trim().to_string();

            let violation_type = match type_str.to_uppercase().as_str() {
                "FALLBACK" => ViolationType::Fallback,
                "IGNORED_INSTRUCTION" => ViolationType::IgnoredInstruction,
                "UNAUTHORIZED_CHANGE" => ViolationType::UnauthorizedChange,
                _ => ViolationType::Fallback,
            };

            // LLM must provide correction
            if correction.is_empty() {
                return Err(GugugagaError::LlmEvaluation(
                    "LLMLLM did not provide correction".to_string(),
                ));
            }

            return Ok(Violation {
                violation_type,
                description,
                correction,
            });
        }

        Err(GugugagaError::LlmEvaluation(
            "Cannot parse violation response".to_string(),
        ))
    }

    /// Parse user input analysis response from LLM
    pub fn parse_user_input_analysis(&self, response: &str) -> Result<UserInputAnalysis> {
        // Try to parse as JSON
        if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(response) {
            let main_goal = parsed
                .get("main_goal")
                .and_then(|v| v.as_str())
                .map(String::from);

            let constraints = parsed
                .get("constraints")
                .and_then(|v| v.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|v| v.as_str().map(String::from))
                        .collect()
                })
                .unwrap_or_default();

            let explicit_instructions = parsed
                .get("explicit_instructions")
                .and_then(|v| v.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|v| v.as_str().map(String::from))
                        .collect()
                })
                .unwrap_or_default();

            return Ok(UserInputAnalysis {
                main_goal,
                constraints,
                explicit_instructions,
            });
        }

        // If JSON parsing fails, return empty analysis
        Ok(UserInputAnalysis {
            main_goal: None,
            constraints: Vec::new(),
            explicit_instructions: Vec::new(),
        })
    }
}

impl Default for Responder {
    fn default() -> Self {
        Self::new()
    }
}
