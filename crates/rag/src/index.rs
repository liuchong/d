//! Search index for RAG
//!
//! Provides keyword-based retrieval (simplified BM25-like scoring).

use crate::chunker::Chunk;
use std::collections::{HashMap, HashSet};

/// Search index entry
#[derive(Debug, Clone)]
pub struct IndexEntry {
    pub chunk: Chunk,
    pub term_freq: HashMap<String, f32>,
}

/// Simple keyword-based index
#[derive(Debug, Default)]
pub struct SearchIndex {
    entries: Vec<IndexEntry>,
    // Inverted index: term -> [entry indices]
    inverted_index: HashMap<String, Vec<usize>>,
    // Document frequency: term -> number of documents containing term
    doc_freq: HashMap<String, usize>,
    // Total documents
    total_docs: usize,
}

impl SearchIndex {
    pub fn new() -> Self {
        Self::default()
    }

    /// Add chunks to the index
    pub fn add_chunks(&mut self, chunks: Vec<Chunk>) {
        for chunk in chunks {
            self.add_chunk(chunk);
        }
    }

    /// Add a single chunk
    pub fn add_chunk(&mut self, chunk: Chunk) {
        let terms = tokenize(&chunk.content);
        let term_freq = calculate_term_freq(&terms);
        
        let entry_id = self.entries.len();
        
        // Update inverted index
        let unique_terms: HashSet<_> = terms.iter().cloned().collect();
        for term in unique_terms {
            self.inverted_index
                .entry(term.clone())
                .or_default()
                .push(entry_id);
            *self.doc_freq.entry(term).or_insert(0) += 1;
        }
        
        self.entries.push(IndexEntry {
            chunk,
            term_freq,
        });
        
        self.total_docs += 1;
    }

    /// Search for relevant chunks
    pub fn search(&self, query: &str, top_k: usize) -> Vec<SearchResult> {
        let query_terms = tokenize(query);
        let query_tf = calculate_term_freq(&query_terms);
        
        let mut scores: HashMap<usize, f32> = HashMap::new();
        
        // Calculate score for each query term
        for (term, query_weight) in query_tf {
            if let Some(entry_indices) = self.inverted_index.get(&term) {
                let idf = calculate_idf(self.total_docs, *self.doc_freq.get(&term).unwrap_or(&1));
                
                for &entry_idx in entry_indices {
                    let entry = &self.entries[entry_idx];
                    let term_weight = *entry.term_freq.get(&term).unwrap_or(&0.0);
                    
                    let score = query_weight * term_weight * idf;
                    *scores.entry(entry_idx).or_insert(0.0) += score;
                }
            }
        }
        
        // Sort by score and return top_k
        let mut results: Vec<_> = scores
            .into_iter()
            .map(|(idx, score)| {
                let entry = &self.entries[idx];
                SearchResult {
                    chunk: entry.chunk.clone(),
                    score,
                }
            })
            .collect();
        
        results.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap());
        results.truncate(top_k);
        
        results
    }

    /// Get total number of indexed chunks
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Check if index is empty
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Clear the index
    pub fn clear(&mut self) {
        self.entries.clear();
        self.inverted_index.clear();
        self.doc_freq.clear();
        self.total_docs = 0;
    }
}

/// Search result
#[derive(Debug, Clone)]
pub struct SearchResult {
    pub chunk: Chunk,
    pub score: f32,
}

/// Simple tokenizer
fn tokenize(text: &str) -> Vec<String> {
    text.to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|s| !s.is_empty() && s.len() > 2)
        .map(|s| s.to_string())
        .collect()
}

/// Calculate term frequency
fn calculate_term_freq(terms: &[String]) -> HashMap<String, f32> {
    let mut freq = HashMap::new();
    let total = terms.len() as f32;
    
    for term in terms {
        *freq.entry(term.clone()).or_insert(0.0) += 1.0 / total;
    }
    
    freq
}

/// Calculate inverse document frequency
fn calculate_idf(total_docs: usize, doc_freq: usize) -> f32 {
    let total = total_docs as f32;
    let df = doc_freq.max(1) as f32;
    (total / df).ln().max(0.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chunker::{Chunk, ChunkMetadata};

    fn create_test_chunk(id: &str, content: &str) -> Chunk {
        Chunk {
            id: id.to_string(),
            content: content.to_string(),
            source: "test".to_string(),
            start_pos: 0,
            end_pos: content.len(),
            metadata: ChunkMetadata::default(),
        }
    }

    #[test]
    fn test_index_creation() {
        let index = SearchIndex::new();
        assert!(index.is_empty());
    }

    #[test]
    fn test_add_and_search() {
        let mut index = SearchIndex::new();
        
        let chunks = vec![
            create_test_chunk("1", "Rust is a systems programming language"),
            create_test_chunk("2", "Python is great for data science"),
            create_test_chunk("3", "Rust provides memory safety"),
        ];
        
        index.add_chunks(chunks);
        assert_eq!(index.len(), 3);
        
        let results = index.search("rust programming", 2);
        assert!(!results.is_empty());
        assert!(results[0].chunk.content.to_lowercase().contains("rust"));
    }

    #[test]
    fn test_scoring() {
        let mut index = SearchIndex::new();
        
        let chunks = vec![
            create_test_chunk("1", "apple banana cherry"),
            create_test_chunk("2", "apple apple banana"),
            create_test_chunk("3", "banana cherry date"),
        ];
        
        index.add_chunks(chunks);
        
        let results = index.search("apple", 3);
        assert_eq!(results.len(), 2);
        // Document with more "apple" should score higher
        assert!(results[0].chunk.content.contains("apple apple") || 
                results[1].chunk.content.contains("apple apple"));
    }

    #[test]
    fn test_clear() {
        let mut index = SearchIndex::new();
        index.add_chunk(create_test_chunk("1", "test content"));
        assert_eq!(index.len(), 1);
        
        index.clear();
        assert!(index.is_empty());
    }
}
