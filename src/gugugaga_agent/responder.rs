//! Response parser for the gugugaga agent.
//!
//! Parsing strategy (inspired by Codex's `parse_review_output_event`):
//! 1. Try `serde_json::from_str` on the full response text.
//! 2. If that fails, extract the first `{…}` JSON substring and retry.
//! 3. If that also fails, fall back to legacy text patterns (OK: / VIOLATION:).
//! 4. If nothing matches, return a safe default — **never return Err**.

use super::{EvaluationResult, UserInputAnalysis};
use crate::rules::{Violation, ViolationType};
use crate::Result;
use serde::Deserialize;
use tracing::debug;

// ── JSON schemas for LLM responses ──────────────────────────────────

/// The structured response we ask the LLM to produce for violation checks.
///
/// ```json
/// // No violation:
/// { "result": "ok", "summary": "Codex executed the requested task." }
///
/// // Violation found:
/// { "result": "violation", "type": "UNNECESSARY_INTERACTION",
///   "description": "Codex stopped to narrate its plan",
///   "correction": "Execute the task silently without narration" }
/// ```
#[derive(Debug, Deserialize)]
struct CheckResponseJson {
    result: String,

    #[serde(default)]
    summary: Option<String>,

    /// Violation type (only when result == "violation")
    #[serde(rename = "type")]
    #[serde(default)]
    violation_type: Option<String>,

    /// What went wrong
    #[serde(default)]
    description: Option<String>,

    /// How to fix it
    #[serde(default)]
    correction: Option<String>,
}

/// Structured response for evaluation requests.
#[derive(Debug, Deserialize)]
struct EvalResponseJson {
    action: String,

    #[serde(default)]
    content: Option<String>,
}

/// Structured response for user-input analysis.
#[derive(Debug, Deserialize)]
struct UserInputJson {
    #[serde(default)]
    main_goal: Option<String>,

    #[serde(default)]
    constraints: Vec<String>,

    #[serde(default)]
    explicit_instructions: Vec<String>,
}

// ── CheckResult (parsed) ────────────────────────────────────────────

/// The final parsed check result — always succeeds.
pub struct ParsedCheck {
    pub violation: Option<Violation>,
    pub summary: String,
}

// ── Responder ───────────────────────────────────────────────────────

/// Parses LLM responses using JSON-first strategy with text fallbacks.
pub struct Responder;

impl Responder {
    pub fn new() -> Self {
        Self
    }

    // ── Violation check parsing ─────────────────────────────────

    /// Parse a violation-check response. **Never returns Err.**
    pub fn parse_check_response(&self, response: &str) -> ParsedCheck {
        let text = response.trim();

        // Layer 1: try full JSON parse
        if let Some(result) = Self::try_json_check(text) {
            return result;
        }

        // Layer 2: extract first JSON object substring and retry
        if let Some(json_str) = Self::extract_json_object(text) {
            if let Some(result) = Self::try_json_check(json_str) {
                return result;
            }
        }

        // Layer 3: legacy text-based fallback
        Self::fallback_text_check(text)
    }

    /// Try to deserialize `text` as a `CheckResponseJson`.
    fn try_json_check(text: &str) -> Option<ParsedCheck> {
        let parsed: CheckResponseJson = serde_json::from_str(text).ok()?;

        match parsed.result.to_lowercase().as_str() {
            "ok" | "pass" | "normal" => Some(ParsedCheck {
                violation: None,
                summary: parsed
                    .summary
                    .unwrap_or_else(|| "Check complete".to_string()),
            }),
            "violation" | "violated" | "fail" => {
                let vtype = parsed
                    .violation_type
                    .as_deref()
                    .map(Self::parse_violation_type)
                    .unwrap_or(ViolationType::Fallback);
                let description = parsed
                    .description
                    .unwrap_or_else(|| "Violation detected".to_string());
                let correction = parsed.correction.unwrap_or_else(|| description.clone());

                Some(ParsedCheck {
                    summary: description.clone(),
                    violation: Some(Violation {
                        violation_type: vtype,
                        description,
                        correction,
                    }),
                })
            }
            other => {
                debug!("Unknown result value from LLM JSON: {other}");
                Some(ParsedCheck {
                    violation: None,
                    summary: parsed
                        .summary
                        .unwrap_or_else(|| "Check complete".to_string()),
                })
            }
        }
    }

