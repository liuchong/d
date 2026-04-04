//! Security and approval system for AI Daemon
//!
//! Handles permission checks, tool execution approval, and audit logging.

pub mod approval;
pub mod audit;
pub mod checker;
pub mod policy;

pub use approval::{ApprovalSystem, ApprovalRequest, ApprovalDecision};
pub use audit::AuditLog;
pub use policy::Policy;
