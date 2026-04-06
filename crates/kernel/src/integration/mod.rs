//! Integration hub for external services and APIs
//!
//! Provides:
//! - Plugin-based integration architecture
//! - Common interface for external APIs
//! - Authentication management
//! - Rate limiting and caching

use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{debug, error, info, warn};

/// Integration capability flags
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Capabilities {
    pub read: bool,
    pub write: bool,
    pub delete: bool,
    pub search: bool,
    pub subscribe: bool,
}

impl Capabilities {
    /// Full access capabilities
    pub fn full() -> Self {
        Self {
            read: true,
            write: true,
            delete: true,
            search: true,
            subscribe: true,
        }
    }

    /// Read-only capabilities
    pub fn read_only() -> Self {
        Self {
            read: true,
            write: false,
            delete: false,
            search: true,
            subscribe: false,
        }
    }
}

/// Authentication configuration
#[derive(Debug, Clone)]
pub enum AuthConfig {
    /// API key authentication
    ApiKey { key: String, header: String },
    /// OAuth 2.0
    OAuth2 {
        client_id: String,
        client_secret: String,
        token_url: String,
        scopes: Vec<String>,
    },
    /// Bearer token
    Bearer(String),
    /// Basic authentication
    Basic { username: String, password: String },
}

/// Integration configuration
#[derive(Debug, Clone)]
pub struct IntegrationConfig {
    /// Integration name
    pub name: String,
    /// Base URL or endpoint
    pub endpoint: String,
    /// Authentication configuration
    pub auth: Option<AuthConfig>,
    /// Rate limit: requests per minute
    pub rate_limit: Option<u32>,
    /// Request timeout in seconds
    pub timeout_secs: u64,
    /// Retry configuration
    pub max_retries: u32,
    /// Additional headers
    pub headers: HashMap<String, String>,
}

impl Default for IntegrationConfig {
    fn default() -> Self {
        Self {
            name: String::new(),
            endpoint: String::new(),
            auth: None,
            rate_limit: None,
            timeout_secs: 30,
            max_retries: 3,
            headers: HashMap::new(),
        }
    }
}

/// Integration trait for external services
#[async_trait::async_trait]
pub trait Integration: Send + Sync {
    /// Get integration name
    fn name(&self) -> &str;

    /// Get capabilities
    fn capabilities(&self) -> Capabilities;

    /// Check if integration is healthy
    async fn health_check(&self) -> anyhow::Result<()>;

    /// Execute a request
    async fn execute(&self, request: IntegrationRequest) -> anyhow::Result<IntegrationResponse>;
}

/// Integration request
#[derive(Debug, Clone)]
pub struct IntegrationRequest {
    /// Operation type
    pub operation: Operation,
    /// Resource path
    pub path: String,
    /// Query parameters
    pub params: HashMap<String, String>,
    /// Request body
    pub body: Option<serde_json::Value>,
    /// Custom headers
    pub headers: HashMap<String, String>,
}

/// Operation types
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Operation {
    Get,
    Post,
    Put,
    Patch,
    Delete,
    Search,
}

/// Integration response
#[derive(Debug, Clone)]
pub struct IntegrationResponse {
    /// HTTP status or equivalent
    pub status: u16,
    /// Response headers
    pub headers: HashMap<String, String>,
    /// Response body
    pub body: serde_json::Value,
    /// Metadata
    pub metadata: HashMap<String, String>,
}

/// Integration registry
pub struct IntegrationRegistry {
    integrations: RwLock<HashMap<String, Arc<dyn Integration>>>,
}

impl IntegrationRegistry {
    /// Create a new registry
    pub fn new() -> Self {
        Self {
            integrations: RwLock::new(HashMap::new()),
        }
    }

    /// Register an integration
    pub async fn register(&self, integration: Arc<dyn Integration>) {
        let name = integration.name().to_string();
        info!("Registering integration: {}", name);
        
        let mut integrations = self.integrations.write().await;
        integrations.insert(name, integration);
    }

    /// Unregister an integration
    pub async fn unregister(&self, name: &str) -> Option<Arc<dyn Integration>> {
        let mut integrations = self.integrations.write().await;
        integrations.remove(name)
    }

