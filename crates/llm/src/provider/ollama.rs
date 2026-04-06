//! Ollama local LLM provider implementation

use super::{
    CompletionRequest, CompletionResponse, Message, Provider, ProviderCapabilities,
    ProviderConfig, ProviderType, Role, Usage,
};
use async_trait::async_trait;

/// Ollama API client
pub struct OllamaProvider {
    client: reqwest::Client,
    config: ProviderConfig,
    base_url: String,
}

impl OllamaProvider {
    /// Default local endpoint
    const DEFAULT_BASE: &'static str = "http://localhost:11434";

    /// Create new provider
    pub fn new() -> anyhow::Result<Self> {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(120))
            .build()?;

        let config = ProviderConfig::new(ProviderType::Ollama)
            .with_base_url(Self::DEFAULT_BASE);

        Ok(Self {
            client,
            config,
            base_url: Self::DEFAULT_BASE.to_string(),
        })
    }

    /// Create with custom base URL
    pub fn with_base_url(base_url: impl Into<String>) -> anyhow::Result<Self> {
        let base_url = base_url.into();
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(120))
            .build()?;

        let config = ProviderConfig::new(ProviderType::Ollama)
            .with_base_url(&base_url);

        Ok(Self {
            client,
            config,
            base_url,
        })
    }

    /// Create from config
    pub fn from_config(config: ProviderConfig) -> anyhow::Result<Self> {
        let base_url = config.base_url.clone().unwrap_or_else(|| {
            Self::DEFAULT_BASE.to_string()
        });

        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(config.timeout_secs))
            .build()?;

        Ok(Self {
            client,
            config,
            base_url,
        })
    }

    /// Convert messages to Ollama prompt format
    fn convert_messages(messages: &[Message]) -> String {
        messages.iter()
            .map(|msg| {
                let role_str = match msg.role {
                    Role::System => format!("[SYSTEM]\n{}\n", msg.content),
                    Role::User => format!("[USER]\n{}\n", msg.content),
                    Role::Assistant => format!("[ASSISTANT]\n{}\n", msg.content),
                    Role::Tool => format!("[TOOL]\n{}\n", msg.content),
                };
                role_str
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// Check if Ollama is running
    pub async fn is_running(&self) -> bool {
        match self.client
            .get(format!("{}/api/tags", self.base_url))
            .send()
            .await
        {
            Ok(resp) => resp.status().is_success(),
            Err(_) => false,
        }
    }

    /// List available models
    pub async fn list_models(&self) -> anyhow::Result<Vec<String>> {
        let response = self.client
            .get(format!("{}/api/tags", self.base_url))
            .send()
            .await?;

        if !response.status().is_success() {
            anyhow::bail!("Failed to list models");
        }

        let tags: OllamaTagsResponse = response.json().await?;
        Ok(tags.models.into_iter().map(|m| m.name).collect())
    }
}

#[async_trait]
impl Provider for OllamaProvider {
    fn provider_type(&self) -> ProviderType {
        ProviderType::Ollama
    }

    fn capabilities(&self) -> ProviderCapabilities {
        ProviderCapabilities {
            streaming: true,
            function_calling: false, // Limited support
            reasoning: false,
            vision: false,
        }
    }

    fn default_model(&self) -> &str {
        self.config.default_model.as_deref().unwrap_or("llama3.2")
    }

    fn is_available(&self) -> bool {
        // Check at runtime in is_running()
        true
    }

    async fn complete(&self, request: CompletionRequest) -> anyhow::Result<CompletionResponse> {
        let url = format!("{}/api/generate", self.base_url);

        let prompt = Self::convert_messages(&request.messages);

        let body = serde_json::json!({
            "model": request.model,
            "prompt": prompt,
            "stream": false,
            "options": {
                "temperature": request.temperature.unwrap_or(0.7),
                "num_predict": request.max_tokens.map(|n| n as i32).unwrap_or(-1),
            },
        });

        let response = self.client
            .post(&url)
            .header("Content-Type", "application/json")
            .json(&body)
            .send()
            .await?;

        if !response.status().is_success() {
            let error_text = response.text().await?;
            anyhow::bail!("Ollama API error: {}", error_text);
        }

        let api_response: OllamaResponse = response.json().await?;

        Ok(CompletionResponse {
            content: api_response.response,
            reasoning_content: None,
            finish_reason: if api_response.done {
                "stop".to_string()
            } else {
                "length".to_string()
            },
            usage: match (api_response.prompt_eval_count, api_response.eval_count) {
                (Some(p), Some(c)) => Some(Usage {
                    prompt_tokens: p,
                    completion_tokens: c,
                    total_tokens: p + c,
                }),
                _ => None,
            },
        })
    }
}

/// Ollama generate response
#[derive(Debug, serde::Deserialize)]
struct OllamaResponse {
    response: String,
    done: bool,
    #[serde(default)]
    prompt_eval_count: Option<u32>,
    #[serde(default)]
    eval_count: Option<u32>,
}

/// Ollama tags response
#[derive(Debug, serde::Deserialize)]
struct OllamaTagsResponse {
    models: Vec<OllamaModel>,
}

#[derive(Debug, serde::Deserialize)]
struct OllamaModel {
    name: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_provider_creation() {
        let provider = OllamaProvider::new();
        assert!(provider.is_ok());
        
        let provider = provider.unwrap();
        assert_eq!(provider.provider_type(), ProviderType::Ollama);
    }

    #[test]
    fn test_custom_base_url() {
        let provider = OllamaProvider::with_base_url("http://192.168.1.100:11434");
        assert!(provider.is_ok());
    }

    #[test]
    fn test_default_model() {
        let provider = OllamaProvider::new().unwrap();
        assert_eq!(provider.default_model(), "llama3.2");
    }

    #[test]
    fn test_message_conversion() {
        let messages = vec![
            Message::system("You are helpful"),
            Message::user("Hello"),
        ];
        
        let prompt = OllamaProvider::convert_messages(&messages);
        assert!(prompt.contains("[SYSTEM]"));
        assert!(prompt.contains("[USER]"));
    }
}
