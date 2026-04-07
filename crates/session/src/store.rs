//! Session store with persistence, indexing and search
//!
//! Provides:
//! - Automatic session persistence
//! - Session indexing for fast listing
//! - Full-text search
//! - Import/export

use crate::{Session, SessionInfo, SessionSearch};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use tokio::fs;
use tokio::io::AsyncWriteExt;
use tracing::{debug, info, warn};

/// Session store with persistence
pub struct SessionStore {
    storage_dir: PathBuf,
    sessions: HashMap<String, Session>,
    index: SessionIndex,
    auto_save: bool,
}

/// Session index for fast queries
#[derive(Debug, Default)]
struct SessionIndex {
    by_time: Vec<(String, chrono::DateTime<chrono::Utc>)>,
    by_name: HashMap<String, Vec<String>>,
    by_git_branch: HashMap<String, Vec<String>>,
    by_working_dir: HashMap<PathBuf, Vec<String>>,
}

impl SessionIndex {
    /// Build index from sessions
    fn rebuild(&mut self, sessions: &HashMap<String, Session>) {
        self.by_time.clear();
        self.by_name.clear();
        self.by_git_branch.clear();
        self.by_working_dir.clear();

        for (id, session) in sessions {
            self.by_time.push((id.clone(), session.last_accessed));
            
            self.by_name
                .entry(session.name.clone())
                .or_default()
                .push(id.clone());
            
            if let Some(ref branch) = session.git_branch {
                self.by_git_branch
                    .entry(branch.clone())
                    .or_default()
                    .push(id.clone());
            }
            
            self.by_working_dir
                .entry(session.working_dir.clone())
                .or_default()
                .push(id.clone());
        }

        // Sort by time descending
        self.by_time.sort_by(|a, b| b.1.cmp(&a.1));
    }

    /// Get recent sessions
    fn recent(&self, limit: usize) -> Vec<String> {
        self.by_time.iter()
            .take(limit)
            .map(|(id, _)| id.clone())
            .collect()
    }

    /// Get sessions by name
    fn by_name(&self, name: &str) -> Vec<String> {
        self.by_name.get(name).cloned().unwrap_or_default()
    }

    /// Get sessions by branch
    fn by_branch(&self, branch: &str) -> Vec<String> {
        self.by_git_branch.get(branch).cloned().unwrap_or_default()
    }
}

impl SessionStore {
    /// Create new session store
    pub async fn new() -> anyhow::Result<Self> {
        let storage_dir = dirs::data_dir()
            .ok_or_else(|| anyhow::anyhow!("Cannot find data directory"))?
            .join("d")
            .join("sessions");

        fs::create_dir_all(&storage_dir).await?;

        let mut store = Self {
            storage_dir,
            sessions: HashMap::new(),
            index: SessionIndex::default(),
            auto_save: true,
        };

        // Load existing sessions
        store.load_all().await?;

        Ok(store)
    }

    /// Create with custom storage directory
    pub async fn with_path(path: impl AsRef<Path>) -> anyhow::Result<Self> {
        let storage_dir = path.as_ref().to_path_buf();
        fs::create_dir_all(&storage_dir).await?;

        let mut store = Self {
            storage_dir,
            sessions: HashMap::new(),
            index: SessionIndex::default(),
            auto_save: true,
        };

        store.load_all().await?;

        Ok(store)
    }

    /// Set auto-save
    pub fn set_auto_save(&mut self, auto_save: bool) {
        self.auto_save = auto_save;
    }

    /// Create new session
    pub async fn create(&mut self, name: Option<String>) -> anyhow::Result<Session> {
        let session = Session::new(name);
        let id = session.id.clone();
        
        self.sessions.insert(id.clone(), session.clone());
        self.index.rebuild(&self.sessions);

        if self.auto_save {
            self.save(&id).await?;
        }

        info!("Created session: {} ({}", session.name, &id[..8]);
        Ok(session)
    }

    /// Get session by ID
    pub fn get(&self, id: &str) -> Option<Session> {
        self.sessions.get(id).cloned()
    }

    /// Get session info by ID
    pub fn get_info(&self, id: &str) -> Option<SessionInfo> {
        self.sessions.get(id).map(|s| s.info())
    }

    /// Get or create session
    pub async fn get_or_create(&mut self, id: &str) -> anyhow::Result<Session> {
        if let Some(session) = self.get(id) {
            Ok(session)
        } else {
            let session = Session::with_id(id);
            self.sessions.insert(id.to_string(), session.clone());
            self.index.rebuild(&self.sessions);
            
            if self.auto_save {
                self.save(id).await?;
            }
            
            Ok(session)
        }
    }

    /// Update session
    pub async fn update<F>(&mut self, id: &str, f: F) -> anyhow::Result<()>
    where
        F: FnOnce(&mut Session),
    {
        if let Some(session) = self.sessions.get_mut(id) {
            f(session);
            self.index.rebuild(&self.sessions);
            
            if self.auto_save {
                self.save(id).await?;
            }
            
            Ok(())
        } else {
            anyhow::bail!("Session not found: {}", id)
        }
    }

