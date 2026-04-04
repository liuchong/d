//! Tool execution system for AI Daemon
//!
//! Provides safe execution of various tools like file operations,
//! shell commands, and search functionality.

pub mod fs;
pub mod grep;
pub mod shell;
pub mod str_replace;

use serde_json::Value;
use std::collections::HashMap;
use std::sync::Arc;

/// Tool execution context
#[derive(Debug, Clone)]
pub struct ToolContext {
    pub working_dir: std::path::PathBuf,
    pub allow_dangerous: bool,
}

impl Default for ToolContext {
    fn default() -> Self {
        Self {
            working_dir: std::env::current_dir().unwrap_or_default(),
            allow_dangerous: false,
        }
    }
}

/// Tool execution result
#[derive(Debug, Clone)]
pub enum ToolResult {
    Success(String),
    Error(String),
}

impl ToolResult {
    pub fn success(content: impl Into<String>) -> Self {
        ToolResult::Success(content.into())
    }

    pub fn error(msg: impl Into<String>) -> Self {
        ToolResult::Error(msg.into())
    }
}

impl std::fmt::Display for ToolResult {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ToolResult::Success(s) => write!(f, "{}", s),
            ToolResult::Error(e) => write!(f, "Error: {}", e),
        }
    }
}

/// Tool trait for all executable tools
pub trait Tool: Send + Sync {
    /// Tool name
    fn name(&self) -> &str;

    /// Tool description for LLM
    fn description(&self) -> &str;

    /// JSON schema for parameters
    fn parameters(&self) -> Value;

    /// Execute the tool
    fn execute<'a>(
        &'a self,
        args: Value,
        ctx: &'a ToolContext,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ToolResult> + Send + 'a>>;
}

/// Tool registry
#[derive(Default)]
pub struct ToolRegistry {
    tools: HashMap<String, Arc<dyn Tool>>,
}

impl ToolRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a tool
    pub fn register(&mut self, tool: Arc<dyn Tool>) {
        self.tools.insert(tool.name().to_string(), tool);
    }

    /// Get a tool by name
    pub fn get(&self, name: &str) -> Option<Arc<dyn Tool>> {
        self.tools.get(name).cloned()
    }

    /// List all registered tools
    pub fn list(&self) -> Vec<&str> {
        self.tools.keys().map(|s| s.as_str()).collect()
    }

    /// Get tools as LLM function definitions
    pub fn to_llm_tools(&self) -> Vec<llm::Tool> {
        self.tools
            .values()
            .map(|t| {
                llm::Tool::new(t.name(), t.description())
                    .with_parameters(t.parameters())
            })
            .collect()
    }
}

/// Create default tool registry with built-in tools
pub fn default_registry() -> ToolRegistry {
    let mut registry = ToolRegistry::new();
    
    // Register file system tools
    registry.register(Arc::new(fs::ReadFileTool));
    registry.register(Arc::new(fs::WriteFileTool));
    registry.register(Arc::new(fs::ListDirectoryTool));
    registry.register(Arc::new(fs::GlobTool));
    
    // Register search tools
    registry.register(Arc::new(grep::GrepTool));
    
    // Register shell tool
    registry.register(Arc::new(shell::ShellTool));
    
    // Register str_replace tool
    registry.register(Arc::new(str_replace::StrReplaceTool));
    
    registry
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tool_registry() {
        let mut registry = ToolRegistry::new();
        assert!(registry.list().is_empty());

        registry.register(Arc::new(fs::ReadFileTool));
        assert_eq!(registry.list().len(), 1);
        assert!(registry.get("read_file").is_some());
    }

    #[test]
    fn test_tool_result() {
        let success = ToolResult::success("done");
        assert!(matches!(success, ToolResult::Success(_)));

        let error = ToolResult::error("failed");
        assert!(matches!(error, ToolResult::Error(_)));
    }

    #[test]
    fn test_default_registry() {
        let registry = default_registry();
        let tools = registry.list();
        assert!(!tools.is_empty());
        assert!(tools.contains(&"read_file"));
        assert!(tools.contains(&"shell"));
    }
}