    /// Fallback: parse legacy text patterns.
    fn fallback_text_check(text: &str) -> ParsedCheck {
        // "OK: ..."
        if text.starts_with("OK:") || text.starts_with("OK：") {
            let summary = text
                .trim_start_matches("OK:")
                .trim_start_matches("OK：")
                .trim();
            return ParsedCheck {
                violation: None,
                summary: if summary.is_empty() {
                    "Check complete".to_string()
                } else {
                    summary.to_string()
                },
            };
        }

        // "VIOLATION: TYPE - desc - correction" or similar
        if let Some(pos) = text.find("VIOLATION:") {
            let after = text[pos + "VIOLATION:".len()..].trim();
            return Self::parse_violation_text(after);
        }

        // Nothing matched — treat as "OK" with the full text as summary
        debug!("Could not parse LLM response, treating as OK: {text}");
        ParsedCheck {
            violation: None,
            summary: if text.is_empty() {
                "Check complete".to_string()
            } else {
                // Take first 200 chars as summary
                let end = text
                    .char_indices()
                    .nth(200)
                    .map(|(i, _)| i)
                    .unwrap_or(text.len());
                text[..end].to_string()
            },
        }
    }

    /// Parse the text after "VIOLATION:" into a `ParsedCheck`.
    fn parse_violation_text(text: &str) -> ParsedCheck {
        // Try to split: TYPE separator rest
        // Accept `-`, `–`, `:` as separators
        let parts: Vec<&str> = text.splitn(2, ['-', '–', ':']).collect();

        let (type_str, rest) = if parts.len() >= 2 {
            (parts[0].trim(), parts[1].trim())
        } else {
            ("FALLBACK", text)
        };

        let vtype = Self::parse_violation_type(type_str);

        // Try to split rest into description + correction using last " - "
        let (description, correction) = if let Some(sep) = rest.rfind(" - ") {
            let desc = rest[..sep].trim().to_string();
            let corr = rest[sep + 3..].trim().to_string();
            if corr.is_empty() {
                (rest.to_string(), rest.to_string())
            } else {
                (desc, corr)
            }
        } else {
            (rest.to_string(), rest.to_string())
        };

        ParsedCheck {
            summary: description.clone(),
            violation: Some(Violation {
                violation_type: vtype,
                description,
                correction,
            }),
        }
    }

    // ── Evaluation parsing ──────────────────────────────────────

    /// Parse an evaluation response (AUTO_REPLY / CORRECT / FORWARD_TO_USER).
    pub fn parse_evaluation_response(&self, response: &str) -> Result<EvaluationResult> {
        let text = response.trim();

        // Layer 1: JSON
        if let Ok(parsed) = serde_json::from_str::<EvalResponseJson>(text) {
            return Ok(Self::eval_from_json(&parsed));
        }

        // Layer 2: extract JSON substring
        if let Some(json_str) = Self::extract_json_object(text) {
            if let Ok(parsed) = serde_json::from_str::<EvalResponseJson>(json_str) {
                return Ok(Self::eval_from_json(&parsed));
            }
        }

        // Layer 3: text pattern
        let upper = text.to_uppercase();
        if upper.starts_with("AUTO_REPLY:") {
            let content = text["AUTO_REPLY:".len()..].trim();
            return Ok(EvaluationResult::AutoReply(content.to_string()));
        }
        if upper.starts_with("CORRECT:") {
            let content = text["CORRECT:".len()..].trim();
            return Ok(EvaluationResult::Correct(content.to_string()));
        }

        Ok(EvaluationResult::ForwardToUser)
    }

    fn eval_from_json(parsed: &EvalResponseJson) -> EvaluationResult {
        let content = parsed.content.clone().unwrap_or_default();
        match parsed.action.to_uppercase().as_str() {
            "AUTO_REPLY" => EvaluationResult::AutoReply(content),
            "CORRECT" => EvaluationResult::Correct(content),
            _ => EvaluationResult::ForwardToUser,
        }
    }

    // ── User input analysis ─────────────────────────────────────

    /// Parse user input analysis response.
    pub fn parse_user_input_analysis(&self, response: &str) -> Result<UserInputAnalysis> {
        let text = response.trim();

        // Layer 1: full JSON
        if let Ok(parsed) = serde_json::from_str::<UserInputJson>(text) {
            return Ok(Self::user_input_from_json(parsed));
        }

        // Layer 2: extract JSON substring
        if let Some(json_str) = Self::extract_json_object(text) {
            if let Ok(parsed) = serde_json::from_str::<UserInputJson>(json_str) {
                return Ok(Self::user_input_from_json(parsed));
            }
        }

        // Layer 3: empty result
        Ok(UserInputAnalysis {
            main_goal: None,
            constraints: Vec::new(),
            explicit_instructions: Vec::new(),
        })
    }