    /// Get an integration
    pub async fn get(&self, name: &str) -> Option<Arc<dyn Integration>> {
        let integrations = self.integrations.read().await;
        integrations.get(name).cloned()
    }

    /// List all integrations
    pub async fn list(&self) -> Vec<String> {
        let integrations = self.integrations.read().await;
        integrations.keys().cloned().collect()
    }

    /// Check health of all integrations
    pub async fn health_check_all(&self) -> HashMap<String, anyhow::Result<()>> {
        let integrations = self.integrations.read().await;
        let mut results = HashMap::new();

        for (name, integration) in integrations.iter() {
            let result = integration.health_check().await;
            if let Err(ref e) = result {
                warn!("Integration {} health check failed: {}", name, e);
            }
            results.insert(name.clone(), result);
        }

        results
    }
}

impl Default for IntegrationRegistry {
    fn default() -> Self {
        Self::new()
    }
}

/// HTTP-based integration client
pub struct HttpIntegration {
    config: IntegrationConfig,
    client: reqwest::Client,
}

impl HttpIntegration {
    /// Create a new HTTP integration
    pub fn new(config: IntegrationConfig) -> anyhow::Result<Self> {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(config.timeout_secs))
            .build()?;

        Ok(Self { config, client })
    }

    /// Build request with auth
    fn build_request(&self, request: &IntegrationRequest) -> anyhow::Result<reqwest::RequestBuilder> {
        let url = format!("{}{}", self.config.endpoint, request.path);
        let mut builder = match request.operation {
            Operation::Get => self.client.get(&url),
            Operation::Post => self.client.post(&url),
            Operation::Put => self.client.put(&url),
            Operation::Patch => self.client.patch(&url),
            Operation::Delete => self.client.delete(&url),
            Operation::Search => self.client.get(&url),
        };

        // Add auth
        if let Some(ref auth) = self.config.auth {
            builder = match auth {
                AuthConfig::ApiKey { key, header } => builder.header(header, key),
                AuthConfig::Bearer(token) => builder.header("Authorization", format!("Bearer {}", token)),
                AuthConfig::Basic { username, password } => {
                    builder.basic_auth(username, Some(password))
                }
                _ => builder,
            };
        }

        // Add custom headers
        for (key, value) in &self.config.headers {
            builder = builder.header(key, value);
        }
        for (key, value) in &request.headers {
            builder = builder.header(key, value);
        }

        // Add query params
        for (key, value) in &request.params {
            builder = builder.query(&[(key, value)]);
        }

        // Add body
        if let Some(ref body) = request.body {
            builder = builder.json(body);
        }

        Ok(builder)
    }
}

#[async_trait::async_trait]
impl Integration for HttpIntegration {
    fn name(&self) -> &str {
        &self.config.name
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities::full()
    }

    async fn health_check(&self) -> anyhow::Result<()> {
        let request = IntegrationRequest {
            operation: Operation::Get,
            path: "/health".to_string(),
            params: HashMap::new(),
            body: None,
            headers: HashMap::new(),
        };

        self.execute(request).await.map(|_| ())
    }

    async fn execute(&self, request: IntegrationRequest) -> anyhow::Result<IntegrationResponse> {
        let builder = self.build_request(&request)?;
        let response = builder.send().await?;

        let status = response.status().as_u16();
        let headers = response
            .headers()
            .iter()
            .filter_map(|(k, v)| v.to_str().ok().map(|v| (k.to_string(), v.to_string())))
            .collect();
        let body: serde_json::Value = response.json().await.unwrap_or_default();

        Ok(IntegrationResponse {
            status,
            headers,
            body,
            metadata: HashMap::new(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_capabilities() {
        let full = Capabilities::full();
        assert!(full.read && full.write && full.delete);

        let read = Capabilities::read_only();
        assert!(read.read && !read.write && !read.delete);
    }

    #[tokio::test]
    async fn test_registry() {
        let registry = IntegrationRegistry::new();
        assert!(registry.list().await.is_empty());
    }
}
