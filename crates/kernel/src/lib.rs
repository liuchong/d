pub mod config;
pub mod daemon;
pub mod env;
pub mod environment;
pub mod error;
pub mod integration;
pub mod log;
pub mod module;
pub mod optimization;
pub mod pcode;
pub mod persistence;
pub mod state;
pub mod worktree;

#[cfg(test)]
mod config_test;

pub use async_trait::async_trait;
pub use config::{AiConfig, Config, ServerConfig};
pub use daemon::Daemon;
pub use env::Environment;
pub use environment::EnvironmentInfo;
pub use error::{Error, Result};
pub use integration::IntegrationRegistry;
pub use module::{Module, ModuleContext};
pub use optimization::OptimizationAnalyzer;
pub use pcode::VM;
pub use persistence::PersistenceManager;
pub use state::SharedState;
pub use worktree::WorktreeManager;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
