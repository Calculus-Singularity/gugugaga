//! Per-thread session state caching.
//!
//! ALL state lives inside each thread's session. There is NO global persistent
//! state across conversations. New thread = blank slate. Resume thread = full
//! restore. Each session is saved to sessions/{thread_id}.json on exit.

use crate::Result;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use tokio::fs;
use tokio::io::AsyncWriteExt;
use tracing::{debug, info, warn};

use super::notebook::{AttentionItem, CompletedItem, GugugagaNotebook, MistakeEntry};
use super::persistent::{
    BehaviorEntry, ConversationTurn, Decision, PersistentMemory, TaskObjective, UserInstruction,
};

/// Full snapshot of PersistentMemory (everything, not just "session-scoped").
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemorySnapshot {
    pub user_instructions: Vec<UserInstruction>,
    pub current_task: Option<TaskObjective>,
    pub decisions: Vec<Decision>,
    pub behavior_log: Vec<BehaviorEntry>,
    pub conversation_history: Vec<ConversationTurn>,
}

/// Full snapshot of GugugagaNotebook (everything).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NotebookSnapshot {
    pub current_activity: Option<String>,
    pub completed: Vec<CompletedItem>,
    pub attention: Vec<AttentionItem>,
    pub mistakes: Vec<MistakeEntry>,
}

/// Combined session snapshot saved per thread.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionSnapshot {
    pub thread_id: String,
    pub saved_at: DateTime<Utc>,
    pub memory: MemorySnapshot,
    pub notebook: NotebookSnapshot,
}

/// Manages per-thread session state files.
///
/// Directory layout:
///   {sessions_dir}/{thread_id}.json
pub struct SessionStore {
    sessions_dir: PathBuf,
}

impl SessionStore {
    /// Create a new session store. Creates the directory if needed.
    pub async fn new(project_dir: &Path) -> Result<Self> {
        let sessions_dir = project_dir.join("sessions");
        fs::create_dir_all(&sessions_dir).await?;
        Ok(Self { sessions_dir })
    }

    /// Save the FULL current state for a given thread.
    pub async fn save(
        &self,
        thread_id: &str,
        memory: &PersistentMemory,
        notebook: &GugugagaNotebook,
    ) -> Result<()> {
        let snapshot = SessionSnapshot {
            thread_id: thread_id.to_string(),
            saved_at: Utc::now(),
            memory: MemorySnapshot {
                user_instructions: memory.user_instructions.clone(),
                current_task: memory.current_task.clone(),
                decisions: memory.decisions.clone(),
                behavior_log: memory.behavior_log.clone(),
                conversation_history: memory.conversation_history.clone(),
            },
            notebook: NotebookSnapshot {
                current_activity: notebook.current_activity.clone(),
                completed: notebook.completed.clone(),
                attention: notebook.attention.clone(),
                mistakes: notebook.mistakes.clone(),
            },
        };

        let path = self.session_path(thread_id);
        let content = serde_json::to_string_pretty(&snapshot)?;
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(&path)
            .await?;
        file.write_all(content.as_bytes()).await?;
        file.flush().await?;

        debug!("Saved session state for thread {}", thread_id);
        Ok(())
    }

    /// Load a previously saved session snapshot for a thread.
    /// Returns None if no saved session exists.
    pub async fn load(&self, thread_id: &str) -> Result<Option<SessionSnapshot>> {
        let path = self.session_path(thread_id);
        if !path.exists() {
            return Ok(None);
        }

        let content = fs::read_to_string(&path).await?;
        match serde_json::from_str::<SessionSnapshot>(&content) {
            Ok(snapshot) => {
                info!("Loaded session state for thread {}", thread_id);
                Ok(Some(snapshot))
            }
            Err(e) => {
                warn!("Failed to parse session file for {}: {}", thread_id, e);
                Ok(None)
            }
        }
    }

    /// Check if a session exists for a thread.
    pub fn has_session(&self, thread_id: &str) -> bool {
        self.session_path(thread_id).exists()
    }

    /// List all saved thread IDs.
    pub async fn list_threads(&self) -> Result<Vec<String>> {
        let mut threads = Vec::new();
        let mut entries = fs::read_dir(&self.sessions_dir).await?;
        while let Some(entry) = entries.next_entry().await? {
            if let Some(name) = entry.file_name().to_str() {
                if let Some(thread_id) = name.strip_suffix(".json") {
                    threads.push(thread_id.to_string());
                }
            }
        }
        Ok(threads)
    }

    /// Clean up old sessions (keep the most recent N).
    pub async fn cleanup(&self, keep: usize) -> Result<()> {
        let mut entries: Vec<(PathBuf, std::time::SystemTime)> = Vec::new();
        let mut dir = fs::read_dir(&self.sessions_dir).await?;
        while let Some(entry) = dir.next_entry().await? {
            if let Ok(meta) = entry.metadata().await {
                if let Ok(modified) = meta.modified() {
                    entries.push((entry.path(), modified));
                }
            }
        }

        // Sort oldest first
        entries.sort_by_key(|(_, t)| *t);

        // Remove oldest entries beyond the keep limit
        if entries.len() > keep {
            let to_remove = entries.len() - keep;
            for (path, _) in entries.iter().take(to_remove) {
                let _ = fs::remove_file(path).await;
            }
            info!("Cleaned up {} old session files", to_remove);
        }

        Ok(())
    }

    fn session_path(&self, thread_id: &str) -> PathBuf {
        // Sanitize thread_id for use as filename
        let safe_id: String = thread_id
            .chars()
            .map(|c| {
                if c.is_alphanumeric() || c == '-' || c == '_' {
                    c
                } else {
                    '_'
                }
            })
            .collect();
        self.sessions_dir.join(format!("{}.json", safe_id))
    }
}

/// Apply a loaded session snapshot — replaces ALL memory and notebook state.
pub async fn restore_snapshot(
    memory: &mut PersistentMemory,
    notebook: &mut GugugagaNotebook,
    snapshot: SessionSnapshot,
) -> Result<()> {
    // Restore ALL memory state
    memory.user_instructions = snapshot.memory.user_instructions;
    memory.current_task = snapshot.memory.current_task;
    memory.decisions = snapshot.memory.decisions;
    memory.behavior_log = snapshot.memory.behavior_log;
    memory.conversation_history = snapshot.memory.conversation_history;

    // Restore ALL notebook state
    notebook.current_activity = snapshot.notebook.current_activity;
    notebook.completed = snapshot.notebook.completed;
    notebook.attention = snapshot.notebook.attention;
    notebook.mistakes = snapshot.notebook.mistakes;

    // Persist the restored state
    memory.save().await?;
    notebook.save().await?;

    info!("Restored full session state for thread {}", snapshot.thread_id);
    Ok(())
}
