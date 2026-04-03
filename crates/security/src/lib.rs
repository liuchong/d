//! Security and approval system for AI Daemon
//!
//! Handles permission checks, tool execution approval, and audit logging.

pub mod approval;
pub mod audit;
pub mod policy;

pub use approval::ApprovalSystem;
pub use audit::AuditLog;
pub use policy::Policy;
