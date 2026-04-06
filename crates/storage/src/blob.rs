//! Blob storage for binary data

use async_trait::async_trait;
use std::path::{Path, PathBuf};
use tracing::{debug, error, info, warn};

/// Blob metadata
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct BlobInfo {
    /// Blob ID
    pub id: String,
    /// Content type
    pub content_type: String,
    /// Size in bytes
    pub size: u64,
    /// Creation timestamp
    pub created_at: chrono::DateTime<chrono::Utc>,
    /// Checksum
    pub checksum: Option<String>,
}

/// Blob store trait
#[async_trait]
pub trait BlobStore: Send + Sync {
    /// Store blob
    async fn put(&self, id: &str, data: Vec<u8>, content_type: &str) -> anyhow::Result<BlobInfo>;
    
    /// Retrieve blob
    async fn get(&self, id: &str) -> anyhow::Result<Option<Vec<u8>>>;
    
    /// Get blob info
    async fn info(&self, id: &str) -> anyhow::Result<Option<BlobInfo>>;
    
    /// Delete blob
    async fn delete(&self, id: &str) -> anyhow::Result<()>;
    
    /// Check if blob exists
    async fn exists(&self, id: &str) -> anyhow::Result<bool>;
    
    /// List blobs
    async fn list(&self, prefix: &str) -> anyhow::Result<Vec<BlobInfo>>;
}

/// In-memory blob store
pub struct MemoryBlobStore {
    blobs: tokio::sync::RwLock<std::collections::HashMap<String, (Vec<u8>, BlobInfo)>>,
}

impl MemoryBlobStore {
    /// Create new store
    pub fn new() -> Self {
        Self {
            blobs: tokio::sync::RwLock::new(std::collections::HashMap::new()),
        }
    }

    /// Calculate checksum
    fn calculate_checksum(data: &[u8]) -> String {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};
        
        let mut hasher = DefaultHasher::new();
        data.hash(&mut hasher);
        format!("{:x}", hasher.finish())
    }
}

impl Default for MemoryBlobStore {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl BlobStore for MemoryBlobStore {
    async fn put(&self, id: &str, data: Vec<u8>, content_type: &str) -> anyhow::Result<BlobInfo> {
        let checksum = Some(Self::calculate_checksum(&data));
        
        let info = BlobInfo {
            id: id.to_string(),
            content_type: content_type.to_string(),
            size: data.len() as u64,
            created_at: chrono::Utc::now(),
            checksum,
        };
        
        let mut blobs = self.blobs.write().await;
        blobs.insert(id.to_string(), (data, info.clone()));
        
        info!("Stored blob: {} ({} bytes)", id, info.size);
        Ok(info)
    }

    async fn get(&self, id: &str) -> anyhow::Result<Option<Vec<u8>>> {
        let blobs = self.blobs.read().await;
        Ok(blobs.get(id).map(|(data, _)| data.clone()))
    }

    async fn info(&self, id: &str) -> anyhow::Result<Option<BlobInfo>> {
        let blobs = self.blobs.read().await;
        Ok(blobs.get(id).map(|(_, info)| info.clone()))
    }

    async fn delete(&self, id: &str) -> anyhow::Result<()> {
        let mut blobs = self.blobs.write().await;
        blobs.remove(id);
        info!("Deleted blob: {}", id);
        Ok(())
    }

    async fn exists(&self, id: &str) -> anyhow::Result<bool> {
        let blobs = self.blobs.read().await;
        Ok(blobs.contains_key(id))
    }

    async fn list(&self, prefix: &str) -> anyhow::Result<Vec<BlobInfo>> {
        let blobs = self.blobs.read().await;
        Ok(blobs
            .values()
            .filter(|(_, info)| info.id.starts_with(prefix))
            .map(|(_, info)| info.clone())
            .collect())
    }
}

/// File-based blob store
pub struct FileBlobStore {
    base_path: PathBuf,
}

impl FileBlobStore {
    /// Create new file blob store
    pub fn new(base_path: impl AsRef<Path>) -> anyhow::Result<Self> {
        let base_path = base_path.as_ref().to_path_buf();
        std::fs::create_dir_all(&base_path)?;
        Ok(Self { base_path })
    }

