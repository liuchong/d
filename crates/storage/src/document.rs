//! Document storage for structured data

use async_trait::async_trait;
use serde::{de::DeserializeOwned, Serialize};
use std::collections::HashMap;
use tracing::{debug, error, info, warn};

/// Document with metadata
#[derive(Debug, Clone)]
pub struct Document<T> {
    /// Document ID
    pub id: String,
    /// Document content
    pub content: T,
    /// Creation timestamp
    pub created_at: chrono::DateTime<chrono::Utc>,
    /// Last update timestamp
    pub updated_at: chrono::DateTime<chrono::Utc>,
    /// Document version
    pub version: u64,
    /// Metadata
    pub metadata: HashMap<String, String>,
}

impl<T> Document<T> {
    /// Create new document
    pub fn new(id: impl Into<String>, content: T) -> Self {
        let now = chrono::Utc::now();
        Self {
            id: id.into(),
            content,
            created_at: now,
            updated_at: now,
            version: 1,
            metadata: HashMap::new(),
        }
    }

    /// Add metadata
    pub fn with_metadata(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.metadata.insert(key.into(), value.into());
        self
    }

    /// Bump version
    pub fn bump_version(&mut self) {
        self.version += 1;
        self.updated_at = chrono::Utc::now();
    }
}

/// Document store trait
#[async_trait]
pub trait DocumentStore<T>: Send + Sync
where
    T: Serialize + DeserializeOwned + Send + Sync,
{
    /// Get document by ID
    async fn get(&self, id: &str) -> anyhow::Result<Option<Document<T>>>;

    /// Save document
    async fn save(&self, document: Document<T>) -> anyhow::Result<()>;

    /// Delete document
    async fn delete(&self, id: &str) -> anyhow::Result<()>;

    /// List all documents
    async fn list(&self) -> anyhow::Result<Vec<Document<T>>>;

    /// Query documents
    async fn query(&self, query: DocumentQuery) -> anyhow::Result<Vec<Document<T>>>;

    /// Count documents
    async fn count(&self) -> anyhow::Result<usize>;
}

/// Query criteria
#[derive(Debug, Clone, Default)]
pub struct DocumentQuery {
    /// Filter by metadata key-value pairs
    pub metadata_filters: HashMap<String, String>,
    /// Sort by field
    pub sort_by: Option<String>,
    /// Sort ascending
    pub sort_asc: bool,
    /// Maximum results
    pub limit: Option<usize>,
    /// Skip results
    pub offset: Option<usize>,
}

impl DocumentQuery {
    /// Create new query
    pub fn new() -> Self {
        Self::default()
    }

    /// Filter by metadata
    pub fn with_metadata(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.metadata_filters.insert(key.into(), value.into());
        self
    }

    /// Sort by field
    pub fn sort_by(mut self, field: impl Into<String>, asc: bool) -> Self {
        self.sort_by = Some(field.into());
        self.sort_asc = asc;
        self
    }

    /// Limit results
    pub fn limit(mut self, limit: usize) -> Self {
        self.limit = Some(limit);
        self
    }

    /// Offset results
    pub fn offset(mut self, offset: usize) -> Self {
        self.offset = Some(offset);
        self
    }
}

/// In-memory document store
pub struct MemoryDocumentStore<T> {
    documents: tokio::sync::RwLock<HashMap<String, Document<T>>>,
}

impl<T> MemoryDocumentStore<T> {
    /// Create new store
    pub fn new() -> Self {
        Self {
            documents: tokio::sync::RwLock::new(HashMap::new()),
        }
    }
}

