//! LLM Provider abstraction layer
//!
//! Provides:
//! - Provider trait for different LLM backends
//! - Provider registry for dynamic selection
//! - Streaming response support
//! - Unified error handling

use async_trait::async_trait;

use serde::{Deserialize, Serialize};



pub mod moonshot;
pub mod ollama;
pub mod openai;
pub mod registry;

pub use moonshot::MoonshotProvider;
pub use ollama::OllamaProvider;
pub use openai::OpenAiProvider;
pub use registry::ProviderRegistry;

/// Provider type enum
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ProviderType {
    /// Moonshot AI (Kimi)
    Moonshot,
    /// Ollama local models
    Ollama,
    /// OpenAI
    OpenAi,
    /// Anthropic Claude
    Anthropic,
    /// Google Gemini
    Google,
    /// Custom endpoint
    Custom,
}

impl std::fmt::Display for ProviderType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ProviderType::Moonshot => write!(f, "moonshot"),
            ProviderType::Ollama => write!(f, "ollama"),
            ProviderType::OpenAi => write!(f, "openai"),
            ProviderType::Anthropic => write!(f, "anthropic"),
            ProviderType::Google => write!(f, "google"),
            ProviderType::Custom => write!(f, "custom"),
        }
    }
}

/// Message role
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    System,
    User,
    Assistant,
    Tool,
}

impl std::fmt::Display for Role {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Role::System => write!(f, "system"),
            Role::User => write!(f, "user"),
            Role::Assistant => write!(f, "assistant"),
            Role::Tool => write!(f, "tool"),
        }
    }
}

/// Chat message
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub role: Role,
    pub content: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning_content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<serde_json::Value>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

impl Message {
    /// Create system message
    pub fn system(content: impl Into<String>) -> Self {
        Self {
            role: Role::System,
            content: content.into(),
            reasoning_content: None,
            tool_calls: None,
            tool_call_id: None,
            name: None,
        }
    }

    /// Create user message
    pub fn user(content: impl Into<String>) -> Self {
        Self {
            role: Role::User,
            content: content.into(),
            reasoning_content: None,
            tool_calls: None,
            tool_call_id: None,
            name: None,
        }
    }

    /// Create assistant message
    pub fn assistant(content: impl Into<String>) -> Self {
        Self {
            role: Role::Assistant,
            content: content.into(),
            reasoning_content: None,
            tool_calls: None,
            tool_call_id: None,
            name: None,
        }
    }

    /// Create tool message
    pub fn tool(tool_call_id: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            role: Role::Tool,
            content: content.into(),
            reasoning_content: None,
            tool_calls: None,
            tool_call_id: Some(tool_call_id.into()),
            name: None,
        }
    }

    /// Create assistant message with reasoning
    pub fn assistant_with_reasoning(
        content: impl Into<String>,
        reasoning: impl Into<String>,
    ) -> Self {
        Self {
            role: Role::Assistant,
            content: content.into(),
            reasoning_content: Some(reasoning.into()),
            tool_calls: None,
            tool_call_id: None,
            name: None,
        }
    }
}

/// Completion request
#[derive(Debug, Clone, Serialize)]
pub struct CompletionRequest {
    pub model: String,
    pub messages: Vec<Message>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub top_p: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stream: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<serde_json::Value>>,
}

impl CompletionRequest {
    /// Create new request
    pub fn new(model: impl Into<String>, messages: Vec<Message>) -> Self {
        Self {
            model: model.into(),
            messages,
            temperature: None,
            max_tokens: None,
            top_p: None,
            stream: None,
            tools: None,
        }
    }

    /// Set temperature
    pub fn temperature(mut self, temp: f32) -> Self {
        self.temperature = Some(temp);
        self
    }

    /// Set max tokens
    pub fn max_tokens(mut self, tokens: u32) -> Self {
        self.max_tokens = Some(tokens);
        self
    }

