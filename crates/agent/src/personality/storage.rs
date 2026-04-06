//! Personality storage trait and implementations

use super::PersonalityProfile;
use async_trait::async_trait;
use std::path::Path;

/// Trait for personality profile storage
#[async_trait]
pub trait PersonalityStorage: Send + Sync {
    /// Load a profile by user ID
    async fn load(&self, user_id: &str) -> anyhow::Result<Option<PersonalityProfile>>;
    
    /// Save a profile
    async fn save(&self, profile: &PersonalityProfile) -> anyhow::Result<()>;
    
    /// List all stored user IDs
    async fn list_users(&self) -> anyhow::Result<Vec<String>>;
    
    /// Delete a profile
    async fn delete(&self, _user_id: &str) -> anyhow::Result<bool> {
        // Default implementation - override if needed
        Ok(false)
    }
}

/// In-memory storage for testing
pub struct MemoryStorage {
    profiles: std::sync::Mutex<std::collections::HashMap<String, PersonalityProfile>>,
}

impl MemoryStorage {
    /// Create a new memory storage
    pub fn new() -> Self {
        Self {
            profiles: std::sync::Mutex::new(std::collections::HashMap::new()),
        }
    }
}

impl Default for MemoryStorage {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl PersonalityStorage for MemoryStorage {
    async fn load(&self, user_id: &str) -> anyhow::Result<Option<PersonalityProfile>> {
        let profiles = self.profiles.lock().unwrap();
        Ok(profiles.get(user_id).cloned())
    }

    async fn save(&self, profile: &PersonalityProfile) -> anyhow::Result<()> {
        let mut profiles = self.profiles.lock().unwrap();
        profiles.insert(profile.user_id.clone(), profile.clone());
        Ok(())
    }

    async fn list_users(&self) -> anyhow::Result<Vec<String>> {
        let profiles = self.profiles.lock().unwrap();
        Ok(profiles.keys().cloned().collect())
    }

    async fn delete(&self, user_id: &str) -> anyhow::Result<bool> {
        let mut profiles = self.profiles.lock().unwrap();
        Ok(profiles.remove(user_id).is_some())
    }
}

/// File-based storage implementation
pub struct FileStorage {
    base_path: std::path::PathBuf,
}

impl FileStorage {
    /// Create a new file storage
    pub fn new(base_path: impl AsRef<Path>) -> Self {
        Self {
            base_path: base_path.as_ref().to_path_buf(),
        }
    }
}

#[async_trait]
impl PersonalityStorage for FileStorage {
    async fn load(&self, user_id: &str) -> anyhow::Result<Option<PersonalityProfile>> {
        let path = self.base_path.join(format!("{}.json", user_id));
        if !path.exists() {
            return Ok(None);
        }
        
        let content = tokio::fs::read_to_string(&path).await?;
        let profile = serde_json::from_str(&content)?;
        Ok(Some(profile))
    }

    async fn save(&self, profile: &PersonalityProfile) -> anyhow::Result<()> {
        tokio::fs::create_dir_all(&self.base_path).await?;
        let path = self.base_path.join(format!("{}.json", profile.user_id));
        let content = serde_json::to_string_pretty(profile)?;
        tokio::fs::write(&path, content).await?;
        Ok(())
    }

    async fn list_users(&self) -> anyhow::Result<Vec<String>> {
        let mut users = Vec::new();
        if self.base_path.exists() {
            let mut entries = tokio::fs::read_dir(&self.base_path).await?;
            while let Some(entry) = entries.next_entry().await? {
                if let Some(name) = entry.file_name().to_str() {
                    if name.ends_with(".json") {
                        users.push(name.trim_end_matches(".json").to_string());
                    }
                }
            }
        }
        Ok(users)
    }

    async fn delete(&self, user_id: &str) -> anyhow::Result<bool> {
        let path = self.base_path.join(format!("{}.json", user_id));
        if path.exists() {
            tokio::fs::remove_file(&path).await?;
            Ok(true)
        } else {
            Ok(false)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::personality::PersonalityProfile;

    #[tokio::test]
    async fn test_memory_storage() {
        let storage = MemoryStorage::new();
        
        // Initially empty
        assert!(storage.load("user1").await.unwrap().is_none());
        assert!(storage.list_users().await.unwrap().is_empty());
        
        // Save profile
        let profile = PersonalityProfile::new("user1");
        storage.save(&profile).await.unwrap();
        
        // Load and verify
        let loaded = storage.load("user1").await.unwrap();
        assert!(loaded.is_some());
        assert_eq!(loaded.unwrap().user_id, "user1");
        
        // List users
        let users = storage.list_users().await.unwrap();
        assert_eq!(users.len(), 1);
        assert_eq!(users[0], "user1");
        
        // Delete
        assert!(storage.delete("user1").await.unwrap());
        assert!(!storage.delete("user1").await.unwrap());
        assert!(storage.load("user1").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn test_file_storage() {
        let temp_dir = std::env::temp_dir().join("test_personality_storage");
        let _ = tokio::fs::remove_dir_all(&temp_dir).await;
        
        let storage = FileStorage::new(&temp_dir);
        
        // Save
        let profile = PersonalityProfile::new("test_user");
        storage.save(&profile).await.unwrap();
        
        // Load
        let loaded = storage.load("test_user").await.unwrap();
        assert!(loaded.is_some());
        assert_eq!(loaded.unwrap().user_id, "test_user");
        
        // List
        let users = storage.list_users().await.unwrap();
        assert_eq!(users.len(), 1);
        
        // Delete
        assert!(storage.delete("test_user").await.unwrap());
        assert!(storage.load("test_user").await.unwrap().is_none());
        
        // Cleanup
        let _ = tokio::fs::remove_dir_all(&temp_dir).await;
    }
}
