//! Plan mode for read-only operations
//!
//! Plan mode allows the agent to explore and plan without making changes.
//! Only read-only tools are allowed in plan mode.

use std::path::PathBuf;
use tokio::fs;

/// Plan mode manager
#[derive(Debug, Clone)]
pub struct PlanMode {
    enabled: bool,
    plan_file: Option<PathBuf>,
}

impl Default for PlanMode {
    fn default() -> Self {
        Self::new()
    }
}

impl PlanMode {
    /// Create a new plan mode manager (disabled by default)
    pub fn new() -> Self {
        Self {
            enabled: false,
            plan_file: None,
        }
    }

    /// Enable plan mode
    pub fn enable(&mut self) {
        if self.enabled {
            return;
        }

        self.enabled = true;

        // Set plan file path
        let session_id = format!("plan_{}", chrono::Utc::now().timestamp());
        self.plan_file = Some(std::env::temp_dir().join(format!("d_plan_{}.md", session_id)));
    }

    /// Disable plan mode
    pub fn disable(&mut self) {
        if !self.enabled {
            return;
        }

        self.enabled = false;
        self.plan_file = None;
    }

    /// Toggle plan mode
    pub fn toggle(&mut self) {
        if self.enabled {
            self.disable();
        } else {
            self.enable();
        }
    }

    /// Check if plan mode is enabled
    pub fn is_enabled(&self) -> bool {
        self.enabled
    }

    /// Get plan file path
    pub fn plan_file(&self) -> Option<&PathBuf> {
        self.plan_file.as_ref()
    }

    /// Read current plan content
    pub async fn read_plan(&self) -> Option<String> {
        let path = self.plan_file.as_ref()?;
        match fs::read_to_string(path).await {
            Ok(content) => Some(content),
            Err(_) => None,
        }
    }

    /// Write plan content
    pub async fn write_plan(&self, content: &str) -> anyhow::Result<()> {
        let path = match &self.plan_file {
            Some(p) => p,
            None => anyhow::bail!("Plan mode not enabled"),
        };

        fs::write(path, content).await?;
        Ok(())
    }

    /// Append to plan content
    pub async fn append_plan(&self, content: &str) -> anyhow::Result<()> {
        let path = match &self.plan_file {
            Some(p) => p,
            None => anyhow::bail!("Plan mode not enabled"),
        };

        let existing = fs::read_to_string(path).await.unwrap_or_default();
        fs::write(path, format!("{}\n{}", existing, content)).await?;
        Ok(())
    }
}

/// Check if a tool is allowed in plan mode
pub fn is_tool_allowed_in_plan_mode(tool_name: &str) -> bool {
    // Only read-only tools are allowed in plan mode
    let allowed_tools = [
        "read_file",
        "list_directory",
        "glob",
        "grep",
        "web_search",
        "fetch_url",
    ];

    allowed_tools.contains(&tool_name)
}

/// Get the list of allowed tools in plan mode
pub fn allowed_plan_mode_tools() -> &'static [&'static str] {
    &[
        "read_file",
        "list_directory",
        "glob",
        "grep",
        "web_search",
        "fetch_url",
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_plan_mode_toggle() {
        let mut plan = PlanMode::new();
        assert!(!plan.is_enabled());

        plan.enable();
        assert!(plan.is_enabled());
        assert!(plan.plan_file().is_some());

        plan.disable();
        assert!(!plan.is_enabled());
        assert!(plan.plan_file().is_none());
    }

    #[test]
    fn test_allowed_tools() {
        assert!(is_tool_allowed_in_plan_mode("read_file"));
        assert!(is_tool_allowed_in_plan_mode("list_directory"));
        assert!(is_tool_allowed_in_plan_mode("web_search"));
        assert!(!is_tool_allowed_in_plan_mode("write_file"));
        assert!(!is_tool_allowed_in_plan_mode("str_replace"));
        assert!(!is_tool_allowed_in_plan_mode("shell"));
    }
}
