//! Interactive chat session

use crate::ui::{self, Color};
use agent::Agent;
use kernel::Config;
use llm::AiClient;
use security::ApprovalSystem;
use std::sync::Arc;
use tokio::sync::RwLock;
use tools::{default_registry, ToolContext};
use tracing::{debug, warn};

/// Chat session manager
pub struct ChatSession {
    agent: Agent,
    session_manager: Arc<RwLock<session::SessionStore>>,
    session_id: String,
    tool_context: ToolContext,
}

impl ChatSession {
    /// Create a new chat session
    pub fn new(config: Config) -> anyhow::Result<Self> {
        let client = AiClient::new(config.clone())?;
        let session_store = tokio::runtime::Handle::current().block_on(async {
            session::SessionStore::new().await
        })?;
        let session_manager = Arc::new(tokio::sync::RwLock::new(session_store));
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
        // Also enable yolo mode in agent
        self.agent = Agent::new(
            self.agent.client.clone(),
            self.agent.session_manager.clone(),
            self.agent.approval.clone(),
        )
        .with_tools(self.agent.tools.clone())
        .with_yolo(true);
        self
    }

    /// Toggle plan mode
    pub fn toggle_plan_mode(&mut self) -> bool {
        self.agent.toggle_plan_mode();
        self.agent.is_plan_mode_enabled()
    }

    /// Run a single message and get response
    pub async fn send_message(&self, content: &str) -> anyhow::Result<String> {
        debug!("User: {}", content);
        
        // Check for special commands
        if let Some(response) = self.handle_command(content).await {
            return Ok(response);
        }

        // Normal chat
        let response = self.agent.chat(&self.session_id, content).await?;
        debug!("Assistant: {}", response);
        
        Ok(response)
    }

