//! Persistence layer for data storage and retrieval
//!
//! Provides:
//! - Pluggable storage backends
//! - Transaction support
//! - Data versioning
//! - Automatic migration

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;
use tracing::{debug, error, info, warn};

/// Persistable data trait
pub trait Persistable: Serialize + for<'de> Deserialize<'de> + Send + Sync {
    /// Unique identifier
    fn id(&self) -> &str;
    
    /// Data version for migration
    fn version(&self) -> u32;
}

/// Storage backend trait
#[async_trait]
pub trait StorageBackend: Send + Sync {
    /// Backend name
    fn name(&self) -> &str;
    
    /// Check if backend is available
    async fn is_available(&self) -> bool;
    
    /// Store data
    async fn store(&self, key: &str, data: &[u8]) -> anyhow::Result<()>;
    
    /// Retrieve data
    async fn retrieve(&self, key: &str) -> anyhow::Result<Option<Vec<u8>>>;
    
    /// Delete data
    async fn delete(&self, key: &str) -> anyhow::Result<()>;
    
    /// List keys
    async fn list_keys(&self, prefix: &str) -> anyhow::Result<Vec<String>>;
}

/// File-based storage backend
pub struct FileStorage {
    base_path: PathBuf,
}

impl FileStorage {
    /// Create file storage
    pub fn new(base_path: impl Into<PathBuf>) -> anyhow::Result<Self> {
        let base_path = base_path.into();
        std::fs::create_dir_all(&base_path)?;
        
        Ok(Self { base_path })
    }

    /// Get file path for key
    fn key_to_path(&self, key: &str) -> PathBuf {
        // Sanitize key for filesystem
        let safe_key = key.replace('/', "_").replace('\\', "_");
        self.base_path.join(format!("{}.bin", safe_key))
    }
}

#[async_trait]
impl StorageBackend for FileStorage {
    fn name(&self) -> &str {
        "file"
    }

    async fn is_available(&self) -> bool {
        self.base_path.exists()
    }

    async fn store(&self, key: &str, data: &[u8]) -> anyhow::Result<()> {
        let path = self.key_to_path(key);
        let temp_path = path.with_extension("tmp");
        
        // Write to temp file first
        tokio::fs::write(&temp_path, data).await?;
        
        // Atomic rename
        tokio::fs::rename(&temp_path, &path).await?;
        
        debug!("Stored {} bytes to {:?}", data.len(), path);
        Ok(())
    }

