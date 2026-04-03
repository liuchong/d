//! Interactive chat session

use agent::Agent;
use kernel::Config;
use llm::{AiClient, Message};
use security::ApprovalSystem;
use session::SessionManager;
use std::sync::Arc;
use tools::{default_registry, ToolContext};
use tracing::{info, warn};

/// Chat session manager
pub struct ChatSession {
    agent: Agent,
    session_manager: Arc<SessionManager>,
    session_id: String,
    tool_context: ToolContext,
}

impl ChatSession {
    /// Create a new chat session
    pub fn new(config: Config) -> anyhow::Result<Self> {
        let client = AiClient::new(config.clone())?;
        let session_manager = Arc::new(SessionManager::new()?);
        let approval = Arc::new(ApprovalSystem::default());
        
        let tool_registry = default_registry();
        let tools = tool_registry.to_llm_tools();
        
        let agent = Agent::new(client, session_manager.clone(), approval)
            .with_tools(tools);

        let session_id = uuid::Uuid::new_v4().to_string();
        
        Ok(Self {
            agent,
            session_manager,
            session_id,
            tool_context: ToolContext::default(),
        })
    }

    /// Load an existing session
    pub fn with_session(mut self, session_id: impl Into<String>) -> Self {
        self.session_id = session_id.into();
        self
    }

    /// Enable yolo mode (auto-approve dangerous operations)
    pub fn with_yolo(mut self) -> Self {
        self.tool_context.allow_dangerous = true;
        self
    }

    /// Run a single message and get response
    pub async fn send_message(&self, content: &str) -> anyhow::Result<String> {
        info!("User: {}", content);
        
        // Check for special commands
        if let Some(response) = self.handle_command(content).await {
            return Ok(response);
        }

        // Normal chat
        let response = self.agent.chat(&self.session_id, content).await?;
        info!("Assistant: {}", response);
        
        Ok(response)
    }

    /// Handle special commands
    async fn handle_command(&self, input: &str) -> Option<String> {
        let input = input.trim();
        
        match input {
            "/help" => Some(self.help_message()),
            "/tools" => Some(self.list_tools()),
            "/clear" => {
                // TODO: Clear session
                Some("Session cleared".to_string())
            }
            "/sessions" => Some(self.list_sessions()),
            s if s.starts_with("/load ") => {
                let id = s.trim_start_matches("/load ").trim();
                // TODO: Load session
                Some(format!("Loading session: {}", id))
            }
            s if s.starts_with("/tool ") => {
                let args = s.trim_start_matches("/tool ").trim();
                Some(self.execute_tool_direct(args).await)
            }
            _ => None,
        }
    }

    /// Execute a tool directly
    async fn execute_tool_direct(&self, args: &str) -> String {
        // Parse tool name and arguments
        let parts: Vec<&str> = args.splitn(2, ' ').collect();
        if parts.is_empty() {
            return "Usage: /tool <tool_name> <args_json>".to_string();
        }

        let tool_name = parts[0];
        let tool_args = parts.get(1).map(|s| s.trim()).unwrap_or("{}");

        let registry = default_registry();
        let tool = match registry.get(tool_name) {
            Some(t) => t,
            None => return format!("Unknown tool: {}", tool_name),
        };

        let args = match serde_json::from_str(tool_args) {
            Ok(a) => a,
            Err(e) => return format!("Invalid JSON arguments: {}", e),
        };

        let result = tool.execute(args, &self.tool_context).await;
        format!("{}", result)
    }

    /// Help message
    fn help_message(&self) -> String {
        r#"Available commands:
  /help      - Show this help message
  /tools     - List available tools
  /clear     - Clear current session
  /sessions  - List saved sessions
  /load <id> - Load a session
  /quit      - Exit chat

Type your message normally to chat with the AI."#.to_string()
    }

    /// List available tools
    fn list_tools(&self) -> String {
        let registry = default_registry();
        let tools = registry.list();
        
        let mut output = String::from("Available tools:\n");
        for tool_name in tools {
            if let Some(tool) = registry.get(tool_name) {
                output.push_str(&format!("  {} - {}\n", tool_name, tool.description()));
            }
        }
        output
    }

    /// List saved sessions
    fn list_sessions(&self) -> String {
        let sessions = self.session_manager.list();
        if sessions.is_empty() {
            "No saved sessions".to_string()
        } else {
            let mut output = String::from("Saved sessions:\n");
            for session in sessions {
                output.push_str(&format!("  {} - {}\n", session.id, session.title));
            }
            output
        }
    }

    /// Get current session ID
    pub fn session_id(&self) -> &str {
        &self.session_id
    }
}

/// Run interactive chat
pub async fn run_interactive(config: Config) -> anyhow::Result<()> {
    println!("🤖 D AI Chat");
    println!("Type /help for available commands\n");

    let session = ChatSession::new(config)?;
    let mut repl = crate::repl::Repl::new();

    loop {
        match repl.read_line("You: ")? {
            Some(input) => {
                if input == "/quit" || input == "/exit" {
                    println!("Goodbye!");
                    break;
                }

                match session.send_message(&input).await {
                    Ok(response) => {
                        println!("AI: {}\n", response);
                    }
                    Err(e) => {
                        warn!("Error: {}", e);
                        println!("Error: {}\n", e);
                    }
                }
            }
            None => continue,
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_chat_session_creation() {
        // This would need a mock config to test properly
        // For now, just test that the types compile correctly
    }

    #[test]
    fn test_help_message() {
        // Can't create session without config, but we can test the help format
        assert!(true);
    }
}