impl<T> Default for MemoryDocumentStore<T> {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl<T> DocumentStore<T> for MemoryDocumentStore<T>
where
    T: Serialize + DeserializeOwned + Send + Sync + Clone,
{
    async fn get(&self, id: &str) -> anyhow::Result<Option<Document<T>>> {
        let docs = self.documents.read().await;
        Ok(docs.get(id).cloned())
    }

    async fn save(&self, document: Document<T>) -> anyhow::Result<()> {
        let mut docs = self.documents.write().await;
        docs.insert(document.id.clone(), document);
        Ok(())
    }

    async fn delete(&self, id: &str) -> anyhow::Result<()> {
        let mut docs = self.documents.write().await;
        docs.remove(id);
        Ok(())
    }

    async fn list(&self) -> anyhow::Result<Vec<Document<T>>> {
        let docs = self.documents.read().await;
        Ok(docs.values().cloned().collect())
    }

    async fn query(&self, query: DocumentQuery) -> anyhow::Result<Vec<Document<T>>> {
        let docs = self.documents.read().await;
        
        let mut results: Vec<Document<T>> = docs
            .values()
            .filter(|doc| {
                query.metadata_filters.iter().all(|(k, v)| {
                    doc.metadata.get(k) == Some(v)
                })
            })
            .cloned()
            .collect();

        // Sort
        if let Some(sort_by) = query.sort_by {
            match sort_by.as_str() {
                "id" => {
                    results.sort_by(|a, b| {
                        if query.sort_asc {
                            a.id.cmp(&b.id)
                        } else {
                            b.id.cmp(&a.id)
                        }
                    });
                }
                "created" => {
                    results.sort_by(|a, b| {
                        if query.sort_asc {
                            a.created_at.cmp(&b.created_at)
                        } else {
                            b.created_at.cmp(&a.created_at)
                        }
                    });
                }
                "updated" => {
                    results.sort_by(|a, b| {
                        if query.sort_asc {
                            a.updated_at.cmp(&b.updated_at)
                        } else {
                            b.updated_at.cmp(&a.updated_at)
                        }
                    });
                }
                _ => {}
            }
        }

        // Apply offset
        if let Some(offset) = query.offset {
            results = results.into_iter().skip(offset).collect();
        }

        // Apply limit
        if let Some(limit) = query.limit {
            results.truncate(limit);
        }

        Ok(results)
    }

    async fn count(&self) -> anyhow::Result<usize> {
        let docs = self.documents.read().await;
        Ok(docs.len())
    }
}

/// Collection for type-specific storage
pub struct Collection<T> {
    name: String,
    store: Box<dyn DocumentStore<T>>,
}

impl<T> Collection<T> {
    /// Create collection with store
    pub fn new(name: impl Into<String>, store: Box<dyn DocumentStore<T>>) -> Self {
        Self {
            name: name.into(),
            store,
        }
    }

    /// Get document
    pub async fn get(&self, id: &str) -> anyhow::Result<Option<Document<T>>>
    where
        T: Serialize + DeserializeOwned + Send + Sync,
    {
        self.store.get(id).await
    }

    /// Insert document
    pub async fn insert(&self, id: impl Into<String>, content: T) -> anyhow::Result<Document<T>>
    where
        T: Serialize + DeserializeOwned + Send + Sync,
    {
        let id_string = id.into();
        let doc = Document::new(id_string.clone(), content);
        self.store.save(doc).await?;
        self.get(&id_string).await?.ok_or_else(|| anyhow::anyhow!("Failed to retrieve saved document"))
    }

    /// Update document
    pub async fn update(&self, id: &str, content: T) -> anyhow::Result<Document<T>>
    where
        T: Serialize + DeserializeOwned + Clone + Send + Sync,
    {
        if let Some(mut doc) = self.store.get(id).await? {
            doc.content = content;
            doc.bump_version();
            self.store.save(doc.clone()).await?;
            Ok(doc)
        } else {
            anyhow::bail!("Document not found: {}", id)
        }
    }

    /// Delete document
    pub async fn delete(&self, id: &str) -> anyhow::Result<()>
    where
        T: Serialize + DeserializeOwned + Send + Sync,
    {
        self.store.delete(id).await
    }

    /// Find documents
    pub async fn find(&self, query: DocumentQuery) -> anyhow::Result<Vec<Document<T>>>
    where
        T: Serialize + DeserializeOwned + Send + Sync,
    {
        self.store.query(query).await
    }

    /// Count documents
    pub async fn count(&self) -> anyhow::Result<usize>
    where
        T: Serialize + DeserializeOwned + Send + Sync,
    {
        self.store.count().await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq)]
    struct TestDoc {
        title: String,
        content: String,
    }

    #[tokio::test]
    async fn test_document_crud() {
        let store = MemoryDocumentStore::<TestDoc>::new();
        
        // Create
        let doc = Document::new("doc1", TestDoc {
            title: "Test".to_string(),
            content: "Content".to_string(),
        });
        store.save(doc).await.unwrap();
        
        // Read
        let retrieved = store.get("doc1").await.unwrap().unwrap();
        assert_eq!(retrieved.content.title, "Test");
        assert_eq!(retrieved.version, 1);
        
        // Delete
        store.delete("doc1").await.unwrap();
        assert!(store.get("doc1").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn test_query() {
        let store = MemoryDocumentStore::<TestDoc>::new();
        
        let doc1 = Document::new("doc1", TestDoc {
            title: "A".to_string(),
            content: "Content".to_string(),
        }).with_metadata("type", "post");
        
        let doc2 = Document::new("doc2", TestDoc {
            title: "B".to_string(),
            content: "Content".to_string(),
        }).with_metadata("type", "page");
        
        store.save(doc1).await.unwrap();
        store.save(doc2).await.unwrap();
        
        let query = DocumentQuery::new()
            .with_metadata("type", "post");
        
        let results = store.query(query).await.unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].id, "doc1");
    }

    #[tokio::test]
    async fn test_collection() {
        let store = MemoryDocumentStore::<TestDoc>::new();
        let collection = Collection::new("posts", Box::new(store));
        
        let doc = collection.insert("post1", TestDoc {
            title: "Hello".to_string(),
            content: "World".to_string(),
        }).await.unwrap();
        
        assert_eq!(doc.id, "post1");
        assert_eq!(collection.count().await.unwrap(), 1);
    }
}
