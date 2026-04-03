//! Long-term memory storage for AI Daemon
//! 
//! Provides vector-based memory storage and retrieval for cross-session knowledge.

pub mod store;
pub mod embedding;

pub use store::{MemoryStore, MemoryEntry};
pub use embedding::EmbeddingProvider;
