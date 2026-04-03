use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;

/// A memory entry with embedding vector
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryEntry {
    pub id: String,
    pub content: String,
    pub embedding: Option<Vec<f32>>,
    pub metadata: HashMap<String, String>,
    pub created_at: DateTime<Utc>,
    pub session_id: Option<String>,
}

/// Memory storage backend trait
pub trait MemoryStore: Send + Sync {
    /// Store a new memory entry
    fn store(
        &self,
        entry: MemoryEntry,
    ) -> Pin<Box<dyn Future<Output = anyhow::Result<()>> + Send + '_>>;

    /// Search memories by similarity
    fn search(
        &self,
        query_embedding: Vec<f32>,
        limit: usize,
    ) -> Pin<Box<dyn Future<Output = anyhow::Result<Vec<MemoryEntry>>> + Send + '_>>;

    /// Delete memories for a session
    fn delete_by_session(
        &self,
        session_id: &str,
    ) -> Pin<Box<dyn Future<Output = anyhow::Result<usize>> + Send + '_>>;

    /// List all memories (for debugging/admin)
    fn list(
        &self,
        limit: usize,
        offset: usize,
    ) -> Pin<Box<dyn Future<Output = anyhow::Result<Vec<MemoryEntry>>> + Send + '_>>;
}

/// Simple in-memory store for testing
pub struct InMemoryStore {
    memories: std::sync::RwLock<Vec<MemoryEntry>>,
}

impl InMemoryStore {
    pub fn new() -> Self {
        Self {
            memories: std::sync::RwLock::new(Vec::new()),
        }
    }
}

impl MemoryStore for InMemoryStore {
    fn store(
        &self,
        entry: MemoryEntry,
    ) -> Pin<Box<dyn Future<Output = anyhow::Result<()>> + Send + '_>> {
        Box::pin(async move {
            self.memories.write().unwrap().push(entry);
            Ok(())
        })
    }

    fn search(
        &self,
        _query_embedding: Vec<f32>,
        limit: usize,
    ) -> Pin<Box<dyn Future<Output = anyhow::Result<Vec<MemoryEntry>>> + Send + '_>> {
        Box::pin(async move {
            // TODO: Implement cosine similarity
            let memories = self.memories.read().unwrap();
            Ok(memories.iter().take(limit).cloned().collect())
        })
    }

    fn delete_by_session(
        &self,
        session_id: &str,
    ) -> Pin<Box<dyn Future<Output = anyhow::Result<usize>> + Send + '_>> {
        let session_id = session_id.to_string();
        Box::pin(async move {
            let mut memories = self.memories.write().unwrap();
            let before = memories.len();
            memories.retain(|m| m.session_id.as_ref() != Some(&session_id));
            Ok(before - memories.len())
        })
    }

    fn list(
        &self,
        limit: usize,
        offset: usize,
    ) -> Pin<Box<dyn Future<Output = anyhow::Result<Vec<MemoryEntry>>> + Send + '_>> {
        Box::pin(async move {
            let memories = self.memories.read().unwrap();
            Ok(memories.iter().skip(offset).take(limit).cloned().collect())
        })
    }
}
