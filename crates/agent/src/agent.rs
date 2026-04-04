use crate::plan::{PlanMode, is_tool_allowed_in_plan_mode};
use llm::{AiClient, ChatResponse, Message, Tool, ToolCall, ToolResult};
use session::SessionManager;
use memory::MemoryStore;
use security::{ApprovalSystem, ApprovalRequest, ApprovalDecision};
use std::sync::Arc;
use tools::{default_registry, ToolContext};
use tracing::{debug, info};

/// The main agent struct that orchestrates AI interactions
#[derive(Clone)]
pub struct Agent {
    pub client: AiClient,
    pub session_manager: Arc<SessionManager>,
    pub memory: Option<Arc<dyn MemoryStore>>,
    pub approval: Arc<ApprovalSystem>,
    pub tools: Vec<Tool>,
    pub plan_mode: PlanMode,
    max_iterations: usize,
    yolo_mode: bool,
}

impl Agent {
    pub fn new(
        client: AiClient,
        session_manager: Arc<SessionManager>,
        approval: Arc<ApprovalSystem>,
    ) -> Self {
        Self {
            client,
            session_manager,
            memory: None,
            approval,
            tools: Vec::new(),
            plan_mode: PlanMode::new(),
            max_iterations: 10,
            yolo_mode: false,
        }
    }

    pub fn with_memory(mut self, memory: Arc<dyn MemoryStore>) -> Self {
        self.memory = Some(memory);
        self
    }

    pub fn with_tools(mut self, tools: Vec<Tool>) -> Self {
        self.tools = tools;
        self
    }

    pub fn with_max_iterations(mut self, n: usize) -> Self {
        self.max_iterations = n;
        self
    }

    pub fn with_yolo(mut self, enabled: bool) -> Self {
        self.yolo_mode = enabled;
        self
    }

    pub fn with_plan_mode(mut self, enabled: bool) -> Self {
        if enabled {
            self.plan_mode.enable();
        } else {
            self.plan_mode.disable();
        }
        self
    }

    pub fn toggle_plan_mode(&mut self) {
        self.plan_mode.toggle();
    }

    pub fn is_plan_mode_enabled(&self) -> bool {
        self.plan_mode.is_enabled()
    }

    /// Convert internal Message to LLM ChatMessage format
    fn to_chat_message(msg: &Message) -> llm::client::ChatMessage {
        let role = format!("{:?}", msg.role).to_lowercase();
        let tool_calls = msg.tool_calls.as_ref().map(|tc| {
            tc.iter().map(|t| serde_json::to_value(t).unwrap()).collect()
        });
        
        // For assistant messages, ensure reasoning_content is set when tools are used
        // kimi-for-coding API requires this field when thinking is enabled
        let reasoning_content = if role == "assistant" {
            // If there are tool calls, reasoning_content must be present with non-empty content
            if tool_calls.is_some() {
                Some(msg.reasoning_content.clone().unwrap_or_else(|| "Let me help you with that".to_string()))
            } else {
                msg.reasoning_content.clone()
            }
        } else {
            None // Non-assistant roles don't need reasoning_content
        };
        
        llm::client::ChatMessage {
            role,
            content: msg.content.clone(),
            reasoning_content,
            tool_calls,
            tool_call_id: msg.tool_call_id.clone(),
            name: msg.name.clone(),
        }
    }

    /// Run a single turn conversation with tool support
    pub async fn chat(&self, session_id: &str, user_input: &str) -> anyhow::Result<String> {
        // Get or create session
        let session = self.session_manager.get_or_create(session_id);
        
        // Clone messages for LLM call
        let mut messages = session.messages.clone();
        
        // Add user message
        messages.push(Message::user(user_input));
        
        // Retrieve relevant memories if available
        if let Some(ref memory) = self.memory {
            // TODO: Query memories and inject into context
            let _ = memory;
        }
        
        // Convert to ChatMessage
        let chat_messages: Vec<llm::client::ChatMessage> = messages.iter()
            .map(Self::to_chat_message)
            .collect();
        
        // Call LLM with tools
        let response = self.client.chat_with_tools(chat_messages, &self.tools).await
            .map_err(|e| anyhow::anyhow!("{}", e))?;
        
        // Handle tool calls if present
        let final_response = if response.has_tool_calls() {
            self.handle_tool_calls(session_id, &messages, response).await?
        } else {
            response.content.unwrap_or_default()
        };
        
        // Update session with new messages
        self.session_manager.update(session_id, |s| {
            s.add_message(Message::user(user_input));
            s.add_message(Message::assistant(&final_response));
        })?;
        
        // Save session
        let updated = self.session_manager.get(session_id).unwrap();
        self.session_manager.save(&updated)?;
        
        Ok(final_response)
    }