    /// Add message to session
    pub async fn add_message(
        &mut self,
        id: &str,
        message: crate::SessionMessage,
    ) -> anyhow::Result<()> {
        self.update(id, |session| {
            session.add_message(message);
        }).await
    }

    /// List all session infos
    pub fn list(&self) -> Vec<SessionInfo> {
        self.sessions.values()
            .map(|s| s.info())
            .collect()
    }

    /// List recent sessions
    pub fn list_recent(&self, limit: usize) -> Vec<SessionInfo> {
        self.index.recent(limit)
            .iter()
            .filter_map(|id| self.get_info(id))
            .collect()
    }

    /// Search sessions
    pub fn search(&self, search: SessionSearch) -> Vec<SessionInfo> {
        let mut results: Vec<_> = self.sessions.values()
            .filter(|session| {
                // Query filter
                if let Some(ref query) = search.query {
                    let q = query.to_lowercase();
                    if !session.name.to_lowercase().contains(&q)
                        && !session.id.to_lowercase().contains(&q)
                        && session.summary.as_ref()
                            .map(|s| !s.to_lowercase().contains(&q))
                            .unwrap_or(true)
                    {
                        return false;
                    }
                }

                // Working directory filter
                if let Some(ref dir) = search.working_dir {
                    if !session.working_dir.starts_with(dir) {
                        return false;
                    }
                }

                // Git branch filter
                if let Some(ref branch) = search.git_branch {
                    if session.git_branch.as_ref() != Some(branch) {
                        return false;
                    }
                }

                // Time filters
                if let Some(after) = search.after {
                    if session.last_accessed < after {
                        return false;
                    }
                }

                if let Some(before) = search.before {
                    if session.last_accessed > before {
                        return false;
                    }
                }

                true
            })
            .map(|s| s.info())
            .collect();

        // Sort by last accessed descending
        results.sort_by(|a, b| b.last_accessed.cmp(&a.last_accessed));

        // Apply limit
        if let Some(limit) = search.limit {
            results.truncate(limit);
        }

        results
    }

    /// Delete session
    pub async fn delete(&mut self, id: &str) -> anyhow::Result<bool> {
        if self.sessions.remove(id).is_some() {
            // Delete file
            let file_path = self.storage_dir.join(format!("{}.json", id));
            if file_path.exists() {
                fs::remove_file(&file_path).await?;
            }
            
            self.index.rebuild(&self.sessions);
            info!("Deleted session: {}", &id[..8]);
            Ok(true)
        } else {
            Ok(false)
        }
    }

    /// Rename session
    pub async fn rename(&mut self, id: &str, name: impl Into<String>) -> anyhow::Result<()> {
        let name = name.into();
        self.update(id, |session| {
            session.rename(&name);
        }).await
    }

    /// Save session to disk (by session object)
    pub async fn save_session(&self, session: &Session) -> anyhow::Result<()> {
        let id = &session.id;
        let file_path = self.storage_dir.join(format!("{}.json", id));
        let temp_path = file_path.with_extension("tmp");

        // Serialize
        let json = serde_json::to_string_pretty(session)?;

        // Write to temp file first
        let mut file = fs::File::create(&temp_path).await?;
        file.write_all(json.as_bytes()).await?;
        file.flush().await?;
        drop(file);

        // Atomic rename
        fs::rename(&temp_path, &file_path).await?;

        debug!("Saved session: {}", &id[..8]);
        Ok(())
    }

    /// Save session to disk (by id)
    pub async fn save(&self, id: &str) -> anyhow::Result<()> {
        let session = self.sessions.get(id)
            .ok_or_else(|| anyhow::anyhow!("Session not found: {}", id))?;

        let file_path = self.storage_dir.join(format!("{}.json", id));
        let temp_path = file_path.with_extension("tmp");

        // Serialize
        let json = serde_json::to_string_pretty(session)?;

        // Write to temp file first
        let mut file = fs::File::create(&temp_path).await?;
        file.write_all(json.as_bytes()).await?;
        file.flush().await?;
        drop(file);

        // Atomic rename
        fs::rename(&temp_path, &file_path).await?;

        debug!("Saved session: {}", &id[..8]);
        Ok(())
    }

    /// Save all sessions
    pub async fn save_all(&self) -> anyhow::Result<()> {
        for id in self.sessions.keys() {
            self.save(id).await?;
        }
        Ok(())
    }

    /// Load session from disk
    pub async fn load(&mut self, id: &str) -> anyhow::Result<Option<Session>> {
        let file_path = self.storage_dir.join(format!("{}.json", id));
        
        if !file_path.exists() {
            return Ok(None);
        }

        let json = fs::read_to_string(&file_path).await?;
        let session: Session = serde_json::from_str(&json)?;
        
        self.sessions.insert(id.to_string(), session.clone());
        self.index.rebuild(&self.sessions);

        info!("Loaded session: {} ({})", session.name, &id[..8]);
        Ok(Some(session))
    }

