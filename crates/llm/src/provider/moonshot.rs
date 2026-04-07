//! Moonshot AI (Kimi) provider implementation

use super::{
    CompletionRequest, CompletionResponse, Message, Provider, ProviderCapabilities,
    ProviderConfig, ProviderType, Usage,
};
use async_trait::async_trait;

/// Moonshot API client
pub struct MoonshotProvider {
    client: reqwest::Client,
    config: ProviderConfig,
    api_key: String,
}

impl MoonshotProvider {
    /// API endpoint
    const API_BASE: &'static str = "https://api.moonshot.cn/v1";

    /// Create new provider
    pub fn new(api_key: impl Into<String>) -> anyhow::Result<Self> {
        let api_key = api_key.into();
        if api_key.is_empty() {
            anyhow::bail!("API key is required");
        }

        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(60))
            .build()?;

        let config = ProviderConfig::new(ProviderType::Moonshot)
            .with_base_url(Self::API_BASE)
            .with_api_key(&api_key);

        Ok(Self {
            client,
            config,
            api_key,
        })
    }

    /// Create from config
    pub fn from_config(config: ProviderConfig) -> anyhow::Result<Self> {
        let api_key = config.api_key.clone().ok_or_else(|| {
            anyhow::anyhow!("API key is required for Moonshot provider")
        })?;

        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(config.timeout_secs))
            .build()?;

        Ok(Self {
            client,
            config,
            api_key,
        })
    }

    /// Convert message to Moonshot format
    fn convert_message(msg: &Message) -> serde_json::Value {
        let mut json = serde_json::json!({
            "role": msg.role.to_string(),
            "content": msg.content,
        });

        if let Some(ref tool_calls) = msg.tool_calls {
            json["tool_calls"] = serde_json::json!(tool_calls);
        }

        if let Some(ref tool_call_id) = msg.tool_call_id {
            json["tool_call_id"] = serde_json::json!(tool_call_id);
        }

        if let Some(ref name) = msg.name {
            json["name"] = serde_json::json!(name);
        }

        json
    }
}

#[async_trait]
impl Provider for MoonshotProvider {
    fn provider_type(&self) -> ProviderType {
        ProviderType::Moonshot
    }

    fn capabilities(&self) -> ProviderCapabilities {
        ProviderCapabilities {
            streaming: true,
            function_calling: true,
            reasoning: true,
            vision: false,
        }
    }

    fn default_model(&self) -> &str {
        self.config.default_model.as_deref().unwrap_or("kimi-latest")
    }

    fn is_available(&self) -> bool {
        !self.api_key.is_empty()
    }

    async fn complete(&self, request: CompletionRequest) -> anyhow::Result<CompletionResponse> {
        let url = format!("{}/chat/completions", self.config.base_url.as_deref().unwrap_or(Self::API_BASE));

        let messages: Vec<_> = request.messages.iter()
            .map(Self::convert_message)
            .collect();

        let body = serde_json::json!({
            "model": request.model,
            "messages": messages,
            "temperature": request.temperature.unwrap_or(0.7),
            "stream": false,
        });

        let response = self.client
            .post(&url)
            .header("Authorization", format!("Bearer {}", self.api_key))
            .header("Content-Type", "application/json")
            .json(&body)
            .send()
            .await?;

        if !response.status().is_success() {
            let error_text = response.text().await?;
            anyhow::bail!("Moonshot API error: {}", error_text);
        }

        let api_response: MoonshotResponse = response.json().await?;
        
        let choice = api_response.choices.into_iter()
            .next()
            .ok_or_else(|| anyhow::anyhow!("No response from API"))?;

        Ok(CompletionResponse {
            content: choice.message.content,
            reasoning_content: None,
            finish_reason: choice.finish_reason,
            usage: api_response.usage.map(|u| Usage {
                prompt_tokens: u.prompt_tokens,
                completion_tokens: u.completion_tokens,
                total_tokens: u.total_tokens,
            }),
        })
    }
}

/// Moonshot API response
#[derive(Debug, serde::Deserialize)]
struct MoonshotResponse {
    choices: Vec<MoonshotChoice>,
    usage: Option<MoonshotUsage>,
}

#[derive(Debug, serde::Deserialize)]
struct MoonshotChoice {
    message: MoonshotMessage,
    finish_reason: String,
}

#[derive(Debug, serde::Deserialize)]
struct MoonshotMessage {
    content: String,
    #[serde(default)]
    reasoning_content: Option<String>,
}

#[derive(Debug, serde::Deserialize)]
struct MoonshotUsage {
    prompt_tokens: u32,
    completion_tokens: u32,
    total_tokens: u32,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_provider_creation() {
        let provider = MoonshotProvider::new("test-key");
        assert!(provider.is_ok());
        
        let provider = provider.unwrap();
        assert_eq!(provider.provider_type(), ProviderType::Moonshot);
        assert!(provider.is_available());
    }

    #[test]
    fn test_empty_key_rejected() {
        let provider = MoonshotProvider::new("");
        assert!(provider.is_err());
    }

    #[test]
    fn test_capabilities() {
        let provider = MoonshotProvider::new("test-key").unwrap();
        let caps = provider.capabilities();
        assert!(caps.streaming);
        assert!(caps.function_calling);
        assert!(caps.reasoning);
    }
}
