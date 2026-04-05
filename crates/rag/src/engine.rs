//! RAG (Retrieval-Augmented Generation) Engine

use crate::chunker::{Chunk, ChunkMetadata, ChunkStrategy, Chunker};
use crate::index::{SearchIndex, SearchResult};
use anyhow::Result;

/// RAG Engine
pub struct RagEngine {
    chunker: Chunker,
    index: SearchIndex,
    config: RagConfig,
}

#[derive(Debug, Clone)]
pub struct RagConfig {
    pub chunk_strategy: ChunkStrategy,
    pub top_k: usize,
    pub min_score: f32,
}

impl Default for RagConfig {
    fn default() -> Self {
        Self {
            chunk_strategy: ChunkStrategy::CodeAware,
            top_k: 5,
            min_score: 0.0,
        }
    }
}

impl RagEngine {
    pub fn new() -> Self {
        Self::with_config(RagConfig::default())
    }

    pub fn with_config(config: RagConfig) -> Self {
        let chunker = Chunker::new(config.chunk_strategy);
        Self {
            chunker,
            index: SearchIndex::new(),
            config,
        }
    }

    /// Add a document to the RAG index
    pub fn add_document(&mut self, content: &str, source: impl Into<String>) {
        let source = source.into();
        let chunks = self.chunker.chunk(content, &source);
        self.index.add_chunks(chunks);
    }

    /// Add a document with custom metadata
    pub fn add_document_with_metadata(
        &mut self,
        content: &str,
        source: impl Into<String>,
        title: Option<String>,
        file_path: Option<String>,
    ) {
        let source = source.into();
        let mut chunks = self.chunker.chunk(content, &source);
        
        // Update metadata
        for chunk in &mut chunks {
            chunk.metadata.title = title.clone();
            chunk.metadata.file_path = file_path.clone();
        }
        
        self.index.add_chunks(chunks);
    }

    /// Query the RAG engine
    pub fn query(&self, query: &str) -> Vec<RetrievalResult> {
        let results = self.index.search(query, self.config.top_k);
        
        results
            .into_iter()
            .filter(|r| r.score >= self.config.min_score)
            .map(|r| RetrievalResult {
                content: r.chunk.content,
                source: r.chunk.source,
                score: r.score,
                metadata: r.chunk.metadata,
            })
            .collect()
    }

    /// Query and format as context string
    pub fn query_as_context(&self, query: &str, max_tokens: usize) -> String {
        let results = self.query(query);
        
        if results.is_empty() {
            return String::new();
        }

        let mut context = String::new();
        let mut estimated_tokens = 0;
        let tokens_per_char = 0.25; // Rough estimate

        for result in results {
            let entry = format!(
                "[Source: {}]\n{}\n\n",
                result.source,
                result.content
            );
            
            let entry_tokens = (entry.len() as f32 * tokens_per_char) as usize;
            
            if estimated_tokens + entry_tokens > max_tokens {
                break;
            }
            
            context.push_str(&entry);
            estimated_tokens += entry_tokens;
        }

        context.trim().to_string()
    }

    /// Get index statistics
    pub fn stats(&self) -> RagStats {
        RagStats {
            total_chunks: self.index.len(),
            config: self.config.clone(),
        }
    }

    /// Clear all indexed documents
    pub fn clear(&mut self) {
        self.index.clear();
    }
}

/// Retrieval result
#[derive(Debug, Clone)]
pub struct RetrievalResult {
    pub content: String,
    pub source: String,
    pub score: f32,
    pub metadata: ChunkMetadata,
}

/// RAG statistics
#[derive(Debug, Clone)]
pub struct RagStats {
    pub total_chunks: usize,
    pub config: RagConfig,
}

/// Load documents from directory
pub async fn load_documents_from_dir(dir: &std::path::Path) -> Result<Vec<(String, String)>> {
    use tokio::fs;
    
    let mut documents = Vec::new();
    
    let mut entries = fs::read_dir(dir).await?;
    
    while let Some(entry) = entries.next_entry().await? {
        let path = entry.path();
        
        if path.is_file() {
            let content = fs::read_to_string(&path).await?;
            let filename = path.file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("unknown")
                .to_string();
            documents.push((filename, content));
        }
    }
    
    Ok(documents)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_rag_engine() {
        let mut engine = RagEngine::new();
        
        engine.add_document(
            "Rust is a systems programming language. It provides memory safety.",
            "rust_doc"
        );
        
        engine.add_document(
            "Python is great for scripting and data science.",
            "python_doc"
        );
        
        let results = engine.query("memory safety");
        assert!(!results.is_empty());
        assert!(results[0].content.to_lowercase().contains("rust"));
    }

    #[test]
    fn test_query_as_context() {
        let mut engine = RagEngine::new();
        
        engine.add_document(
            "First document about programming.",
            "doc1"
        );
        
        engine.add_document(
            "Second document about coding.",
            "doc2"
        );
        
        let context = engine.query_as_context("programming", 1000);
        assert!(!context.is_empty());
        assert!(context.contains("doc1") || context.contains("doc2"));
    }

    #[test]
    fn test_empty_query() {
        let engine = RagEngine::new();
        let results = engine.query("nonexistent topic");
        assert!(results.is_empty());
    }

    #[test]
    fn test_stats() {
        let mut engine = RagEngine::new();
        assert_eq!(engine.stats().total_chunks, 0);
        
        engine.add_document("Some content here.", "test");
        assert!(engine.stats().total_chunks > 0);
    }
}
