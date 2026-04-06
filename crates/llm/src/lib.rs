pub mod client;
pub mod message;
pub mod provider;
pub mod tool;

#[cfg(test)]
mod message_test;
#[cfg(test)]
mod tool_test;

pub use client::{AiClient, ChatResponse, CodingAgent, TokenUsage};
pub use message::{Message, MessageRole};
pub use provider::{
    CompletionRequest, CompletionResponse, Message as ProviderMessage, Provider,
    ProviderCapabilities, ProviderConfig, ProviderRegistry, ProviderType, Role, StreamChunk, Usage,
    MoonshotProvider, OpenAiProvider, OllamaProvider,
};
pub use tool::{Tool, ToolCall, ToolResult};
