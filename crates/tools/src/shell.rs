//! Shell command execution tool

use super::{Tool, ToolContext, ToolResult};
use serde_json::json;
use serde_json::Value;
use std::time::Duration;
use tokio::process::Command;
use tokio::time::timeout;

/// Shell command execution tool
pub struct ShellTool;

/// Commands that are blocked for safety
const BLOCKED_COMMANDS: &[&str] = &[
    "rm",      // Prevent accidental deletion
    "mv",      // Prevent moving important files
    "dd",      // Prevent disk destruction
    "mkfs",    // Prevent filesystem destruction
    "fdisk",   // Prevent partition manipulation
    "format",  // Prevent formatting
];

/// Maximum command execution time
const MAX_EXECUTION_TIME: Duration = Duration::from_secs(60);

impl Tool for ShellTool {
    fn name(&self) -> &str {
        "shell"
    }

    fn description(&self) -> &str {
        "Execute a shell command safely. \
         Returns stdout and stderr. \
         Dangerous commands are blocked. \
         Timeout: 60 seconds."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "command": {
                    "type": "string",
                    "description": "Shell command to execute"
                },
                "working_dir": {
                    "type": "string",
                    "description": "Working directory for the command (optional)"
                }
            },
            "required": ["command"]
        })
    }

    fn execute<'a>(
        &'a self,
        args: Value,
        ctx: &'a ToolContext,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ToolResult> + Send + 'a>> {
        Box::pin(async move {
            let command_str = args["command"].as_str().unwrap_or("").trim();
            if command_str.is_empty() {
                return ToolResult::error("Command is required");
            }

            // Parse command to check for blocked commands
            let parts: Vec<&str> = command_str.split_whitespace().collect();
            if parts.is_empty() {
                return ToolResult::error("Empty command");
            }

            let cmd_name = parts[0];
            
            // Check for blocked commands
            if BLOCKED_COMMANDS.contains(&cmd_name) {
                return ToolResult::error(format!(
                    "Command '{}' is blocked for safety. Use specific file tools instead.",
                    cmd_name
                ));
            }

            // Check for dangerous patterns
            if command_str.contains("| rm") || command_str.contains("; rm") {
                return ToolResult::error("Command contains dangerous pattern");
            }

            // Require approval for destructive operations unless in yolo mode
            let is_destructive = is_destructive_command(cmd_name, command_str);
            if is_destructive && !ctx.allow_dangerous {
                return ToolResult::error(
                    "This command requires approval. Run with 'yolo' mode or use the approval system."
                        .to_string(),
                );
            }

            // Determine working directory
            let working_dir = args["working_dir"]
                .as_str()
                .map(PathBuf::from)
                .unwrap_or_else(|| ctx.working_dir.clone());

            // Execute command
            execute_shell_command(cmd_name, &parts[1..], &working_dir).await
        })
    }
}

/// Check if a command is potentially destructive
fn is_destructive_command(cmd: &str, full_cmd: &str) -> bool {
    let destructive_cmds = [
        "git", "docker", "kubectl", "terraform", "ansible",
    ];
    
    if destructive_cmds.contains(&cmd) {
        // These are only destructive with certain subcommands
        let destructive_subcmds = ["push", "deploy", "apply", "destroy", "delete", "rm"];
        for sub in &destructive_subcmds {
            if full_cmd.contains(sub) {
                return true;
            }
        }
    }
    
    false
}

/// Execute a shell command with timeout
async fn execute_shell_command(
    cmd: &str,
    args: &[&str],
    working_dir: &std::path::Path,
) -> ToolResult {
    let result = timeout(MAX_EXECUTION_TIME, async {
        Command::new(cmd)
            .args(args)
            .current_dir(working_dir)
            .output()
            .await
    })
    .await;

    match result {
        Ok(Ok(output)) => {
            let stdout = String::from_utf8_lossy(&output.stdout);
            let stderr = String::from_utf8_lossy(&output.stderr);

            if output.status.success() {
                let mut result = stdout.to_string();
                if !stderr.is_empty() {
                    result.push_str("\n[stderr]\n");
                    result.push_str(&stderr);
                }
                ToolResult::success(result)
            } else {
                let mut error = format!("Exit code: {:?}\n", output.status.code());
                if !stdout.is_empty() {
                    error.push_str("[stdout]\n");
                    error.push_str(&stdout);
                }
                if !stderr.is_empty() {
                    error.push_str("[stderr]\n");
                    error.push_str(&stderr);
                }
                ToolResult::error(error)
            }
        }
        Ok(Err(e)) => ToolResult::error(format!("Failed to execute command: {}", e)),
        Err(_) => ToolResult::error("Command timed out after 60 seconds"),
    }
}

use std::path::PathBuf;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_destructive_command() {
        assert!(is_destructive_command("git", "git push origin main"));
        assert!(!is_destructive_command("git", "git status"));
        assert!(is_destructive_command("docker", "docker rm container"));
        assert!(!is_destructive_command("ls", "ls -la"));
    }

    #[test]
    fn test_blocked_commands() {
        assert!(BLOCKED_COMMANDS.contains(&"rm"));
        assert!(BLOCKED_COMMANDS.contains(&"mv"));
    }

    #[tokio::test]
    async fn test_shell_tool_params() {
        let tool = ShellTool;
        let ctx = ToolContext::default();
        
        // Test missing command
        let result = tool.execute(json!({}), &ctx).await;
        assert!(matches!(result, ToolResult::Error(_)));

        // Test empty command
        let result = tool.execute(json!({"command": ""}), &ctx).await;
        assert!(matches!(result, ToolResult::Error(_)));
    }

    #[tokio::test]
    async fn test_shell_tool_blocked() {
        let tool = ShellTool;
        let ctx = ToolContext::default();
        
        // Test blocked command
        let result = tool.execute(json!({"command": "rm -rf /"}), &ctx).await;
        assert!(matches!(result, ToolResult::Error(_)));
    }

    #[tokio::test]
    async fn test_shell_tool_ls() {
        let tool = ShellTool;
        let ctx = ToolContext::default();
        
        // Test simple ls command
        let result = tool.execute(json!({"command": "ls"}), &ctx).await;
        assert!(matches!(result, ToolResult::Success(_)));
    }
}
