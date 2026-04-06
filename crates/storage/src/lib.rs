//! Storage - Data storage abstractions
//!
//! Provides:
//! - Key-value storage
//! - Document storage
//! - Blob storage
//! - Cache storage

pub mod kv;
pub mod document;
pub mod blob;
pub mod cache;

pub use kv::*;
pub use document::*;
pub use blob::*;
pub use cache::*;

use async_trait::async_trait;
use std::sync::Arc;

/// Storage backend trait
#[async_trait]
pub trait Storage: Send + Sync {
    /// Storage name
    fn name(&self) -> &str;
    
    /// Check if storage is healthy
    async fn health_check(&self) -> anyhow::Result<()>;
    
    /// Close storage connection
    async fn close(&self) -> anyhow::Result<()>;
}

/// Storage factory
pub struct StorageFactory;

impl StorageFactory {
    /// Create memory storage
    pub fn memory() -> Arc<dyn Storage> {
        Arc::new(kv::MemoryKvStore::new())
    }
}
