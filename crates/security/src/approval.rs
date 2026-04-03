use crate::audit::AuditLog;
use crate::policy::Policy;
use llm::tool::ToolCall;
use std::sync::Arc;

/// Tool execution request pending approval
#[derive(Debug, Clone)]
pub struct ApprovalRequest {
    pub tool_call: ToolCall,
    pub session_id: String,
    pub timestamp: chrono::DateTime<chrono::Utc>,
}

/// Approval decision
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApprovalDecision {
    Approve,
    Deny,
    Ask, // Needs user confirmation
}

/// The approval system handles tool execution permissions
pub struct ApprovalSystem {
    policy: Policy,
    audit: Arc<dyn AuditLog>,
    auto_approve: bool,
}

impl ApprovalSystem {
    pub fn new(policy: Policy, audit: Arc<dyn AuditLog>) -> Self {
        Self {
            policy,
            audit,
            auto_approve: false,
        }
    }

    pub fn with_auto_approve(mut self, enabled: bool) -> Self {
        self.auto_approve = enabled;
        self
    }

    /// Check if a tool call requires approval
    pub fn check(&self, request: &ApprovalRequest) -> ApprovalDecision {
        // Auto-approve if enabled
        if self.auto_approve {
            return ApprovalDecision::Approve;
        }

        // Check policy
        let decision = self.policy.evaluate(request);

        // Log the decision
        self.audit.log_decision(request, decision);

        decision
    }

    /// Request user approval for a tool call
    pub async fn request_user_approval(&self, request: &ApprovalRequest) -> anyhow::Result<bool> {
        // TODO: Implement user prompt through CLI or HTTP
        // For now, deny by default
        self.audit.log_user_response(request, false);
        Ok(false)
    }
}

impl Default for ApprovalSystem {
    fn default() -> Self {
        Self {
            policy: Policy::default(),
            audit: Arc::new(crate::audit::NoOpAuditLog),
            auto_approve: false,
        }
    }
}
