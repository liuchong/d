use crate::approval::{ApprovalDecision, ApprovalRequest};

/// Audit log trait for recording security decisions
pub trait AuditLog: Send + Sync {
    fn log_decision(&self, request: &ApprovalRequest, decision: ApprovalDecision);
    fn log_user_response(&self, request: &ApprovalRequest, approved: bool);
    fn log_execution(&self, request: &ApprovalRequest, result: &str);
}

/// No-op audit log for testing
pub struct NoOpAuditLog;

impl AuditLog for NoOpAuditLog {
    fn log_decision(&self, _request: &ApprovalRequest, _decision: ApprovalDecision) {}
    fn log_user_response(&self, _request: &ApprovalRequest, _approved: bool) {}
    fn log_execution(&self, _request: &ApprovalRequest, _result: &str) {}
}

/// File-based audit log
pub struct FileAuditLog {
    path: std::path::PathBuf,
}

impl FileAuditLog {
    pub fn new(path: std::path::PathBuf) -> anyhow::Result<Self> {
        std::fs::create_dir_all(path.parent().unwrap_or(&path))?;
        Ok(Self { path })
    }
}

impl AuditLog for FileAuditLog {
    fn log_decision(&self, request: &ApprovalRequest, decision: ApprovalDecision) {
        let entry = format!(
            "[{}] DECISION: {:?} for {}\n",
            request.timestamp, decision, &request.tool_call.function.name
        );
        let _ = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
            .and_then(|mut f| { use std::io::Write; f.write_all(entry.as_bytes()) });
    }

    fn log_user_response(&self, request: &ApprovalRequest, approved: bool) {
        let entry = format!(
            "[{}] USER: {} for {}\n",
            request.timestamp,
            if approved { "APPROVED" } else { "DENIED" },
            &request.tool_call.function.name
        );
        let _ = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
            .and_then(|mut f| { use std::io::Write; f.write_all(entry.as_bytes()) });
    }

    fn log_execution(&self, request: &ApprovalRequest, result: &str) {
        let entry = format!(
            "[{}] EXEC: {} = {}\n",
            request.timestamp, &request.tool_call.function.name, result
        );
        let _ = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
            .and_then(|mut f| { use std::io::Write; f.write_all(entry.as_bytes()) });
    }
}
