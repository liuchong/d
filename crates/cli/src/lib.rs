//! Command-line interface for AI Daemon
//!
//! Provides interactive chat with tool execution capabilities.

pub mod chat;
pub mod repl;
pub mod ui;

pub use chat::ChatSession;
pub use repl::Repl;
