//! Document chunking for RAG
//!
//! Splits documents into manageable chunks for indexing and retrieval.

use regex::Regex;

/// Chunking strategy
#[derive(Debug, Clone, Copy)]
pub enum ChunkStrategy {
    /// Fixed size chunks with overlap
    FixedSize { size: usize, overlap: usize },
    /// Split by paragraphs
    Paragraphs,
    /// Split by sentences
    Sentences,
    /// Split by code blocks and paragraphs
    CodeAware,
}

impl Default for ChunkStrategy {
    fn default() -> Self {
        ChunkStrategy::FixedSize { size: 1000, overlap: 200 }
    }
}

/// Document chunk
#[derive(Debug, Clone)]
pub struct Chunk {
    pub id: String,
    pub content: String,
    pub source: String,
    pub start_pos: usize,
    pub end_pos: usize,
    pub metadata: ChunkMetadata,
}

#[derive(Debug, Clone, Default)]
pub struct ChunkMetadata {
    pub title: Option<String>,
    pub file_path: Option<String>,
    pub line_start: Option<usize>,
    pub line_end: Option<usize>,
}

/// Document chunker
pub struct Chunker {
    strategy: ChunkStrategy,
}

impl Chunker {
    pub fn new(strategy: ChunkStrategy) -> Self {
        Self { strategy }
    }

    /// Chunk text into pieces
    pub fn chunk(&self, text: &str, source: impl Into<String>) -> Vec<Chunk> {
        let source = source.into();
        
        match self.strategy {
            ChunkStrategy::FixedSize { size, overlap } => {
                self.chunk_fixed_size(text, &source, size, overlap)
            }
            ChunkStrategy::Paragraphs => {
                self.chunk_by_paragraphs(text, &source)
            }
            ChunkStrategy::Sentences => {
                self.chunk_by_sentences(text, &source)
            }
            ChunkStrategy::CodeAware => {
                self.chunk_code_aware(text, &source)
            }
        }
    }

    fn chunk_fixed_size(&self, text: &str, source: &str, size: usize, overlap: usize) -> Vec<Chunk> {
        let mut chunks = Vec::new();
        let mut start = 0;
        let mut chunk_id = 0;

        while start < text.len() {
            let end = (start + size).min(text.len());
            let content = text[start..end].to_string();
            
            chunks.push(Chunk {
                id: format!("{}-{}", source, chunk_id),
                content,
                source: source.to_string(),
                start_pos: start,
                end_pos: end,
                metadata: ChunkMetadata::default(),
            });

            chunk_id += 1;
            start += size - overlap;
            
            if start >= text.len() || end == text.len() {
                break;
            }
        }

        chunks
    }

    fn chunk_by_paragraphs(&self, text: &str, source: &str) -> Vec<Chunk> {
        let paragraph_regex = Regex::new(r"\n\s*\n").unwrap();
        let paragraphs: Vec<&str> = paragraph_regex.split(text).collect();
        
        paragraphs
            .into_iter()
            .enumerate()
            .filter(|(_, p)| !p.trim().is_empty())
            .map(|(i, p)| {
                let content = p.trim().to_string();
                Chunk {
                    id: format!("{}-p{}", source, i),
                    content,
                    source: source.to_string(),
                    start_pos: 0,
                    end_pos: 0,
                    metadata: ChunkMetadata::default(),
                }
            })
            .collect()
    }

    fn chunk_by_sentences(&self, text: &str, source: &str) -> Vec<Chunk> {
        // Simple sentence splitting
        let sentence_regex = Regex::new(r"[.!?]+\s+").unwrap();
        let sentences: Vec<&str> = sentence_regex.split(text).collect();
        
        sentences
            .into_iter()
            .enumerate()
            .filter(|(_, s)| !s.trim().is_empty())
            .map(|(i, s)| {
                let content = s.trim().to_string();
                Chunk {
                    id: format!("{}-s{}", source, i),
                    content,
                    source: source.to_string(),
                    start_pos: 0,
                    end_pos: 0,
                    metadata: ChunkMetadata::default(),
                }
            })
            .collect()
    }

    fn chunk_code_aware(&self, text: &str, source: &str) -> Vec<Chunk> {
        // Try to split by code blocks first, then fall back to paragraphs
        let code_block_regex = Regex::new(r"```[\s\S]*?```|`[^`]+`").unwrap();
        
        let mut chunks = Vec::new();
        let mut last_end = 0;
        let mut chunk_id = 0;

        for mat in code_block_regex.find_iter(text) {
            // Add text before code block as paragraph chunk
            if mat.start() > last_end {
                let text_chunk = text[last_end..mat.start()].trim();
                if !text_chunk.is_empty() {
                    for para in text_chunk.split("\n\n") {
                        if !para.trim().is_empty() {
                            chunks.push(Chunk {
                                id: format!("{}-{}", source, chunk_id),
                                content: para.trim().to_string(),
                                source: source.to_string(),
                                start_pos: last_end,
                                end_pos: mat.start(),
                                metadata: ChunkMetadata::default(),
                            });
                            chunk_id += 1;
                        }
                    }
                }
            }

            // Add code block as its own chunk
            chunks.push(Chunk {
                id: format!("{}-{}", source, chunk_id),
                content: mat.as_str().to_string(),
                source: source.to_string(),
                start_pos: mat.start(),
                end_pos: mat.end(),
                metadata: ChunkMetadata {
                    title: Some("Code block".to_string()),
                    ..Default::default()
                },
            });
            chunk_id += 1;

            last_end = mat.end();
        }

        // Add remaining text
        if last_end < text.len() {
            let remaining = text[last_end..].trim();
            if !remaining.is_empty() {
                for para in remaining.split("\n\n") {
                    if !para.trim().is_empty() {
                        chunks.push(Chunk {
                            id: format!("{}-{}", source, chunk_id),
                            content: para.trim().to_string(),
                            source: source.to_string(),
                            start_pos: last_end,
                            end_pos: text.len(),
                            metadata: ChunkMetadata::default(),
                        });
                        chunk_id += 1;
                    }
                }
            }
        }

        chunks
    }
}

impl Default for Chunker {
    fn default() -> Self {
        Self::new(ChunkStrategy::default())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_fixed_size_chunking() {
        let chunker = Chunker::new(ChunkStrategy::FixedSize { size: 100, overlap: 20 });
        let text = "a".repeat(250);
        let chunks = chunker.chunk(&text, "test");
        
        assert!(!chunks.is_empty());
        assert_eq!(chunks[0].content.len(), 100);
    }

    #[test]
    fn test_paragraph_chunking() {
        let chunker = Chunker::new(ChunkStrategy::Paragraphs);
        let text = "First paragraph.\n\nSecond paragraph.\n\nThird paragraph.";
        let chunks = chunker.chunk(text, "test");
        
        assert_eq!(chunks.len(), 3);
    }

    #[test]
    fn test_sentence_chunking() {
        let chunker = Chunker::new(ChunkStrategy::Sentences);
        let text = "First sentence. Second sentence! Third sentence?";
        let chunks = chunker.chunk(text, "test");
        
        assert_eq!(chunks.len(), 3);
    }

    #[test]
    fn test_code_aware_chunking() {
        let chunker = Chunker::new(ChunkStrategy::CodeAware);
        let text = r#"Some text.

```rust
fn main() {}
```

More text."#;
        let chunks = chunker.chunk(text, "test");
        
        assert!(chunks.len() >= 2);
        assert!(chunks.iter().any(|c| c.content.contains("```")));
    }
}
