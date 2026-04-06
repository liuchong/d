//! Key-value storage implementations

use async_trait::async_trait;
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{debug, error, info, warn};

/// Key-value store trait
#[async_trait]
pub trait KvStore: Send + Sync {
    /// Get value
    async fn get(&self, key: &str) -> anyhow::Result<Option<Vec<u8>>>;
    
    /// Set value
    async fn set(&self, key: &str, value: Vec<u8>) -> anyhow::Result<()>;
    
    /// Delete key
    async fn delete(&self, key: &str) -> anyhow::Result<()>;
    
    /// Check if key exists
    async fn exists(&self, key: &str) -> anyhow::Result<bool>;
    
    /// List keys with prefix
    async fn list_keys(&self, prefix: &str) -> anyhow::Result<Vec<String>>;
    
    /// Count keys
    async fn count(&self) -> anyhow::Result<usize>;
    
    /// Clear all data
    async fn clear(&self) -> anyhow::Result<()>;
}

/// In-memory key-value store
pub struct MemoryKvStore {
    data: RwLock<HashMap<String, Vec<u8>>>,
}

impl MemoryKvStore {
    /// Create new memory store
    pub fn new() -> Self {
        Self {
            data: RwLock::new(HashMap::new()),
        }
    }

    /// Create with initial data
    pub fn with_data(data: HashMap<String, Vec<u8>>) -> Self {
        Self {
            data: RwLock::new(data),
        }
    }
}

impl Default for MemoryKvStore {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl KvStore for MemoryKvStore {
    async fn get(&self, key: &str) -> anyhow::Result<Option<Vec<u8>>> {
        let data = self.data.read().await;
        Ok(data.get(key).cloned())
    }

    async fn set(&self, key: &str, value: Vec<u8>) -> anyhow::Result<()> {
        let mut data = self.data.write().await;
        data.insert(key.to_string(), value);
        Ok(())
    }

    async fn delete(&self, key: &str) -> anyhow::Result<()> {
        let mut data = self.data.write().await;
        data.remove(key);
        Ok(())
    }

    async fn exists(&self, key: &str) -> anyhow::Result<bool> {
        let data = self.data.read().await;
        Ok(data.contains_key(key))
    }

    async fn list_keys(&self, prefix: &str) -> anyhow::Result<Vec<String>> {
        let data = self.data.read().await;
        Ok(data
            .keys()
            .filter(|k| k.starts_with(prefix))
            .cloned()
            .collect())
    }

    async fn count(&self) -> anyhow::Result<usize> {
        let data = self.data.read().await;
        Ok(data.len())
    }

    async fn clear(&self) -> anyhow::Result<()> {
        let mut data = self.data.write().await;
        data.clear();
        Ok(())
    }
}

#[async_trait]
impl super::Storage for MemoryKvStore {
    fn name(&self) -> &str {
        "memory_kv"
    }

    async fn health_check(&self) -> anyhow::Result<()> {
        Ok(())
    }

    async fn close(&self) -> anyhow::Result<()> {
        Ok(())
    }
}

/// File-based key-value store
pub struct FileKvStore {
    base_path: std::path::PathBuf,
}

impl FileKvStore {
    /// Create new file store
    pub fn new(base_path: impl AsRef<Path>) -> anyhow::Result<Self> {
        let base_path = base_path.as_ref().to_path_buf();
        std::fs::create_dir_all(&base_path)?;
        
        Ok(Self { base_path })
    }