    fn user_input_from_json(parsed: UserInputJson) -> UserInputAnalysis {
        UserInputAnalysis {
            main_goal: parsed.main_goal,
            constraints: parsed.constraints,
            explicit_instructions: parsed.explicit_instructions,
        }
    }

    // ── Shared utilities ────────────────────────────────────────

    /// Extract the first `{…}` JSON object from a text blob.
    /// Handles nested braces properly.
    fn extract_json_object(text: &str) -> Option<&str> {
        let start = text.find('{')?;
        let mut depth = 0i32;
        let mut in_string = false;
        let mut escape_next = false;

        for (i, ch) in text[start..].char_indices() {
            if escape_next {
                escape_next = false;
                continue;
            }
            match ch {
                '\\' if in_string => escape_next = true,
                '"' => in_string = !in_string,
                '{' if !in_string => depth += 1,
                '}' if !in_string => {
                    depth -= 1;
                    if depth == 0 {
                        return text.get(start..start + i + 1);
                    }
                }
                _ => {}
            }
        }
        None
    }

    /// Parse a violation type string into the enum.
    fn parse_violation_type(s: &str) -> ViolationType {
        match s.trim().to_uppercase().replace(' ', "_").as_str() {
            "FALLBACK" => ViolationType::Fallback,
            "IGNORED_INSTRUCTION" => ViolationType::IgnoredInstruction,
            "UNAUTHORIZED_CHANGE" => ViolationType::UnauthorizedChange,
            "UNNECESSARY_INTERACTION" => ViolationType::UnnecessaryInteraction,
            "OVER_ENGINEERING" => ViolationType::OverEngineering,
            _ => ViolationType::Fallback,
        }
    }
}

impl Default for Responder {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_json_ok() {
        let r = Responder::new();
        let resp = r#"{"result": "ok", "summary": "Codex executed normally."}"#;
        let parsed = r.parse_check_response(resp);
        assert!(parsed.violation.is_none());
        assert_eq!(parsed.summary, "Codex executed normally.");
    }

    #[test]
    fn test_json_violation() {
        let r = Responder::new();
        let resp = r#"{"result": "violation", "type": "UNNECESSARY_INTERACTION", "description": "Codex narrated its plan", "correction": "Execute silently"}"#;
        let parsed = r.parse_check_response(resp);
        assert!(parsed.violation.is_some());
        let v = parsed.violation.unwrap();
        assert_eq!(v.violation_type, ViolationType::UnnecessaryInteraction);
        assert_eq!(v.description, "Codex narrated its plan");
        assert_eq!(v.correction, "Execute silently");
    }

    #[test]
    fn test_json_embedded_in_text() {
        let r = Responder::new();
        let resp = r#"Here is my analysis: {"result": "ok", "summary": "All good"} end"#;
        let parsed = r.parse_check_response(resp);
        assert!(parsed.violation.is_none());
        assert_eq!(parsed.summary, "All good");
    }

    #[test]
    fn test_legacy_ok_text() {
        let r = Responder::new();
        let parsed = r.parse_check_response("OK: Everything looks fine");
        assert!(parsed.violation.is_none());
        assert_eq!(parsed.summary, "Everything looks fine");
    }

    #[test]
    fn test_legacy_violation_text() {
        let r = Responder::new();
        let parsed = r.parse_check_response(
            "VIOLATION: FALLBACK - Codex said skip for now - Implement fully",
        );
        assert!(parsed.violation.is_some());
        let v = parsed.violation.unwrap();
        assert_eq!(v.violation_type, ViolationType::Fallback);
    }

    #[test]
    fn test_unparseable_is_ok() {
        let r = Responder::new();
        let parsed =
            r.parse_check_response("I think everything is fine, Codex did what was asked.");
        assert!(parsed.violation.is_none());
        assert!(!parsed.summary.is_empty());
    }

    #[test]
    fn test_extract_json_nested() {
        let text = r#"prefix {"a": {"b": 1}, "c": 2} suffix"#;
        let extracted = Responder::extract_json_object(text);
        assert_eq!(extracted, Some(r#"{"a": {"b": 1}, "c": 2}"#));
    }
}
