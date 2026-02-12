//! Violation types and detection

use regex::Regex;

/// Types of violations that can be detected
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ViolationType {
    /// Agent reduced functionality instead of implementing fully
    Fallback,

    /// Agent ignored explicit user instructions
    IgnoredInstruction,

    /// Agent made changes not requested by user
    UnauthorizedChange,

    /// Agent stopped to narrate/explain/ask when user requested autonomous work
    UnnecessaryInteraction,

    /// Agent added unrequested complexity or redundant mechanisms
    OverEngineering,
}

impl std::fmt::Display for ViolationType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ViolationType::Fallback => write!(f, "FALLBACK"),
            ViolationType::IgnoredInstruction => write!(f, "IGNORED_INSTRUCTION"),
            ViolationType::UnauthorizedChange => write!(f, "UNAUTHORIZED_CHANGE"),
            ViolationType::UnnecessaryInteraction => write!(f, "UNNECESSARY_INTERACTION"),
            ViolationType::OverEngineering => write!(f, "OVER_ENGINEERING"),
        }
    }
}

/// A detected violation
#[derive(Debug, Clone)]
pub struct Violation {
    pub violation_type: ViolationType,
    pub description: String,
    pub correction: String,
}

/// Detector for rule violations using pattern matching (lightweight pre-filter).
///
/// All real violation detection is done by the LLM in `GugugagaAgent::detect_violation`.
/// This struct only holds user instructions for context and provides a minimal
/// pattern check for obvious fallback phrases.
pub struct ViolationDetector {
    /// User's explicit instructions to check against
    user_instructions: Vec<String>,

    /// Patterns that indicate fallback behavior
    fallback_patterns: Vec<Regex>,
}

impl ViolationDetector {
    pub fn new() -> Self {
        Self {
            user_instructions: Vec::new(),
            fallback_patterns: Self::compile_fallback_patterns(),
        }
    }

    pub fn with_instructions(mut self, instructions: Vec<String>) -> Self {
        self.user_instructions = instructions;
        self
    }

    fn compile_fallback_patterns() -> Vec<Regex> {
        vec![
            Regex::new(r"(?i)for\s+now[,\s]+(?:I'll|we'll|let's)\s+(?:just|simply)").unwrap(),
            Regex::new(r"(?i)(?:simplified|basic)\s+(?:version|implementation)").unwrap(),
            Regex::new(r"(?i)(?:skip|omit|leave\s+out)\s+.{0,30}\s+for\s+now").unwrap(),
            Regex::new(r"(?i)(?:I'll|we'll|let's)\s+(?:skip|omit)").unwrap(),
            Regex::new(r"(?i)placeholder\s+(?:for\s+now|implementation)").unwrap(),
            Regex::new(r"(?i)TODO:\s*implement").unwrap(),
            Regex::new(r"(?i)not\s+(?:yet\s+)?implemented").unwrap(),
            Regex::new(r"暂时").unwrap(),
            Regex::new(r"先这样").unwrap(),
            Regex::new(r"简化版").unwrap(),
            Regex::new(r"待实现").unwrap(),
            Regex::new(r"后面再").unwrap(),
        ]
    }

    /// Check agent output for violations (lightweight pre-filter only).
    /// Semantic violations (ignored instructions, unnecessary interaction, etc.)
    /// are handled entirely by the LLM evaluation.
    pub fn check(&self, agent_output: &str) -> Vec<Violation> {
        let mut violations = Vec::new();

        // Check for fallback patterns
        for pattern in &self.fallback_patterns {
            if pattern.is_match(agent_output) {
                violations.push(Violation {
                    violation_type: ViolationType::Fallback,
                    description: format!("Pattern: {}", pattern.as_str()),
                    correction: String::new(), // LLM will provide
                });
                break;
            }
        }

        violations
    }
}

impl Default for ViolationDetector {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_fallback_detection() {
        let detector = ViolationDetector::new();

        let fallback_messages = vec![
            "For now, I'll just add a simple placeholder",
            "This is a simplified version",
            "I'll skip this for now",
            "暂时先这样实现",
        ];

        for msg in fallback_messages {
            let violations = detector.check(msg);
            assert!(
                violations.iter().any(|v| v.violation_type == ViolationType::Fallback),
                "Should detect fallback in: {}",
                msg
            );
        }
    }
}
