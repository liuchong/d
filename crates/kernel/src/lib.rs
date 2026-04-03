pub mod config;
pub mod daemon;
pub mod error;
pub mod module;
pub mod state;

pub use async_trait::async_trait;
pub use config::Config;
pub use daemon::Daemon;
pub use error::{Error, Result};
pub use module::{Module, ModuleContext};
pub use state::SharedState;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
