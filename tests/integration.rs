//! Integration tests for the Codex Supervisor

use gugugaga::memory::PersistentMemory;
use gugugaga::rules::{ViolationDetector, ViolationType};
use gugugaga::gugugaga_agent::Responder;
use gugugaga::GugugagaConfig;
use std::path::PathBuf;
use tempfile::TempDir;

/// Test that persistent memory can be created and saved
#[tokio::test]
async fn test_persistent_memory_creation() {
    let temp_dir = TempDir::new().unwrap();
    let memory_file = temp_dir.path().join("memory.md");

    let _memory = PersistentMemory::new(memory_file.clone()).await.unwrap();
    assert!(memory_file.exists());
}

/// Test that user instructions are recorded and persisted
#[tokio::test]
async fn test_record_user_instruction() {
    let temp_dir = TempDir::new().unwrap();
    let memory_file = temp_dir.path().join("memory.md");

    let mut memory = PersistentMemory::new(memory_file.clone()).await.unwrap();
    memory
        .record_user_instruction("Never use any type")
        .await
        .unwrap();

    // Reload and verify
    let memory2 = PersistentMemory::new(memory_file).await.unwrap();
    assert!(!memory2.user_instructions.is_empty());
    assert!(memory2.user_instructions[0].content.contains("any type"));
}

/// Test that task objectives are saved correctly
#[tokio::test]
async fn test_set_task_objective() {
    let temp_dir = TempDir::new().unwrap();
    let memory_file = temp_dir.path().join("memory.md");

    let mut memory = PersistentMemory::new(memory_file.clone()).await.unwrap();
    memory
        .set_task_objective(
            "Implement user authentication",
            vec!["Use JWT".to_string(), "No sessions".to_string()],
        )
        .await
        .unwrap();

    let memory2 = PersistentMemory::new(memory_file).await.unwrap();
    assert!(memory2.current_task.is_some());
    let task = memory2.current_task.unwrap();
    assert!(task.main_goal.contains("authentication"));
    assert_eq!(task.constraints.len(), 2);
}

/// Test that context is built correctly
#[tokio::test]
async fn test_build_context() {
    let temp_dir = TempDir::new().unwrap();
    let memory_file = temp_dir.path().join("memory.md");

    let mut memory = PersistentMemory::new(memory_file).await.unwrap();
    memory
        .record_user_instruction("All code must have tests")
        .await
        .unwrap();
    memory
        .set_task_objective("Build API", vec!["REST only".to_string()])
        .await
        .unwrap();

    let context = memory.build_context();
    assert!(context.contains("All code must have tests"));
    assert!(context.contains("Build API"));
    assert!(context.contains("REST only"));
}

/// Test violation detection for fallback patterns
#[test]
fn test_detect_fallback_violations() {
    let detector = ViolationDetector::new();

    let fallback_messages = vec![
        "For now, I'll just add a simple placeholder",
        "This is a simplified version of what you asked for",
        "I'll skip the error handling for now",
        "暂时先这样实现",
        "TODO: implement full validation",
    ];

    for msg in fallback_messages {
        let violations = detector.check(msg);
        assert!(
            violations
                .iter()
                .any(|v| v.violation_type == ViolationType::Fallback),
            "Should detect fallback in: {}",
            msg
        );
    }
}

/// Test that normal messages don't trigger violations
#[test]
fn test_no_false_positives() {
    let detector = ViolationDetector::new();

    let normal_messages = vec![
        "I've implemented the authentication module with JWT support.",
        "The tests are now passing.",
        "I've added error handling for all edge cases.",
        "Using issue tracker to manage tasks.",
    ];

    for msg in normal_messages {
        let violations = detector.check(msg);
        assert!(
            violations.is_empty(),
            "Should not detect violations in: {}",
            msg
        );
    }
}

/// Test responder parsing
#[test]
fn test_responder_parse_evaluation() {
    use gugugaga::EvaluationResult;

    let responder = Responder::new();

    // Test AUTO_REPLY parsing
    let result = responder
        .parse_evaluation_response("AUTO_REPLY: Yes, continue")
        .unwrap();
    match result {
        EvaluationResult::AutoReply(msg) => assert!(msg.contains("continue")),
        _ => panic!("Expected AutoReply"),
    }

    // Test CORRECT parsing
    let result = responder
        .parse_evaluation_response("CORRECT: You made a mistake, fix it")
        .unwrap();
    match result {
        EvaluationResult::Correct(msg) => assert!(msg.contains("mistake")),
        _ => panic!("Expected Correct"),
    }

    // Test FORWARD_TO_USER parsing
    let result = responder
        .parse_evaluation_response("FORWARD_TO_USER: needs human decision")
        .unwrap();
    match result {
        EvaluationResult::ForwardToUser => {}
        _ => panic!("Expected ForwardToUser"),
    }
}

/// Test supervisor config creation
#[test]
fn test_supervisor_config() {
    let cwd = PathBuf::from("/tmp/project");
    let codex_home = PathBuf::from("/home/user/.codex");

    let config = GugugagaConfig::new(cwd.clone(), codex_home.clone())
        .with_strict_mode(true)
        .with_verbose(true);

    assert_eq!(config.cwd, cwd);
    assert_eq!(config.codex_home, codex_home);
    assert!(config.strict_mode);
    assert!(config.verbose);
    assert!(config.memory_file.to_string_lossy().contains("gugugaga"));
}

/// Test behavior logging
#[tokio::test]
async fn test_behavior_logging() {
    let temp_dir = TempDir::new().unwrap();
    let memory_file = temp_dir.path().join("memory.md");

    let mut memory = PersistentMemory::new(memory_file.clone()).await.unwrap();

    memory
        .record_behavior("Attempted fallback", true)
        .await
        .unwrap();
    memory
        .record_behavior("Completed task", false)
        .await
        .unwrap();

    let memory2 = PersistentMemory::new(memory_file).await.unwrap();
    assert_eq!(memory2.behavior_log.len(), 2);
    assert!(memory2.behavior_log[0].was_corrected);
    assert!(!memory2.behavior_log[1].was_corrected);
}
