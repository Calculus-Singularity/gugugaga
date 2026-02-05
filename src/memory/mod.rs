//! Memory module for Gugugaga
//!
//! Provides context management, compaction, notebook, and persistent storage.
//! Follows Codex's patterns for intelligent context management.

mod compact;
mod context;
mod context_manager;
mod notebook;
mod persistent;

pub use compact::{Compactor, SUMMARIZATION_PROMPT, SUMMARY_PREFIX, COMPACTION_WARNING};
pub use context::ContextBuilder;
pub use context_manager::{ContextManager, ConversationItem, TokenUsageInfo};
pub use notebook::{
    GugugagaNotebook, NotebookSummary,
    CompletedItem, AttentionItem, MistakeEntry as NotebookMistakeEntry,
    Priority, AttentionSource,
};
pub use persistent::{PersistentMemory, TurnRole, ConversationTurn};
