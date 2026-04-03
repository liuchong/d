use crate::approval::{ApprovalDecision, ApprovalRequest};

/// Security policy for tool execution
#[derive(Debug, Clone)]
pub struct Policy {
    /// Tools that can always execute without approval
    pub allowed_tools: Vec<String>,
    /// Tools that are always blocked
    pub blocked_tools: Vec<String>,
    /// Require approval for destructive operations
    pub confirm_destructive: bool,
}

impl Policy {
    pub fn evaluate(&self, request: &ApprovalRequest) -> ApprovalDecision {
        let tool = &request.tool_call.function.name;

        // Check blocked list first
        if self.blocked_tools.contains(tool) {
            return ApprovalDecision::Deny;
        }

        // Check allowed list
        if self.allowed_tools.contains(tool) {
            return ApprovalDecision::Approve;
        }

        // Require confirmation for destructive operations
        if self.confirm_destructive && is_destructive(tool) {
            return ApprovalDecision::Ask;
        }

        // Default: ask for approval
        ApprovalDecision::Ask
    }
}

impl Default for Policy {
    fn default() -> Self {
        Self {
            allowed_tools: vec![
                "read_file".to_string(),
                "list_directory".to_string(),
                "search".to_string(),
            ],
            blocked_tools: vec![],
            confirm_destructive: true,
        }
    }
}

fn is_destructive(tool: &str) -> bool {
    matches!(
        tool,
        "write_file" | "delete_file" | "execute_shell" | "execute_command"
    )
}