    /// Get path for blob
    fn blob_path(&self, id: &str) -> PathBuf {
        let safe_id = id.replace('/', "_").replace('\\', "_");
        self.base_path.join(&safe_id)
    }

    /// Get metadata path
    fn meta_path(&self, id: &str) -> PathBuf {
        self.blob_path(id).with_extension("meta")
    }

    /// Calculate checksum
    async fn calculate_checksum(data: &[u8]) -> String {
        use sha2::{Sha256, Digest};
        let mut hasher = Sha256::new();
        hasher.update(data);
        format!("{:x}", hasher.finalize())
    }
}

#[async_trait]
impl BlobStore for FileBlobStore {
    async fn put(&self, id: &str, data: Vec<u8>, content_type: &str) -> anyhow::Result<BlobInfo> {
        let path = self.blob_path(id);
        let temp_path = path.with_extension("tmp");
        
        // Write data
        tokio::fs::write(&temp_path, &data).await?;
        tokio::fs::rename(&temp_path, &path).await?;
        
        // Create info
        let info = BlobInfo {
            id: id.to_string(),
            content_type: content_type.to_string(),
            size: data.len() as u64,
            created_at: chrono::Utc::now(),
            checksum: Some(Self::calculate_checksum(&data).await),
        };
        
        // Write metadata
        let meta_path = self.meta_path(id);
        let meta_json = serde_json::to_string(&info)?;
        tokio::fs::write(&meta_path, meta_json).await?;
        
        Ok(info)
    }

    async fn get(&self, id: &str) -> anyhow::Result<Option<Vec<u8>>> {
        let path = self.blob_path(id);
        
        match tokio::fs::read(&path).await {
            Ok(data) => Ok(Some(data)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    async fn info(&self, id: &str) -> anyhow::Result<Option<BlobInfo>> {
        let meta_path = self.meta_path(id);
        
        match tokio::fs::read_to_string(&meta_path).await {
            Ok(json) => {
                let info: BlobInfo = serde_json::from_str(&json)?;
                Ok(Some(info))
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    async fn delete(&self, id: &str) -> anyhow::Result<()> {
        let path = self.blob_path(id);
        let meta_path = self.meta_path(id);
        
        let _ = tokio::fs::remove_file(&path).await;
        let _ = tokio::fs::remove_file(&meta_path).await;
        
        Ok(())
    }

    async fn exists(&self, id: &str) -> anyhow::Result<bool> {
        let path = self.blob_path(id);
        Ok(path.exists())
    }

    async fn list(&self, prefix: &str) -> anyhow::Result<Vec<BlobInfo>> {
        let mut results = Vec::new();
        
        let mut entries = tokio::fs::read_dir(&self.base_path).await?;
        while let Some(entry) = entries.next_entry().await? {
            let path = entry.path();
            
            if path.extension().map(|e| e == "meta").unwrap_or(false) {
                continue;
            }
            
            let name = entry.file_name().to_string_lossy().to_string();
            if name.starts_with(prefix) {
                if let Some(info) = self.info(&name).await? {
                    results.push(info);
                }
            }
        }
        
        Ok(results)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_memory_blob_store() {
        let store = MemoryBlobStore::new();
        
        // Put
        let info = store.put("blob1", b"hello world".to_vec(), "text/plain").await.unwrap();
        assert_eq!(info.size, 11);
        assert_eq!(info.content_type, "text/plain");
        
        // Get
        let data = store.get("blob1").await.unwrap().unwrap();
        assert_eq!(data, b"hello world");
        
        // Exists
        assert!(store.exists("blob1").await.unwrap());
        assert!(!store.exists("blob2").await.unwrap());
        
        // Delete
        store.delete("blob1").await.unwrap();
        assert!(!store.exists("blob1").await.unwrap());
    }

    #[tokio::test]
    async fn test_file_blob_store() {
        let temp_dir = tempfile::tempdir().unwrap();
        let store = FileBlobStore::new(temp_dir.path()).unwrap();
        
        let info = store.put("test", b"data".to_vec(), "application/octet-stream").await.unwrap();
        assert_eq!(info.size, 4);
        
        let data = store.get("test").await.unwrap().unwrap();
        assert_eq!(data, b"data");
    }
}
