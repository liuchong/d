//! AI Agent orchestration layer
//!
//! Provides ReAct-style agent loop with tool calling, planning,
//! and integration with memory/session layers.

pub mod agent;
pub mod bgtask;
pub mod correction;
pub mod cost;
pub mod game;
pub mod lsp;
pub mod pattern;
pub mod personality;
pub mod plan;
pub mod planner;
pub mod plugin;
pub mod reactor;
pub mod skills;
pub mod thinking;
pub mod workflow;

pub use agent::Agent;
pub use plan::{PlanMode, is_tool_allowed_in_plan_mode};
pub use planner::Planner;
pub use reactor::ReActLoop;
