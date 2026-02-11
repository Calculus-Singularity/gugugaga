//! Memory module for Gugugaga
//!
//! Provides context management, compaction, notebook, and persistent storage.
//! Follows Codex's patterns for intelligent context management.

pub mod compact;
mod context;
mod notebook;
mod persistent;

pub use compact::Compactor;
pub use context::ContextBuilder;
pub use notebook::{
    GugugagaNotebook, NotebookSummary,
    CompletedItem, AttentionItem, MistakeEntry as NotebookMistakeEntry,
    Priority, AttentionSource,
};
pub use persistent::{PersistentMemory, TurnRole, ConversationTurn};
