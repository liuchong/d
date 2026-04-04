use kernel::config::Config;
use futures::StreamExt;
use serde::{Deserialize, Serialize};

use crate::tool::{Tool, ToolCall};

/// Coding Agent User-Agents for kimi-for-coding API
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CodingAgent {
    KimiCli,
    ClaudeCode,
    RooCode,
    KiloCode,
    Cursor,
    GitHubCopilot,
}

impl CodingAgent {
    /// Get the User-Agent string for this coding agent
    /// Format must match what kimi-for-coding API expects (lowercase with patch version)
    pub fn as_str(&self) -> &'static str {
        match self {
            // Kimi-for-coding API requires lowercase format with patch version
            // Reference: https://www.kimi.com/code/docs/more/third-party-agents.html
            CodingAgent::KimiCli => "kimi-cli/1.0.0",
            CodingAgent::ClaudeCode => "claude-code/0.1.0",
            CodingAgent::RooCode => "roo-code/1.0.0",
            CodingAgent::KiloCode => "kilo-code/1.0.0",
            CodingAgent::Cursor => "cursor/1.0.0",
            CodingAgent::GitHubCopilot => "github-copilot/1.0.0",
        }
    }
}

impl Default for CodingAgent {
    fn default() -> Self {
        CodingAgent::ClaudeCode
    }
}

#[derive(Debug, Clone)]
pub struct AiClient {
    config: Config,
    client: reqwest::Client,
    user_agent: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: String,
    pub content: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "reasoning_content")]
    pub reasoning_content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<serde_json::Value>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

impl ChatMessage {
    pub fn system(content: impl Into<String>) -> Self {
        Self {
            role: "system".to_string(),
            content: content.into(),
            reasoning_content: None,
            tool_calls: None,
            tool_call_id: None,
            name: None,
        }
    }

    pub fn user(content: impl Into<String>) -> Self {
        Self {
            role: "user".to_string(),
            content: content.into(),
            reasoning_content: None,
            tool_calls: None,
            tool_call_id: None,
            name: None,
        }
    }

    pub fn assistant(content: impl Into<String>) -> Self {
        Self {
            role: "assistant".to_string(),
            content: content.into(),
            reasoning_content: None,
            tool_calls: None,
            tool_call_id: None,
            name: None,
        }
    }

    pub fn tool(tool_call_id: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            role: "tool".to_string(),
            content: content.into(),
            reasoning_content: None,
            tool_calls: None,
            tool_call_id: Some(tool_call_id.into()),
            name: None,
        }
    }

    pub fn with_tool_calls(mut self, calls: Vec<serde_json::Value>) -> Self {
        self.tool_calls = Some(calls);
        self
    }

    pub fn with_reasoning(mut self, reasoning: impl Into<String>) -> Self {
        self.reasoning_content = Some(reasoning.into());
        self
    }
}

#[derive(Debug)]
pub enum AiError {
    Request(String),
    Parse(String),
    Config(String),
}

impl std::fmt::Display for AiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AiError::Request(s) => write!(f, "Request error: {s}"),
            AiError::Parse(s) => write!(f, "Parse error: {s}"),
            AiError::Config(s) => write!(f, "Config error: {s}"),
        }
    }
}

impl std::error::Error for AiError {}

impl AiClient {
    pub fn new(config: Config) -> Result<Self, AiError> {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(300))
            .build()
            .map_err(|e| AiError::Config(e.to_string()))?;
        
        // Auto-detect if we need a special User-Agent
        let user_agent = Self::detect_user_agent(&config);
        
        Ok(Self { config, client, user_agent })
    }

    pub fn with_user_agent(mut self, agent: CodingAgent) -> Self {
        self.user_agent = Some(agent.as_str().to_string());
        self
    }

    /// Detect if we need a special User-Agent based on config
    fn detect_user_agent(config: &Config) -> Option<String> {
        let base_url = &config.ai.base_url;
        
        // Check if using kimi-for-coding API
        if base_url.contains("kimi.com/coding") || base_url.contains("kimi-for-coding") {
            tracing::info!("Detected kimi-for-coding API, using Coding Agent User-Agent");
            return Some(CodingAgent::ClaudeCode.as_str().to_string());
        }
        
        None
    }

    fn build_request(&self, url: &str) -> reqwest::RequestBuilder {
        let mut req = self.client
            .post(url)
            .header("Authorization", format!("Bearer {}", self.config.ai.api_key))
            .header("Content-Type", "application/json")
            .header("Accept", "application/json");
        
        // Add User-Agent if needed (for Coding APIs)
        if let Some(ref ua) = self.user_agent {
            req = req.header("User-Agent", ua);
            tracing::info!("Using Coding Agent User-Agent: {}", ua);
        } else {
            // Default User-Agent for standard APIs
            req = req.header("User-Agent", format!("D-Chat/{} (AI Daemon)", env!("CARGO_PKG_VERSION")));
        }
        
        req
    }
}

/// Chat response with optional tool calls
#[derive(Debug, Clone)]
pub struct ChatResponse {
    pub content: Option<String>,
    pub tool_calls: Vec<ToolCall>,
}

impl ChatResponse {
    pub fn new(content: impl Into<String>) -> Self {
        Self {
            content: Some(content.into()),
            tool_calls: Vec::new(),
        }
    }

    pub fn with_tool_calls(mut self, tool_calls: Vec<ToolCall>) -> Self {
        self.tool_calls = tool_calls;
        self
    }

    pub fn has_tool_calls(&self) -> bool {
        !self.tool_calls.is_empty()
    }
}

