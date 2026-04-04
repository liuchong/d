use serde::{Deserialize, Serialize};
use crate::tool::ToolCall;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum MessageRole {
    System,
    User,
    Assistant,
    Tool,
}

impl Default for MessageRole {
    fn default() -> Self {
        MessageRole::User
    }
}

/// Tool call in message format (for serialization)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MessageToolCall {
    pub id: String,
    #[serde(rename = "type")]
    pub call_type: String,
    pub function: MessageFunction,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MessageFunction {
    pub name: String,
    pub arguments: String,
}

impl From<&ToolCall> for MessageToolCall {
    fn from(tc: &ToolCall) -> Self {
        Self {
            id: tc.id.clone(),
            call_type: tc.call_type.clone(),
            function: MessageFunction {
                name: tc.function.name.clone(),
                arguments: tc.function.arguments.clone(),
            },
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub role: MessageRole,
    pub content: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning_content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<MessageToolCall>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

impl Message {
    pub fn system(content: impl Into<String>) -> Self {
        Self {
            role: MessageRole::System,
            content: content.into(),
            reasoning_content: None,
            tool_calls: None,
            tool_call_id: None,
            name: None,
        }
    }

    pub fn user(content: impl Into<String>) -> Self {
        Self {
            role: MessageRole::User,
            content: content.into(),
            reasoning_content: None,
            tool_calls: None,
            tool_call_id: None,
            name: None,
        }
    }

    pub fn assistant(content: impl Into<String>) -> Self {
        Self {
            role: MessageRole::Assistant,
            content: content.into(),
            reasoning_content: None,
            tool_calls: None,
            tool_call_id: None,
            name: None,
        }
    }

    /// Create assistant message with tool calls and optional reasoning
    pub fn assistant_with_tool_calls(content: Option<&str>, reasoning: Option<&str>, tool_calls: &[ToolCall]) -> Self {
        Self {
            role: MessageRole::Assistant,
            content: content.unwrap_or("").to_string(),
            reasoning_content: reasoning.map(|s| s.to_string()),
            tool_calls: Some(tool_calls.iter().map(MessageToolCall::from).collect()),
            tool_call_id: None,
            name: None,
        }
    }

    /// Create tool result message
    pub fn tool_result(tool_call_id: impl Into<String>, name: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            role: MessageRole::Tool,
            content: content.into(),
            reasoning_content: None,
            tool_calls: None,
            tool_call_id: Some(tool_call_id.into()),
            name: Some(name.into()),
        }
    }

    pub fn with_tool_calls(mut self, calls: Vec<serde_json::Value>) -> Self {
        // Convert from serde_json::Value to MessageToolCall
        self.tool_calls = calls.into_iter()
            .map(|v| serde_json::from_value(v).ok())
            .flatten()
            .collect::<Vec<_>>()
            .into();
        self
    }
}
