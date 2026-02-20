//! Memory module for Gugugaga
//!
//! Provides context management, compaction, notebook, and persistent storage.
//! Follows Codex's patterns for intelligent context management.

pub mod compact;
mod context;
mod notebook;
mod persistent;
pub mod session_store;

pub use compact::Compactor;
pub use context::ContextBuilder;
pub use notebook::{
    AttentionItem, AttentionSource, CompletedItem, GugugagaNotebook,
    MistakeEntry as NotebookMistakeEntry, NotebookSummary, Priority,
};
pub use persistent::{ConversationTurn, PersistentMemory, TurnRole};
pub use session_store::SessionStore;
