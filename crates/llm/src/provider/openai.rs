//! OpenAI provider implementation

use super::{
    CompletionRequest, CompletionResponse, Message, Provider, ProviderCapabilities,
    ProviderConfig, ProviderType, Role, Usage,
};
use async_trait::async_trait;

/// OpenAI API client
pub struct OpenAiProvider {
    client: reqwest::Client,
    config: ProviderConfig,
    api_key: String,
}

impl OpenAiProvider {
    /// API endpoint
    const API_BASE: &'static str = "https://api.openai.com/v1";

    /// Create new provider
    pub fn new(api_key: impl Into<String>) -> anyhow::Result<Self> {
        let api_key = api_key.into();
        if api_key.is_empty() {
            anyhow::bail!("API key is required");
        }

        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(60))
            .build()?;

        let config = ProviderConfig::new(ProviderType::OpenAi)
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
            anyhow::anyhow!("API key is required for OpenAI provider")
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

    /// Convert message to OpenAI format
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
impl Provider for OpenAiProvider {
    fn provider_type(&self) -> ProviderType {
        ProviderType::OpenAi
    }

    fn capabilities(&self) -> ProviderCapabilities {
        ProviderCapabilities {
            streaming: true,
            function_calling: true,
            reasoning: false,
            vision: true,
        }
    }

    fn default_model(&self) -> &str {
        self.config.default_model.as_deref().unwrap_or("gpt-4o-mini")
    }

    fn is_available(&self) -> bool {
        !self.api_key.is_empty()
    }

    async fn complete(&self, request: CompletionRequest) -> anyhow::Result<CompletionResponse> {
        let url = format!(
            "{}/chat/completions",
            self.config.base_url.as_deref().unwrap_or(Self::API_BASE)
        );

        let messages: Vec<_> = request.messages.iter()
            .map(Self::convert_message)
            .collect();

        let mut body = serde_json::json!({
            "model": request.model,
            "messages": messages,
            "temperature": request.temperature.unwrap_or(0.7),
            "stream": false,
        });

        if let Some(max_tokens) = request.max_tokens {
            body["max_tokens"] = serde_json::json!(max_tokens);
        }

        if let Some(top_p) = request.top_p {
            body["top_p"] = serde_json::json!(top_p);
        }

        if let Some(ref tools) = request.tools {
            body["tools"] = serde_json::json!(tools);
        }

        let response = self.client
            .post(&url)
            .header("Authorization", format!("Bearer {}", self.api_key))
            .header("Content-Type", "application/json")
            .json(&body)
            .send()
            .await?;

        if !response.status().is_success() {
            let error_text = response.text().await?;
            anyhow::bail!("OpenAI API error: {}", error_text);
        }

        let api_response: OpenAiResponse = response.json().await?;
        
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

/// OpenAI API response
#[derive(Debug, serde::Deserialize)]
struct OpenAiResponse {
    choices: Vec<OpenAiChoice>,
    usage: Option<OpenAiUsage>,
}

#[derive(Debug, serde::Deserialize)]
struct OpenAiChoice {
    message: OpenAiMessage,
    finish_reason: String,
}

#[derive(Debug, serde::Deserialize)]
struct OpenAiMessage {
    content: String,
}

#[derive(Debug, serde::Deserialize)]
struct OpenAiUsage {
    prompt_tokens: u32,
    completion_tokens: u32,
    total_tokens: u32,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_provider_creation() {
        let provider = OpenAiProvider::new("test-key");
        assert!(provider.is_ok());
        
        let provider = provider.unwrap();
        assert_eq!(provider.provider_type(), ProviderType::OpenAi);
        assert!(provider.is_available());
    }

    #[test]
    fn test_default_model() {
        let provider = OpenAiProvider::new("test-key").unwrap();
        assert_eq!(provider.default_model(), "gpt-4o-mini");
    }

    #[test]
    fn test_capabilities() {
        let provider = OpenAiProvider::new("test-key").unwrap();
        let caps = provider.capabilities();
        assert!(caps.streaming);
        assert!(caps.function_calling);
        assert!(caps.vision);
        assert!(!caps.reasoning);
    }
}