    /// Enable streaming
    pub fn stream(mut self) -> Self {
        self.stream = Some(true);
        self
    }

    /// Set tools
    pub fn tools(mut self, tools: Vec<serde_json::Value>) -> Self {
        self.tools = Some(tools);
        self
    }
}

/// Token usage
#[derive(Debug, Clone, Copy, Default, Deserialize)]
pub struct Usage {
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
    pub total_tokens: u32,
}

/// Completion response
#[derive(Debug, Clone, Deserialize)]
pub struct CompletionResponse {
    pub content: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning_content: Option<String>,
    pub finish_reason: String,
    #[serde(default)]
    pub usage: Option<Usage>,
}

/// Streaming chunk
#[derive(Debug, Clone, Deserialize)]
pub struct StreamChunk {
    pub content: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning_content: Option<String>,
    pub is_finished: bool,
}

/// Provider capabilities
#[derive(Debug, Clone, Copy, Default)]
pub struct ProviderCapabilities {
    pub streaming: bool,
    pub function_calling: bool,
    pub reasoning: bool,
    pub vision: bool,
}

/// Provider configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderConfig {
    pub provider_type: ProviderType,
    pub api_key: Option<String>,
    pub base_url: Option<String>,
    pub default_model: Option<String>,
    pub timeout_secs: u64,
}

impl ProviderConfig {
    /// Create config for provider type
    pub fn new(provider_type: ProviderType) -> Self {
        Self {
            provider_type,
            api_key: None,
            base_url: None,
            default_model: None,
            timeout_secs: 60,
        }
    }

    /// Set API key
    pub fn with_api_key(mut self, key: impl Into<String>) -> Self {
        self.api_key = Some(key.into());
        self
    }

    /// Set base URL
    pub fn with_base_url(mut self, url: impl Into<String>) -> Self {
        self.base_url = Some(url.into());
        self
    }

    /// Set default model
    pub fn with_model(mut self, model: impl Into<String>) -> Self {
        self.default_model = Some(model.into());
        self
    }
}

/// Provider trait
#[async_trait]
pub trait Provider: Send + Sync {
    /// Get provider type
    fn provider_type(&self) -> ProviderType;

    /// Get provider capabilities
    fn capabilities(&self) -> ProviderCapabilities;

    /// Get default model
    fn default_model(&self) -> &str;

    /// Complete chat
    async fn complete(&self, request: CompletionRequest) -> anyhow::Result<CompletionResponse>;

    /// Check if provider is available (has valid config)
    fn is_available(&self) -> bool;
}

/// Provider error
#[derive(Debug, thiserror::Error)]
pub enum ProviderError {
    #[error("API request failed: {0}")]
    ApiError(String),
    #[error("Invalid configuration: {0}")]
    ConfigError(String),
    #[error("Rate limited")]
    RateLimited,
    #[error("Model not found: {0}")]
    ModelNotFound(String),
    #[error("Streaming not supported")]
    StreamingNotSupported,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_message_creation() {
        let sys = Message::system("You are helpful");
        assert_eq!(sys.role, Role::System);

        let user = Message::user("Hello");
        assert_eq!(user.role, Role::User);

        let assistant = Message::assistant("Hi");
        assert_eq!(assistant.role, Role::Assistant);
    }

    #[test]
    fn test_completion_request_builder() {
        let req = CompletionRequest::new("gpt-4", vec![Message::user("Hello")])
            .temperature(0.7)
            .max_tokens(100)
            .stream();

        assert_eq!(req.model, "gpt-4");
        assert_eq!(req.temperature, Some(0.7));
        assert_eq!(req.max_tokens, Some(100));
        assert_eq!(req.stream, Some(true));
    }

    #[test]
    fn test_provider_type_display() {
        assert_eq!(ProviderType::Moonshot.to_string(), "moonshot");
        assert_eq!(ProviderType::OpenAi.to_string(), "openai");
    }
}
