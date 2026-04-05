//! RAG (Retrieval-Augmented Generation) implementation
//!
//! Provides document chunking, indexing, and retrieval for augmenting
//! LLM responses with relevant context.
//!
//! ## Example
//!
//! ```rust
//! use rag::RagEngine;
//!
//! let mut engine = RagEngine::new();
//!
//! // Add documents
//! engine.add_document("Rust is a systems programming language.", "rust_doc");
//! engine.add_document("Python is great for scripting.", "python_doc");
//!
//! // Query
//! let results = engine.query("programming languages");
//! for result in results {
//!     println!("{}: {}", result.source, result.content);
//! }
//! ```

pub mod chunker;
pub mod engine;
pub mod index;

pub use chunker::{Chunk, ChunkMetadata, ChunkStrategy, Chunker};
pub use engine::{RagEngine, RagConfig, RetrievalResult, RagStats};
pub use index::{SearchIndex, SearchResult};