    async fn retrieve(&self, key: &str) -> anyhow::Result<Option<Vec<u8>>> {
        let path = self.key_to_path(key);
        
        match tokio::fs::read(&path).await {
            Ok(data) => {
                debug!("Retrieved {} bytes from {:?}", data.len(), path);
                Ok(Some(data))
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    async fn delete(&self, key: &str) -> anyhow::Result<()> {
        let path = self.key_to_path(key);
        match tokio::fs::remove_file(&path).await {
            Ok(()) => {
                debug!("Deleted {:?}", path);
                Ok(())
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e.into()),
        }
    }

    async fn list_keys(&self, prefix: &str) -> anyhow::Result<Vec<String>> {
        let mut keys = Vec::new();
        
        let mut entries = tokio::fs::read_dir(&self.base_path).await?;
        while let Some(entry) = entries.next_entry().await? {
            let name = entry.file_name().to_string_lossy().to_string();
            if name.starts_with(prefix) && name.ends_with(".bin") {
                let key = name.trim_end_matches(".bin").to_string();
                keys.push(key);
            }
        }
        
        Ok(keys)
    }
}

/// In-memory storage backend
pub struct MemoryStorage {
    data: RwLock<HashMap<String, Vec<u8>>>,
}

impl MemoryStorage {
    /// Create memory storage
    pub fn new() -> Self {
        Self {
            data: RwLock::new(HashMap::new()),
        }
    }
}

impl Default for MemoryStorage {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl StorageBackend for MemoryStorage {
    fn name(&self) -> &str {
        "memory"
    }

    async fn is_available(&self) -> bool {
        true
    }

    async fn store(&self, key: &str, data: &[u8]) -> anyhow::Result<()> {
        let mut store = self.data.write().await;
        store.insert(key.to_string(), data.to_vec());
        Ok(())
    }

    async fn retrieve(&self, key: &str) -> anyhow::Result<Option<Vec<u8>>> {
        let store = self.data.read().await;
        Ok(store.get(key).cloned())
    }

    async fn delete(&self, key: &str) -> anyhow::Result<()> {
        let mut store = self.data.write().await;
        store.remove(key);
        Ok(())
    }

    async fn list_keys(&self, prefix: &str) -> anyhow::Result<Vec<String>> {
        let store = self.data.read().await;
        Ok(store
            .keys()
            .filter(|k| k.starts_with(prefix))
            .cloned()
            .collect())
    }
}

/// Migration function type
pub type MigrationFn = Box<dyn Fn(serde_json::Value) -> anyhow::Result<serde_json::Value> + Send + Sync>;

/// Migration registry
pub struct MigrationRegistry {
    migrations: HashMap<String, HashMap<u32, MigrationFn>>,
}

impl MigrationRegistry {
    /// Create migration registry
    pub fn new() -> Self {
        Self {
            migrations: HashMap::new(),
        }
    }

    /// Register a migration
    pub fn register(
        &mut self,
        entity_type: impl Into<String>,
        from_version: u32,
        migration: MigrationFn,
    ) {
        self.migrations
            .entry(entity_type.into())
            .or_default()
            .insert(from_version, migration);
    }

    /// Get migration
    pub fn get(&self, entity_type: &str, from_version: u32) -> Option<&MigrationFn> {
        self.migrations
            .get(entity_type)
            .and_then(|m| m.get(&from_version))
    }
}

impl Default for MigrationRegistry {
    fn default() -> Self {
        Self::new()
    }
}

/// Persistence manager
pub struct PersistenceManager {
    backend: Arc<dyn StorageBackend>,
    migrations: Arc<MigrationRegistry>,
    entity_types: RwLock<HashMap<String, u32>>, // type -> current version
}

impl PersistenceManager {
    /// Create persistence manager
    pub fn new(backend: Arc<dyn StorageBackend>, migrations: Arc<MigrationRegistry>) -> Self {
        Self {
            backend,
            migrations,
            entity_types: RwLock::new(HashMap::new()),
        }
    }

    /// Register entity type with current version
    pub async fn register_type(&self, type_name: impl Into<String>, version: u32) {
        let mut types = self.entity_types.write().await;
        types.insert(type_name.into(), version);
    }

    /// Save entity
    pub async fn save<T: Persistable>(&self, entity: &T) -> anyhow::Result<()> {
        let data = serde_json::to_vec(entity)?;
        let key = format!("{}:{}", std::any::type_name::<T>(), entity.id());
        
        self.backend.store(&key, &data).await?;
        info!("Saved entity: {}", key);
        
        Ok(())
    }

    /// Load entity
    pub async fn load<T: Persistable>(&self, id: &str) -> anyhow::Result<Option<T>> {
        let key = format!("{}:{}", std::any::type_name::<T>(), id);
        
        let data = match self.backend.retrieve(&key).await? {
            Some(d) => d,
            None => return Ok(None),
        };

        let mut value: serde_json::Value = serde_json::from_slice(&data)?;
        
        // Check version and migrate if needed
        let type_name = std::any::type_name::<T>();
        let current_version = {
            let types = self.entity_types.read().await;
            types.get(type_name).copied().unwrap_or(1)
        };
        
        let stored_version = value.get("version")
            .and_then(|v| v.as_u64())
            .unwrap_or(1) as u32;
        
        if stored_version < current_version {
            info!("Migrating {} from v{} to v{}", id, stored_version, current_version);
            
            // Apply migrations
            let mut version = stored_version;
            while version < current_version {
                if let Some(migration) = self.migrations.get(type_name, version) {
                    value = migration(value)?;
                    version += 1;
                } else {
                    warn!("No migration from v{} for {}", version, type_name);
                    break;
                }
            }
        }

        let entity: T = serde_json::from_value(value)?;
        Ok(Some(entity))
    }

    /// Delete entity
    pub async fn delete<T: Persistable>(&self, id: &str) -> anyhow::Result<()> {
        let key = format!("{}:{}", std::any::type_name::<T>(), id);
        self.backend.delete(&key).await?;
        info!("Deleted entity: {}", key);
        Ok(())
    }

    /// List entities of type
    pub async fn list<T: Persistable>(&self) -> anyhow::Result<Vec<T>> {
        let prefix = std::any::type_name::<T>();
        let keys = self.backend.list_keys(prefix).await?;
        
        let mut entities = Vec::new();
        for key in keys {
            let id = key.strip_prefix(&format!("{}:", prefix))
                .unwrap_or(&key);
            
            if let Some(entity) = self.load::<T>(id).await? {
                entities.push(entity);
            }
        }
        
        Ok(entities)
    }

    /// Check health
    pub async fn health_check(&self) -> anyhow::Result<()> {
        if !self.backend.is_available().await {
            anyhow::bail!("Storage backend is not available");
        }
        Ok(())
    }
}

/// Snapshot of data at a point in time
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Snapshot {
    pub id: String,
    pub timestamp: chrono::DateTime<chrono::Utc>,
    pub entity_count: usize,
    pub metadata: HashMap<String, String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, Clone, Serialize, Deserialize)]
    struct TestEntity {
        id: String,
        name: String,
        version: u32,
    }

    impl Persistable for TestEntity {
        fn id(&self) -> &str {
            &self.id
        }

        fn version(&self) -> u32 {
            self.version
        }
    }

    #[tokio::test]
    async fn test_memory_storage() {
        let storage = Arc::new(MemoryStorage::new());
        let migrations = Arc::new(MigrationRegistry::new());
        let manager = PersistenceManager::new(storage, migrations);

        let entity = TestEntity {
            id: "test-1".to_string(),
            name: "Test".to_string(),
            version: 1,
        };

        manager.save(&entity).await.unwrap();
        
        let loaded = manager.load::<TestEntity>("test-1").await.unwrap();
        assert!(loaded.is_some());
        assert_eq!(loaded.unwrap().name, "Test");
    }

    #[test]
    fn test_key_sanitization() {
        let storage = FileStorage::new("/tmp/test").unwrap();
        let path = storage.key_to_path("foo/bar\\baz");
        let name = path.file_name().unwrap().to_string_lossy();
        assert!(name.contains("_"));
    }
}
