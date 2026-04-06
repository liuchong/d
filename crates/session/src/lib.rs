//! Session management with persistence
//!
//! Provides session storage, indexing, and search capabilities.

pub mod session;
pub mod store;

pub use session::{
    Session, SessionInfo, SessionMessage, SessionSearch,
};
pub use store::{SessionStore, StoreStats};
