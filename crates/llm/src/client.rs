use kernel::config::Config;
use futures::StreamExt;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone)]
pub struct AiClient {
    config: Config,
    client: reqwest::Client,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: String,
    pub content: String,
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
        
        Ok(Self { config, client })
    }

    pub async fn chat(&self, messages: Vec<ChatMessage>) -> Result<String, AiError> {
        if self.config.ai.api_key.is_empty() {
            return Err(AiError::Config("API key not configured".to_string()));
        }

        let body = serde_json::json!({
            "model": self.config.ai.model,
            "messages": messages,
            "temperature": self.config.ai.temperature,
            "max_tokens": self.config.ai.max_tokens,
        });

        let response = self.client
            .post(&format!("{}/chat/completions", self.config.ai.base_url))
            .header("Authorization", format!("Bearer {}", self.config.ai.api_key))
            .header("Content-Type", "application/json")
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

        json["choices"][0]["message"]["content"]
            .as_str()
            .map(|s| s.to_string())
            .ok_or_else(|| AiError::Parse("Invalid response format".to_string()))
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

        let response = self.client
            .post(&format!("{}/chat/completions", self.config.ai.base_url))
            .header("Authorization", format!("Bearer {}", self.config.ai.api_key))
            .header("Content-Type", "application/json")
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
