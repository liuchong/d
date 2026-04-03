//! ReAct (Reasoning + Acting) loop implementation

use llm::{AiClient, Message, Tool, ToolCall, ToolResult};
use std::sync::Arc;

/// ReAct loop state
#[derive(Debug, Clone)]
pub enum ReActState {
    /// Thinking about what to do
    Thinking,
    /// Ready to execute an action
    Action(ToolCall),
    /// Observing the result
    Observing(ToolResult),
    /// Task completed
    Done(String),
    /// Error occurred
    Error(String),
}

/// The ReAct loop manages the reasoning-action cycle
pub struct ReActLoop {
    client: Arc<AiClient>,
    max_iterations: usize,
}

impl ReActLoop {
    pub fn new(client: Arc<AiClient>) -> Self {
        Self {
            client,
            max_iterations: 10,
        }
    }

    pub fn with_max_iterations(mut self, n: usize) -> Self {
        self.max_iterations = n;
        self
    }

    /// Run the ReAct loop for a given goal
    pub async fn run(
        &self,
        goal: &str,
        tools: &[Tool],
        execute_tool: impl Fn(&ToolCall) -> ToolResult,
    ) -> anyhow::Result<String> {
        let system_prompt = format!(
            "You are a helpful assistant that can use tools to accomplish tasks.\n\n\
             Available tools:\n{}\n\n\
             Think step by step. When you need to use a tool, respond with:\n\
             ACTION: {{\"tool\": \"tool_name\", \"arguments\": {{...}}}}\n\n\
             After observing the result, continue thinking until the task is complete.",
            serde_json::to_string_pretty(tools).unwrap_or_default()
        );

        let mut messages = vec![
            Message::system(&system_prompt),
            Message::user(&format!("Task: {}", goal)),
        ];

        for _ in 0..self.max_iterations {
            // Get assistant response
            let chat_messages: Vec<llm::client::ChatMessage> = messages.iter().map(|m| {
                llm::client::ChatMessage {
                    role: format!("{:?}", m.role).to_lowercase(),
                    content: m.content.clone(),
                }
            }).collect();
            let response = self.client.chat(chat_messages).await.map_err(|e| anyhow::anyhow!("{}", e))?;

            // Check if done
            if !response.contains("ACTION:") {
                return Ok(response);
            }

            // Parse and execute tool call
            if let Some(tool_call) = self.parse_action(&response) {
                messages.push(Message::assistant(&response));

                // Execute tool
                let result = execute_tool(&tool_call);
                messages.push(Message::user(&format!(
                    "OBSERVATION: {}",
                    serde_json::to_string(&result).unwrap_or_default()
                )));
            } else {
                messages.push(Message::assistant(&response));
            }
        }

        Err(anyhow::anyhow!("Max iterations reached"))
    }

    fn parse_action(&self, response: &str) -> Option<ToolCall> {
        // Simple parsing: extract ACTION: {...}
        if let Some(start) = response.find("ACTION:") {
            let json_start = response[start..].find('{')? + start;
            let json_str = &response[json_start..];
            // Find matching brace
            let mut depth = 0;
            let mut end = json_start;
            for (i, c) in json_str.chars().enumerate() {
                match c {
                    '{' => depth += 1,
                    '}' => {
                        depth -= 1;
                        if depth == 0 {
                            end = json_start + i + 1;
                            break;
                        }
                    }
                    _ => {}
                }
            }
            let json = &response[json_start..end];
            serde_json::from_str(json).ok()
        } else {
            None
        }
    }
}
