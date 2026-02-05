//! Violation types and detection

use regex::Regex;

/// Types of violations that can be detected
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ViolationType {
    /// Agent reduced functionality instead of implementing fully
    Fallback,

    /// Agent ignored explicit user instructions
    IgnoredInstruction,

    /// Agent used built-in todo/plan instead of moonissues
    UsedBuiltinTodo,

    /// Agent made changes not requested by user
    UnauthorizedChange,
}

impl std::fmt::Display for ViolationType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ViolationType::Fallback => write!(f, "FALLBACK"),
            ViolationType::IgnoredInstruction => write!(f, "IGNORED_INSTRUCTION"),
            ViolationType::UsedBuiltinTodo => write!(f, "USED_BUILTIN_TODO"),
            ViolationType::UnauthorizedChange => write!(f, "UNAUTHORIZED_CHANGE"),
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

/// Detector for rule violations using pattern matching
pub struct ViolationDetector {
    /// User's explicit instructions to check against
    user_instructions: Vec<String>,

    /// Patterns that indicate fallback behavior
    fallback_patterns: Vec<Regex>,

    /// Patterns that indicate built-in todo usage
    builtin_todo_patterns: Vec<Regex>,
}

impl ViolationDetector {
    pub fn new() -> Self {
        Self {
            user_instructions: Vec::new(),
            fallback_patterns: Self::compile_fallback_patterns(),
            builtin_todo_patterns: Self::compile_builtin_todo_patterns(),
        }
    }

    pub fn with_instructions(mut self, instructions: Vec<String>) -> Self {
        self.user_instructions = instructions;
        self
    }

    fn compile_fallback_patterns() -> Vec<Regex> {
        vec![
            // English patterns
            Regex::new(r"(?i)for\s+now[,\s]+(?:I'll|we'll|let's)\s+(?:just|simply)").unwrap(),
            Regex::new(r"(?i)(?:simplified|basic)\s+(?:version|implementation)").unwrap(),
            Regex::new(r"(?i)(?:skip|omit|leave\s+out)\s+.{0,30}\s+for\s+now").unwrap(),
            Regex::new(r"(?i)(?:I'll|we'll|let's)\s+(?:skip|omit)").unwrap(),
            Regex::new(r"(?i)placeholder\s+(?:for\s+now|implementation)").unwrap(),
            Regex::new(r"(?i)TODO:\s*implement").unwrap(),
            Regex::new(r"(?i)not\s+(?:yet\s+)?implemented").unwrap(),
            
            // Chinese patterns
            Regex::new(r"暂时").unwrap(),
            Regex::new(r"先这样").unwrap(),
            Regex::new(r"简化版").unwrap(),
            Regex::new(r"待实现").unwrap(),
            Regex::new(r"后面再").unwrap(),
        ]
    }

    fn compile_builtin_todo_patterns() -> Vec<Regex> {
        vec![
            // Detect update_plan tool usage
            Regex::new(r"(?i)update_plan").unwrap(),
            Regex::new(r#"(?i)"?tool"?\s*:\s*"?update_plan"?"#).unwrap(),
        ]
    }

    /// Check agent output for violations
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

        // Check for built-in todo usage (but not if moonissues is mentioned)
        if !agent_output.contains("moonissues") {
            for pattern in &self.builtin_todo_patterns {
                if pattern.is_match(agent_output) {
                    violations.push(Violation {
                        violation_type: ViolationType::UsedBuiltinTodo,
                        description: "Used update_plan".to_string(),
                        correction: String::new(),
                    });
                    break;
                }
            }
        }

        // Check for ignored instructions
        for instruction in &self.user_instructions {
            if self.is_instruction_violated(instruction, agent_output) {
                violations.push(Violation {
                    violation_type: ViolationType::IgnoredInstruction,
                    description: format!("Violated: {}", instruction),
                    correction: String::new(),
                });
            }
        }

        violations
    }

    /// Check if an instruction appears to be violated
    fn is_instruction_violated(&self, instruction: &str, agent_output: &str) -> bool {
        let instruction_lower = instruction.to_lowercase();
        let output_lower = agent_output.to_lowercase();

        // Check for "dont/don't" instructions
        if instruction_lower.contains("dont") || instruction_lower.contains("don't") {
            // Extract what shouldn't be done
            let forbidden: Vec<&str> = instruction_lower
                .split(&['d', 'o', 'n', '\'', 't', ' '][..])
                .filter(|s| !s.is_empty() && s.len() > 2)
                .collect();

            for word in forbidden {
                let word = word.trim();
                if !word.is_empty() && output_lower.contains(word) {
                    return true;
                }
            }
        }

        false
    }

    /// Quick check for builtin todo usage
    pub fn detect_builtin_plan_usage(agent_message: &str) -> bool {
        let has_update_plan = agent_message.contains("update_plan");
        let has_moonissues = agent_message.contains("moonissues");

        has_update_plan && !has_moonissues
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

    #[test]
    fn test_builtin_todo_detection() {
        let detector = ViolationDetector::new();

        // Should detect
        let violations = detector.check(r#"{"tool": "update_plan"}"#);
        assert!(violations.iter().any(|v| v.violation_type == ViolationType::UsedBuiltinTodo));

        // Should not detect when moonissues is mentioned
        let violations = detector.check("I'll use moonissues instead of update_plan");
        assert!(!violations.iter().any(|v| v.violation_type == ViolationType::UsedBuiltinTodo));
    }

    #[test]
    fn test_detect_builtin_plan_usage() {
        assert!(ViolationDetector::detect_builtin_plan_usage("update_plan"));
        assert!(!ViolationDetector::detect_builtin_plan_usage("moonissues create task"));
        assert!(!ViolationDetector::detect_builtin_plan_usage("use moonissues not update_plan"));
    }
}
