//! Gugugaga - Intelligent Gugugaga
//!
//! An independent agent that wraps Codex app-server to:
//! - Monitor and correct Codex behavior
//! - Maintain persistent memory across context compaction
//! - Intelligently filter user interaction requests

pub mod interceptor;
pub mod memory;
pub mod protocol;
pub mod rules;
pub mod gugugaga_agent;
pub mod trust;
pub mod tui;

pub use interceptor::Interceptor;
pub use memory::PersistentMemory;
pub use rules::{Violation, ViolationDetector, ViolationType};
pub use gugugaga_agent::{EvaluationResult, GugugagaAgent, UserInputAnalysis};

use std::path::PathBuf;

/// Configuration for Gugugaga
#[derive(Debug, Clone)]
pub struct GugugagaConfig {
    /// Path to the persistent memory file
    pub memory_file: PathBuf,

    /// Working directory for the project
    pub cwd: PathBuf,

    /// Path to codex home directory
    pub codex_home: PathBuf,

    /// Whether to run in strict mode (interrupt on any violation)
    pub strict_mode: bool,

    /// Whether to show verbose output including evaluations
    pub verbose: bool,
}

impl GugugagaConfig {
    pub fn new(cwd: PathBuf, codex_home: PathBuf) -> Self {
        let memory_file = codex_home.join("gugugaga").join("memory.md");
        Self {
            memory_file,
            cwd,
            codex_home,
            strict_mode: false,
            verbose: false,
        }
    }

    pub fn with_memory_file(mut self, path: PathBuf) -> Self {
        self.memory_file = path;
        self
    }

    pub fn with_strict_mode(mut self, strict: bool) -> Self {
        self.strict_mode = strict;
        self
    }

    pub fn with_verbose(mut self, verbose: bool) -> Self {
        self.verbose = verbose;
        self
    }
}

/// Result type for Gugugaga operations
pub type Result<T> = std::result::Result<T, GugugagaError>;

/// Errors that can occur in Gugugaga
#[derive(Debug, thiserror::Error)]
pub enum GugugagaError {
    #[error("Failed to start app-server: {0}")]
    AppServerStart(String),

    #[error("Failed to communicate with app-server: {0}")]
    Communication(String),

    #[error("Memory error: {0}")]
    Memory(String),

    #[error("LLM evaluation error: {0}")]
    LlmEvaluation(String),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("Authentication error: {0}")]
    Auth(String),
}