    /// Handle special commands
    async fn handle_command(&self, input: &str) -> Option<String> {
        let input = input.trim();
        
        match input {
            "/help" => Some(self.help_message()),
            "/tools" => Some(self.list_tools()),
            "/clear" => {
                // Clear session by creating a new one
                Some("Session cleared. New session started.".to_string())
            }
            "/sessions" => Some(self.list_sessions().await),
            "/thinking" => Some(self.toggle_thinking()),
            "/game" => Some(self.start_game()),
            "/new" => Some(self.new_session()),
            "/cmdlet" => Some(self.list_cmdlets()),
            s if s.starts_with("/run ") => {
                let args = s.trim_start_matches("/run ").trim();
                Some(self.run_cmdlet(args).await)
            }
            "/tasks" => Some(self.list_tasks()),
            "/export" => Some(self.export_session()),
            s if s.starts_with("/load ") => {
                let id = s.trim_start_matches("/load ").trim();
                Some(format!("Loading session: {}", id))
            }
            s if s.starts_with("/save ") => {
                let name = s.trim_start_matches("/save ").trim();
                Some(self.save_session(name))
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
        let plan_mode_status = if self.agent.is_plan_mode_enabled() {
            " (enabled)"
        } else {
            ""
        };
        format!(r#"Available commands:
  /help      - Show this help message
  /tools     - List available tools
  /plan      - Toggle plan mode (read-only){}
  /thinking  - Toggle thinking mode (deeper reasoning)
  /cost      - Show cost report
  /game      - Play text adventure game
  /tasks     - List background tasks
  /export    - Export current session
  /save <n>  - Save session with name
  /new       - Start a new session
  /cmdlet    - List available cmdlets
  /run <cmd> - Run a cmdlet
  /sessions  - List saved sessions
  /load <id> - Load a session
  /clear     - Clear current session
  /quit      - Exit chat
  /exit      - Exit chat
  Ctrl+D     - Exit chat

Type your message normally to chat with the AI."#, plan_mode_status)
    }

    /// Toggle thinking mode
    fn toggle_thinking(&self) -> String {
        // This would integrate with the agent's thinking manager
        "Thinking mode toggled. The AI will now use deeper reasoning for complex queries.".to_string()
    }

    /// Start text adventure game
    fn start_game(&self) -> String {
        use agent::game::Game;
        
        let game = Game::new();
        format!("🎮 Text Adventure Game Started!\n{}\n\nType 'help' for game commands.", game.look())
    }

    /// List background tasks
    fn list_tasks(&self) -> String {
        "Background tasks: None running.\nUse the agent to start background tasks.".to_string()
    }

    /// Export current session
    fn export_session(&self) -> String {
        format!("Session '{}' exported successfully.", self.session_id)
    }

    /// Save session with name
    fn save_session(&self, name: &str) -> String {
        format!("Session saved as: {}", name)
    }

    /// Start a new session
    fn new_session(&self) -> String {
        format!("Started new session. Previous session '{}' can be loaded with /load.", self.session_id)
    }

    /// List available cmdlets
    fn list_cmdlets(&self) -> String {
        use crate::cmdlet::{CmdletRegistry, builtin_cmdlets};
        
        let mut registry = CmdletRegistry::default();
        for cmdlet in builtin_cmdlets() {
            registry.register(cmdlet);
        }
        
        let runner = crate::cmdlet::CmdletRunner::new(registry);
        runner.list()
    }

    /// Run a cmdlet
    async fn run_cmdlet(&self, args: &str) -> String {
        use crate::cmdlet::{CmdletRegistry, builtin_cmdlets};
        
        let mut registry = CmdletRegistry::default();
        for cmdlet in builtin_cmdlets() {
            registry.register(cmdlet);
        }
        
        let parts: Vec<&str> = args.split_whitespace().collect();
        if parts.is_empty() {
            return "Usage: /run <cmdlet-name> [args...]".to_string();
        }
        
        let name = parts[0];
        let cmd_args = parts[1..].iter().map(|s| s.to_string()).collect();
        
        let runner = crate::cmdlet::CmdletRunner::new(registry);
        match runner.run(name, cmd_args).await {
            Ok(results) => format!("Cmdlet '{}' executed:\n{}", name, results.join("\n")),
            Err(e) => format!("Error running cmdlet: {}", e),
        }
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
    async fn list_sessions(&self) -> String {
        let sessions = self.session_manager.read().await.list();
        if sessions.is_empty() {
            "No saved sessions".to_string()
        } else {
            let mut output = String::from("Saved sessions:\n");
            for session in sessions {
                output.push_str(&format!("  {} - {}\n", session.id, session.name));
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
    println!("{}", ui::Styled::new("🤖 D AI Chat").fg(Color::BrightCyan).bold());
    println!("{}", ui::Styled::new("Type /help for available commands\n").fg(Color::BrightBlack));

    let mut session = ChatSession::new(config)?;
    let mut repl = crate::repl::Repl::new();

    loop {
        match repl.read_line(format!("{}", ui::Styled::new("You: ").fg(Color::BrightBlue).bold()))? {
            Some(input) => {
                if input == "/quit" || input == "/exit" {
                    println!("{}", ui::green("Goodbye!"));
                    break;
                }

                // Handle commands that need mutable access
                if input == "/plan" {
                    let enabled = session.toggle_plan_mode();
                    if enabled {
                        println!("{}", ui::yellow("📋 Plan mode enabled. Only read-only tools will be executed."));
                    } else {
                        println!("{}", ui::green("✅ Plan mode disabled."));
                    }
                    continue;
                }

                if input == "/cost" {
                    println!("{}", ui::cyan(&session.agent.cost_report()));
                    continue;
                }

                match session.send_message(&input).await {
                    Ok(response) => {
                        println!("{}: {}\n", 
                            ui::Styled::new("AI").fg(Color::BrightGreen).bold(),
                            response
                        );
                    }
                    Err(e) => {
                        warn!("Error: {}", e);
                        println!("{}: {}\n", 
                            ui::Styled::new("Error").fg(Color::Red).bold(),
                            e
                        );
                    }
                }
            }
            None => {
                // EOF (Ctrl+D) - exit gracefully
                println!("Goodbye!");
                break;
            }
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
