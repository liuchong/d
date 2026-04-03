//! Context compaction for long conversations
//!
//! Manages conversation context size through intelligent compression:
//! - Token estimation for content
//! - Multiple compaction strategies
//! - Configurable thresholds

pub mod compaction;
pub mod token;

pub use compaction::{
    CompactionConfig, CompactionResult, CompactionStats, CompactionStrategy, ContextCompactor,
    ImportanceCompaction, SimpleCompaction, SlidingWindowCompaction,
};
pub use token::{estimate_message_tokens, estimate_messages_tokens, estimate_tokens};

#[cfg(test)]
mod compaction_test;
#[cfg(test)]
mod token_test;