    /// Get file path for key
    fn key_to_path(&self, key: &str) -> std::path::PathBuf {
        // Simple key sanitization
        let safe_key = key
            .replace('/', "_")
            .replace('\\', "_")
            .replace(':', "_");
        self.base_path.join(format!("{}.dat", safe_key))
    }
}

#[async_trait]
impl KvStore for FileKvStore {
    async fn get(&self, key: &str) -> anyhow::Result<Option<Vec<u8>>> {
        let path = self.key_to_path(key);
        
        match tokio::fs::read(&path).await {
            Ok(data) => Ok(Some(data)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    async fn set(&self, key: &str, value: Vec<u8>) -> anyhow::Result<()> {
        let path = self.key_to_path(key);
        let temp_path = path.with_extension("tmp");
        
        tokio::fs::write(&temp_path, value).await?;
        tokio::fs::rename(&temp_path, &path).await?;
        
        Ok(())
    }

    async fn delete(&self, key: &str) -> anyhow::Result<()> {
        let path = self.key_to_path(key);
        
        match tokio::fs::remove_file(&path).await {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e.into()),
        }
    }

    async fn exists(&self, key: &str) -> anyhow::Result<bool> {
        let path = self.key_to_path(key);
        Ok(path.exists())
    }

    async fn list_keys(&self, prefix: &str) -> anyhow::Result<Vec<String>> {
        let mut keys = Vec::new();
        
        let mut entries = tokio::fs::read_dir(&self.base_path).await?;
        while let Some(entry) = entries.next_entry().await? {
            let name = entry.file_name().to_string_lossy().to_string();
            
            if name.ends_with(".dat") {
                let key = name.trim_end_matches(".dat").to_string();
                if key.starts_with(prefix) {
                    keys.push(key);
                }
            }
        }
        
        Ok(keys)
    }

    async fn count(&self) -> anyhow::Result<usize> {
        let mut count = 0;
        
        let mut entries = tokio::fs::read_dir(&self.base_path).await?;
        while let Some(entry) = entries.next_entry().await? {
            let name = entry.file_name().to_string_lossy().to_string();
            if name.ends_with(".dat") {
                count += 1;
            }
        }
        
        Ok(count)
    }

    async fn clear(&self) -> anyhow::Result<()> {
        let mut entries = tokio::fs::read_dir(&self.base_path).await?;
        while let Some(entry) = entries.next_entry().await? {
            let path = entry.path();
            if path.extension().map(|e| e == "dat").unwrap_or(false) {
                tokio::fs::remove_file(&path).await?;
            }
        }
        
        Ok(())
    }
}

/// TTL entry
#[derive(Clone)]
struct TtlEntry {
    value: Vec<u8>,
    expires_at: std::time::Instant,
}

/// TTL-aware key-value store wrapper
pub struct TtlKvStore {
    inner: Arc<dyn KvStore>,
    ttl_data: RwLock<HashMap<String, TtlEntry>>,
    cleanup_interval: std::time::Duration,
}

impl TtlKvStore {
    /// Create TTL store wrapping another store
    pub fn new(inner: Arc<dyn KvStore>) -> Self {
        Self {
            inner,
            ttl_data: RwLock::new(HashMap::new()),
            cleanup_interval: std::time::Duration::from_secs(60),
        }
    }

    /// Set with TTL
    pub async fn set_with_ttl(
        &self,
        key: &str,
        value: Vec<u8>,
        ttl: std::time::Duration,
    ) -> anyhow::Result<()> {
        let entry = TtlEntry {
            value: value.clone(),
            expires_at: std::time::Instant::now() + ttl,
        };
        
        let mut data = self.ttl_data.write().await;
        data.insert(key.to_string(), entry);
        drop(data);
        
        self.inner.set(key, value).await
    }

    /// Cleanup expired entries
    pub async fn cleanup(&self) -> anyhow::Result<()> {
        let now = std::time::Instant::now();
        let mut data = self.ttl_data.write().await;
        
        let expired: Vec<String> = data
            .iter()
            .filter(|(_, entry)| entry.expires_at <= now)
            .map(|(k, _)| k.clone())
            .collect();
        
        for key in expired {
            data.remove(&key);
            let _ = self.inner.delete(&key).await;
        }
        
        Ok(())
    }
}

#[async_trait]
impl KvStore for TtlKvStore {
    async fn get(&self, key: &str) -> anyhow::Result<Option<Vec<u8>>> {
        let data = self.ttl_data.read().await;
        
        if let Some(entry) = data.get(key) {
            if entry.expires_at > std::time::Instant::now() {
                return Ok(Some(entry.value.clone()));
            }
        }
        drop(data);
        
        self.inner.get(key).await
    }

    async fn set(&self, key: &str, value: Vec<u8>) -> anyhow::Result<()> {
        self.inner.set(key, value).await
    }

    async fn delete(&self, key: &str) -> anyhow::Result<()> {
        let mut data = self.ttl_data.write().await;
        data.remove(key);
        drop(data);
        
        self.inner.delete(key).await
    }

    async fn exists(&self, key: &str) -> anyhow::Result<bool> {
        let data = self.ttl_data.read().await;
        
        if let Some(entry) = data.get(key) {
            if entry.expires_at > std::time::Instant::now() {
                return Ok(true);
            }
        }
        drop(data);
        
        self.inner.exists(key).await
    }

    async fn list_keys(&self, prefix: &str) -> anyhow::Result<Vec<String>> {
        self.inner.list_keys(prefix).await
    }

    async fn count(&self) -> anyhow::Result<usize> {
        self.inner.count().await
    }

    async fn clear(&self) -> anyhow::Result<()> {
        let mut data = self.ttl_data.write().await;
        data.clear();
        drop(data);
        
        self.inner.clear().await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_memory_kv_store() {
        let store = MemoryKvStore::new();
        
        // Test set and get
        store.set("key1", b"value1".to_vec()).await.unwrap();
        let value = store.get("key1").await.unwrap();
        assert_eq!(value, Some(b"value1".to_vec()));
        
        // Test exists
        assert!(store.exists("key1").await.unwrap());
        assert!(!store.exists("key2").await.unwrap());
        
        // Test delete
        store.delete("key1").await.unwrap();
        assert!(!store.exists("key1").await.unwrap());
    }

    #[tokio::test]
    async fn test_file_kv_store() {
        let temp_dir = tempfile::tempdir().unwrap();
        let store = FileKvStore::new(temp_dir.path()).unwrap();
        
        store.set("test", b"data".to_vec()).await.unwrap();
        let value = store.get("test").await.unwrap();
        assert_eq!(value, Some(b"data".to_vec()));
        
        let keys = store.list_keys("").await.unwrap();
        assert!(keys.contains(&"test".to_string()));
    }

    #[tokio::test]
    async fn test_kv_prefix() {
        let store = MemoryKvStore::new();
        
        store.set("prefix:one", b"1".to_vec()).await.unwrap();
        store.set("prefix:two", b"2".to_vec()).await.unwrap();
        store.set("other", b"3".to_vec()).await.unwrap();
        
        let keys = store.list_keys("prefix:").await.unwrap();
        assert_eq!(keys.len(), 2);
    }
}