    /// Load all sessions from disk
    pub async fn load_all(&mut self) -> anyhow::Result<()> {
        let mut entries = fs::read_dir(&self.storage_dir).await?;
        let mut count = 0;

        while let Some(entry) = entries.next_entry().await? {
            let path = entry.path();
            if path.extension().map(|e| e == "json").unwrap_or(false) {
                if let Some(id) = path.file_stem().and_then(|s| s.to_str()) {
                    if let Err(e) = self.load(id).await {
                        warn!("Failed to load session {}: {}", id, e);
                    } else {
                        count += 1;
                    }
                }
            }
        }

        info!("Loaded {} sessions from {}", count, self.storage_dir.display());
        Ok(())
    }

    /// Import session from file
    pub async fn import(&mut self, path: impl AsRef<Path>) -> anyhow::Result<Session> {
        let json = fs::read_to_string(path).await?;
        let session: Session = serde_json::from_str(&json)?;
        
        let id = session.id.clone();
        self.sessions.insert(id.clone(), session.clone());
        self.index.rebuild(&self.sessions);
        
        if self.auto_save {
            self.save(&id).await?;
        }

        info!("Imported session: {}", &id[..8]);
        Ok(session)
    }

    /// Export session to file
    pub async fn export(
        &self,
        id: &str,
        path: impl AsRef<Path>,
    ) -> anyhow::Result<()> {
        let session = self.sessions.get(id)
            .ok_or_else(|| anyhow::anyhow!("Session not found: {}", id))?;

        let json = serde_json::to_string_pretty(session)?;
        fs::write(path, json).await?;

        info!("Exported session: {}", &id[..8]);
        Ok(())
    }

    /// Get storage statistics
    pub fn stats(&self) -> StoreStats {
        StoreStats {
            total_sessions: self.sessions.len(),
            total_messages: self.sessions.values().map(|s| s.message_count()).sum(),
            storage_dir: self.storage_dir.clone(),
        }
    }

    /// Get current session count
    pub fn len(&self) -> usize {
        self.sessions.len()
    }

    /// Check if empty
    pub fn is_empty(&self) -> bool {
        self.sessions.is_empty()
    }
}

/// Store statistics
#[derive(Debug, Clone)]
pub struct StoreStats {
    pub total_sessions: usize,
    pub total_messages: usize,
    pub storage_dir: PathBuf,
}

impl std::fmt::Display for StoreStats {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Sessions: {}, Messages: {}, Storage: {}",
            self.total_sessions,
            self.total_messages,
            self.storage_dir.display()
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[tokio::test]
    async fn test_store_creation() {
        let temp_dir = TempDir::new().unwrap();
        let store = SessionStore::with_path(temp_dir.path()).await;
        assert!(store.is_ok());
        assert_eq!(store.unwrap().len(), 0);
    }

    #[tokio::test]
    async fn test_create_and_get() {
        let temp_dir = TempDir::new().unwrap();
        let mut store = SessionStore::with_path(temp_dir.path()).await.unwrap();

        let session = store.create(Some("Test Session".to_string())).await.unwrap();
        let retrieved = store.get(&session.id).unwrap();
        
        assert_eq!(retrieved.name, "Test Session");
    }

    #[tokio::test]
    async fn test_list_recent() {
        let temp_dir = TempDir::new().unwrap();
        let mut store = SessionStore::with_path(temp_dir.path()).await.unwrap();

        store.create(Some("Session 1".to_string())).await.unwrap();
        store.create(Some("Session 2".to_string())).await.unwrap();
        store.create(Some("Session 3".to_string())).await.unwrap();

        let recent = store.list_recent(2);
        assert_eq!(recent.len(), 2);
    }

    #[tokio::test]
    async fn test_search() {
        let temp_dir = TempDir::new().unwrap();
        let mut store = SessionStore::with_path(temp_dir.path()).await.unwrap();

        store.create(Some("Alpha Session".to_string())).await.unwrap();
        store.create(Some("Beta Session".to_string())).await.unwrap();

        let results = store.search(SessionSearch::new().query("Alpha"));
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].name, "Alpha Session");
    }

    #[tokio::test]
    async fn test_save_and_load() {
        let temp_dir = TempDir::new().unwrap();
        let mut store = SessionStore::with_path(temp_dir.path()).await.unwrap();

        let session = store.create(Some("Persisted".to_string())).await.unwrap();
        let id = session.id.clone();

        // Save
        store.save(&id).await.unwrap();

        // Create new store instance and load
        let mut store2 = SessionStore::with_path(temp_dir.path()).await.unwrap();
        assert_eq!(store2.len(), 1);

        let loaded = store2.get(&id).unwrap();
        assert_eq!(loaded.name, "Persisted");
    }

    #[tokio::test]
    async fn test_delete() {
        let temp_dir = TempDir::new().unwrap();
        let mut store = SessionStore::with_path(temp_dir.path()).await.unwrap();

        let session = store.create(None).await.unwrap();
        let id = session.id.clone();

        assert!(store.delete(&id).await.unwrap());
        assert!(store.get(&id).is_none());
    }
}
