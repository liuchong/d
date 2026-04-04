//! AI Agent orchestration layer
//!
//! Provides ReAct-style agent loop with tool calling, planning,
//! and integration with memory/session layers.

pub mod agent;
pub mod cost;
pub mod plan;
pub mod planner;
pub mod reactor;

pub use agent::Agent;
pub use plan::{PlanMode, is_tool_allowed_in_plan_mode};
pub use planner::Planner;
pub use reactor::ReActLoop;
