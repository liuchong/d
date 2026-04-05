//! Git tool for repository operations
//!
//! Provides read-only git operations for inspecting repositories.

use crate::{Tool, ToolContext, ToolResult};
use serde_json::Value;
use std::process::Command;

/// Git tool for repository inspection
pub struct GitTool;

impl Tool for GitTool {
    fn name(&self) -> &str {
        "git"
    }

    fn description(&self) -> &str {
        "Execute git commands for repository inspection. \
         Supports: status, log, diff, show, branch, remote. \
         Only read-only operations are allowed."
    }

    fn parameters(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "command": {
                    "type": "string",
                    "description": "Git subcommand (status, log, diff, show, branch, remote)",
                    "enum": ["status", "log", "diff", "show", "branch", "remote", "ls-files"]
                },
                "args": {
                    "type": "string",
                    "description": "Additional arguments for the git command"
                },
                "path": {
                    "type": "string",
                    "description": "Path to the git repository (default: current directory)"
                }
            },
            "required": ["command"]
        })
    }

    fn execute<'a>(
        &'a self,
        args: Value,
        _ctx: &'a ToolContext,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ToolResult> + Send + 'a>> {
        Box::pin(async move {
            let command = args["command"].as_str().unwrap_or("");
            let extra_args = args["args"].as_str().unwrap_or("");
            let path = args["path"].as_str().unwrap_or(".");

            if command.is_empty() {
                return ToolResult::error("No git command specified");
            }

            // Security: Only allow read-only commands
            let allowed_commands = ["status", "log", "diff", "show", "branch", "remote", "ls-files"];
            if !allowed_commands.contains(&command) {
                return ToolResult::error(format!(
                    "Git command '{}' is not allowed. Only read-only operations are permitted.",
                    command
                ));
            }

            // Build git command
            let mut cmd = Command::new("git");
            cmd.arg("-C").arg(path);
            
            // Add the subcommand
            cmd.arg(command);

            // Add extra arguments
            if !extra_args.is_empty() {
                for arg in extra_args.split_whitespace() {
                    cmd.arg(arg);
                }
            }

            // Set some sensible defaults for certain commands
            match command {
                "log" => {
                    if extra_args.is_empty() {
                        cmd.arg("--oneline").arg("-20");
                    }
                }
                "status" => {
                    if extra_args.is_empty() {
                        cmd.arg("--short");
                    }
                }
                "diff" => {
                    if extra_args.is_empty() {
                        cmd.arg("--stat");
                    }
                }
                _ => {}
            }

            // Execute
            let output = match cmd.output() {
                Ok(o) => o,
                Err(e) => {
                    return ToolResult::error(format!("Failed to execute git: {}", e));
                }
            };

            let stdout = String::from_utf8_lossy(&output.stdout);
            let stderr = String::from_utf8_lossy(&output.stderr);

            if !output.status.success() {
                return ToolResult::error(format!(
                    "Git command failed: {}",
                    if stderr.is_empty() { "Unknown error" } else { &stderr }
                ));
            }

            let result = if stdout.is_empty() {
                if stderr.is_empty() {
                    "Command executed successfully (no output)".to_string()
                } else {
                    stderr.to_string()
                }
            } else {
                stdout.to_string()
            };

            ToolResult::success(result)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_git_status() {
        let tool = GitTool;
        let ctx = ToolContext::default();
        let args = serde_json::json!({
            "command": "status",
            "path": "."
        });

        let result = tool.execute(args, &ctx).await;
        // Should succeed in a git repo
        assert!(matches!(result, ToolResult::Success(_)));
    }

    #[tokio::test]
    async fn test_git_log() {
        let tool = GitTool;
        let ctx = ToolContext::default();
        let args = serde_json::json!({
            "command": "log",
            "args": "-5",
            "path": "."
        });

        let result = tool.execute(args, &ctx).await;
        assert!(matches!(result, ToolResult::Success(_)));
    }

    #[tokio::test]
    async fn test_git_disallowed_command() {
        let tool = GitTool;
        let ctx = ToolContext::default();
        let args = serde_json::json!({
            "command": "commit",
            "path": "."
        });

        let result = tool.execute(args, &ctx).await;
        assert!(matches!(result, ToolResult::Error(_)));
        let ToolResult::Error(msg) = result else { unreachable!() };
        assert!(msg.contains("not allowed"));
    }

    #[test]
    fn test_git_tool_metadata() {
        let tool = GitTool;
        assert_eq!(tool.name(), "git");
        assert!(!tool.description().is_empty());
    }
}