impl AiClient {
    /// Simple chat without tools (legacy API)
    pub async fn chat(&self, messages: Vec<ChatMessage>) -> Result<String, AiError> {
        let response = self.chat_with_tools(messages, &[]).await?;
        Ok(response.content.unwrap_or_default())
    }

    /// Chat with tool support
    pub async fn chat_with_tools(
        &self,
        messages: Vec<ChatMessage>,
        tools: &[Tool],
    ) -> Result<ChatResponse, AiError> {
        if self.config.ai.api_key.is_empty() {
            return Err(AiError::Config("API key not configured".to_string()));
        }

        let mut body = serde_json::json!({
            "model": self.config.ai.model,
            "messages": messages,
            "temperature": self.config.ai.temperature,
            "max_tokens": self.config.ai.max_tokens,
        });
        
        tracing::debug!("Request body: {}", serde_json::to_string_pretty(&body).unwrap_or_default());

        // Add tools if provided
        if !tools.is_empty() {
            body["tools"] = serde_json::to_value(tools).map_err(|e| AiError::Config(e.to_string()))?;
            body["tool_choice"] = "auto".into();
        }

        let response = self
            .build_request(&format!("{}/chat/completions", self.config.ai.base_url))
            .json(&body)
            .send()
            .await
            .map_err(|e| AiError::Request(e.to_string()))?;

        let status = response.status();
        if !status.is_success() {
            let text = response.text().await.unwrap_or_default();
            return Err(AiError::Request(format!("HTTP {status}: {text}")));
        }

        let json: serde_json::Value = response
            .json()
            .await
            .map_err(|e| AiError::Parse(e.to_string()))?;

        let message = &json["choices"][0]["message"];
        
        // Extract content
        let content = message["content"].as_str().map(|s| s.to_string());
        
        // Extract tool calls
        let mut tool_calls = Vec::new();
        if let Some(calls) = message["tool_calls"].as_array() {
            for call in calls {
                if let Some(id) = call["id"].as_str() {
                    if let Some(function) = call["function"].as_object() {
                        let name = function["name"].as_str().unwrap_or("").to_string();
                        let arguments = function["arguments"].as_str().unwrap_or("{}").to_string();
                        tool_calls.push(ToolCall {
                            id: id.to_string(),
                            call_type: "function".to_string(),
                            function: crate::tool::FunctionCall { name, arguments },
                        });
                    }
                }
            }
        }

        Ok(ChatResponse { content, tool_calls })
    }

    pub async fn chat_stream(
        &self,
        messages: Vec<ChatMessage>,
    ) -> Result<impl futures::Stream<Item = Result<String, AiError>>, AiError> {
        if self.config.ai.api_key.is_empty() {
            return Err(AiError::Config("API key not configured".to_string()));
        }

        let body = serde_json::json!({
            "model": self.config.ai.model,
            "messages": messages,
            "temperature": self.config.ai.temperature,
            "max_tokens": self.config.ai.max_tokens,
            "stream": true,
        });

        let response = self
            .build_request(&format!("{}/chat/completions", self.config.ai.base_url))
            .json(&body)
            .send()
            .await
            .map_err(|e| AiError::Request(e.to_string()))?;

        let status = response.status();
        if !status.is_success() {
            let text = response.text().await.unwrap_or_default();
            return Err(AiError::Request(format!("HTTP {status}: {text}")));
        }

        let stream = response.bytes_stream().filter_map(|chunk| async move {
            match chunk {
                Ok(bytes) => {
                    let text = String::from_utf8_lossy(&bytes);
                    let mut result = String::new();
                    for line in text.lines() {
                        if let Some(data) = line.strip_prefix("data: ") {
                            if data == "[DONE]" {
                                continue;
                            }
                            if let Ok(json) = serde_json::from_str::<serde_json::Value>(data) {
                                if let Some(content) = json["choices"][0]["delta"]["content"].as_str() {
                                    result.push_str(content);
                                }
                            }
                        }
                    }
                    if result.is_empty() {
                        None
                    } else {
                        Some(Ok(result))
                    }
                }
                Err(e) => Some(Err(AiError::Request(e.to_string()))),
            }
        });

        Ok(stream)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_coding_agent_user_agent() {
        // User-Agent format must be lowercase with patch version for kimi-for-coding API
        assert_eq!(CodingAgent::ClaudeCode.as_str(), "claude-code/0.1.0");
        assert_eq!(CodingAgent::KimiCli.as_str(), "kimi-cli/1.0.0");
        assert_eq!(CodingAgent::RooCode.as_str(), "roo-code/1.0.0");
    }

    #[test]
    fn test_detect_user_agent_for_coding_api() {
        let config = Config {
            ai: kernel::AiConfig {
                api_key: "test".to_string(),
                base_url: "https://api.kimi.com/coding/v1".to_string(),
                model: "kimi-for-coding".to_string(),
                temperature: 0.7,
                max_tokens: 8192,
            },
            server: kernel::ServerConfig::default(),
        };
        
        let ua = AiClient::detect_user_agent(&config);
        assert!(ua.is_some());
        assert_eq!(ua.unwrap(), "claude-code/0.1.0");
    }

    #[test]
    fn test_no_user_agent_for_standard_api() {
        let config = Config {
            ai: kernel::AiConfig {
                api_key: "test".to_string(),
                base_url: "https://api.moonshot.cn/v1".to_string(),
                model: "kimi-k2-5".to_string(),
                temperature: 0.7,
                max_tokens: 32768,
            },
            server: kernel::ServerConfig::default(),
        };
        
        let ua = AiClient::detect_user_agent(&config);
        assert!(ua.is_none());
    }
}