    /// Handle tool calls from LLM response
    async fn handle_tool_calls(
        &self,
        session_id: &str,
        messages: &[Message],
        initial_response: ChatResponse,
    ) -> anyhow::Result<String> {
        let mut current_messages = messages.to_vec();
        
        // Add assistant message with tool calls
        // reasoning_content will be set in to_chat_message for kimi-for-coding API compatibility
        let tool_calls = initial_response.tool_calls.clone();
        current_messages.push(Message::assistant_with_tool_calls(
            initial_response.content.as_deref(),
            None, // Will be set to default in to_chat_message if needed
            &tool_calls,
        ));
        
        // Execute each tool call and collect results
        for tool_call in &tool_calls {
            info!("Tool call: {} with args {}", tool_call.function.name, tool_call.function.arguments);
            
            // Check approval
            let approval_request = ApprovalRequest {
                tool_call: tool_call.clone(),
                session_id: session_id.to_string(),
                timestamp: chrono::Utc::now(),
            };
            
            let decision = if self.yolo_mode {
                ApprovalDecision::Approve
            } else {
                self.approval.check(&approval_request)
            };
            
            // Check plan mode - block non-read-only tools
            let result = if self.plan_mode.is_enabled() && !is_tool_allowed_in_plan_mode(&tool_call.function.name) {
                ToolResult::new(
                    &tool_call.id,
                    &tool_call.function.name,
                    format!("Tool '{}' is not allowed in plan mode. Only read-only operations are permitted.", 
                        tool_call.function.name)
                )
            } else {
                match decision {
                    ApprovalDecision::Approve => {
                        self.execute_tool(tool_call).await
                    }
                    ApprovalDecision::Deny => {
                        ToolResult::new(&tool_call.id, &tool_call.function.name, "Tool execution rejected by policy")
                    }
                    ApprovalDecision::Ask => {
                        // For now, auto-approve in Ask state (TODO: prompt user)
                        self.execute_tool(tool_call).await
                    }
                }
            };
            
            // Add tool result to messages with proper tool_call_id
            current_messages.push(Message::tool_result(
                &tool_call.id,
                &tool_call.function.name,
                &result.content
            ));
        }
        
        // Get final response from LLM with tool results
        let chat_messages: Vec<llm::client::ChatMessage> = current_messages.iter()
            .map(Self::to_chat_message)
            .collect();
        
        let final_response = self.client.chat_with_tools(chat_messages, &self.tools).await
            .map_err(|e| anyhow::anyhow!("{}", e))?;
        
        Ok(final_response.content.unwrap_or_default())
    }

    /// Execute a single tool
    async fn execute_tool(&self, tool_call: &ToolCall) -> ToolResult {
        let registry = default_registry();
        
        let tool = match registry.get(&tool_call.function.name) {
            Some(t) => t,
            None => {
                return ToolResult::new(
                    &tool_call.id,
                    &tool_call.function.name,
                    format!("Unknown tool: {}", tool_call.function.name)
                );
            }
        };
        
        // Parse arguments
        let args: serde_json::Value = match serde_json::from_str(&tool_call.function.arguments) {
            Ok(a) => a,
            Err(e) => {
                return ToolResult::new(
                    &tool_call.id,
                    &tool_call.function.name,
                    format!("Invalid arguments: {}", e)
                );
            }
        };
        
        // Execute tool
        let ctx = ToolContext::default();
        let output = tool.execute(args, &ctx).await;
        
        ToolResult::new(
            &tool_call.id,
            &tool_call.function.name,
            output.to_string()
        )
    }

    /// Run agent loop with tool calling for complex tasks
    pub async fn run_with_tools(&self, goal: &str) -> anyhow::Result<String> {
        let system_prompt = format!(
            "You are a helpful assistant that can use tools to accomplish tasks.\
             When you need to use a tool, the system will execute it for you.\
             Continue until the task is complete."
        );

        let mut messages = vec![
            llm::client::ChatMessage::system(system_prompt),
            llm::client::ChatMessage::user(goal.to_string()),
        ];

        for iteration in 0..self.max_iterations {
            debug!("ReAct iteration {}", iteration + 1);
            
            let response = self.client.chat_with_tools(messages.clone(), &self.tools).await
                .map_err(|e| anyhow::anyhow!("{}", e))?;
            
            // If no tool calls, we're done
            if !response.has_tool_calls() {
                return Ok(response.content.unwrap_or_default());
            }
            
            // Add assistant message with tool calls
            let tool_calls_json: Vec<serde_json::Value> = response.tool_calls.iter()
                .map(|tc| serde_json::json!({
                    "id": tc.id,
                    "type": tc.call_type,
                    "function": {
                        "name": tc.function.name,
                        "arguments": tc.function.arguments
                    }
                }))
                .collect();
            messages.push(llm::client::ChatMessage::assistant(response.content.unwrap_or_default())
                .with_tool_calls(tool_calls_json));
            
            // Execute tool calls
            for tool_call in &response.tool_calls {
                info!("Executing tool: {}", tool_call.function.name);
                let result = self.execute_tool(tool_call).await;
                
                // Add tool result
                messages.push(llm::client::ChatMessage::tool(
                    &tool_call.id,
                    result.content
                ));
            }
        }
        
        Err(anyhow::anyhow!("Max iterations ({}) reached", self.max_iterations))
    }
}
