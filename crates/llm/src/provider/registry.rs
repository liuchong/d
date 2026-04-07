//! Provider registry for managing multiple LLM providers

use super::{
    MoonshotProvider, OllamaProvider, OpenAiProvider, Provider, ProviderConfig, ProviderType,
};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{debug, info};

/// Provider registry
pub struct ProviderRegistry {
    providers: RwLock<HashMap<ProviderType, Arc<dyn Provider>>>,
    default_provider: RwLock<ProviderType>,
}

impl ProviderRegistry {
    /// Create new registry
    pub fn new() -> Self {
        Self {
            providers: RwLock::new(HashMap::new()),
            default_provider: RwLock::new(ProviderType::Moonshot),
        }
    }

    /// Create with default providers from environment
    pub async fn with_defaults() -> Self {
        let registry = Self::new();

        // Try to add providers from environment variables
        if let Ok(api_key) = std::env::var("MOONSHOT_API_KEY") {
            if let Ok(provider) = MoonshotProvider::new(api_key) {
                registry.register(Arc::new(provider)).await;
                info!("Registered Moonshot provider");
            }
        }

        if let Ok(api_key) = std::env::var("OPENAI_API_KEY") {
            if let Ok(provider) = OpenAiProvider::new(api_key) {
                registry.register(Arc::new(provider)).await;
                info!("Registered OpenAI provider");
            }
        }

        // Ollama doesn't need API key, try to detect
        if let Ok(provider) = OllamaProvider::new() {
            if provider.is_running().await {
                registry.register(Arc::new(provider)).await;
                info!("Registered Ollama provider");
            } else {
                debug!("Ollama not running, skipping registration");
            }
        }

        registry
    }

    /// Register a provider
    pub async fn register(&self, provider: Arc<dyn Provider>) {
        let provider_type = provider.provider_type();
        info!("Registering provider: {:?}", provider_type);
        
        let mut providers = self.providers.write().await;
        providers.insert(provider_type, provider);
    }

    /// Unregister a provider
    pub async fn unregister(&self, provider_type: ProviderType) -> Option<Arc<dyn Provider>> {
        info!("Unregistering provider: {:?}", provider_type);
        
        let mut providers = self.providers.write().await;
        providers.remove(&provider_type)
    }

    /// Get provider by type
    pub async fn get(&self, provider_type: ProviderType) -> Option<Arc<dyn Provider>> {
        let providers = self.providers.read().await;
        providers.get(&provider_type).cloned()
    }

    /// Get default provider
    pub async fn get_default(&self) -> Option<Arc<dyn Provider>> {
        let default_type = *self.default_provider.read().await;
        self.get(default_type).await
    }

    /// Set default provider
    pub async fn set_default(&self, provider_type: ProviderType) -> anyhow::Result<()> {
        let providers = self.providers.read().await;
        if !providers.contains_key(&provider_type) {
            anyhow::bail!("Provider {:?} not registered", provider_type);
        }
        drop(providers);

        let mut default = self.default_provider.write().await;
        *default = provider_type;
        info!("Set default provider to: {:?}", provider_type);
        
        Ok(())
    }

    /// List available providers
    pub async fn list_available(&self) -> Vec<ProviderType> {
        let providers = self.providers.read().await;
        providers
            .iter()
            .filter(|(_, p)| p.is_available())
            .map(|(t, _)| *t)
            .collect()
    }

    /// List all registered providers
    pub async fn list_all(&self) -> Vec<ProviderType> {
        let providers = self.providers.read().await;
        providers.keys().copied().collect()
    }

    /// Check if provider is registered
    pub async fn is_registered(&self, provider_type: ProviderType) -> bool {
        let providers = self.providers.read().await;
        providers.contains_key(&provider_type)
    }

    /// Create provider from config and register
    pub async fn create_and_register(&self, config: ProviderConfig) -> anyhow::Result<()> {
        let provider: Arc<dyn Provider> = match config.provider_type {
            ProviderType::Moonshot => {
                Arc::new(MoonshotProvider::from_config(config)?)
            }
            ProviderType::OpenAi => {
                Arc::new(OpenAiProvider::from_config(config)?)
            }
            ProviderType::Ollama => {
                Arc::new(OllamaProvider::from_config(config)?)
            }
            _ => anyhow::bail!("Provider type not implemented"),
        };

        self.register(provider).await;
        Ok(())
    }

    /// Get provider capabilities summary
    pub async fn capabilities_summary(&self) -> HashMap<ProviderType, String> {
        let providers = self.providers.read().await;
        let mut summary = HashMap::new();

        for (provider_type, provider) in providers.iter() {
            let caps = provider.capabilities();
            let cap_str = format!(
                "streaming={}, tools={}, vision={}",
                caps.streaming, caps.function_calling, caps.vision
            );
            summary.insert(*provider_type, cap_str);
        }

        summary
    }
}

impl Default for ProviderRegistry {
    fn default() -> Self {
        Self::new()
    }
}

/// Global registry singleton
pub async fn global_registry() -> &'static ProviderRegistry {
    use std::sync::OnceLock;
    static REGISTRY: OnceLock<ProviderRegistry> = OnceLock::new();
    
    REGISTRY.get_or_init(|| {
        tokio::runtime::Handle::current().block_on(async {
            ProviderRegistry::with_defaults().await
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_registry() {
        let registry = ProviderRegistry::new();
        
        // Initially empty
        assert!(registry.get_default().await.is_none());
        assert!(registry.list_all().await.is_empty());
    }

    #[tokio::test]
    async fn test_register_provider() {
        let registry = ProviderRegistry::new();
        
        let provider = MoonshotProvider::new("test-key").unwrap();
        registry.register(Arc::new(provider)).await;
        
        assert!(registry.is_registered(ProviderType::Moonshot).await);
        assert_eq!(registry.list_all().await.len(), 1);
    }

    #[tokio::test]
    async fn test_default_provider() {
        let registry = ProviderRegistry::new();
        
        let provider = MoonshotProvider::new("test-key").unwrap();
        registry.register(Arc::new(provider)).await;
        
        // Set as default
        registry.set_default(ProviderType::Moonshot).await.unwrap();
        
        // Get default
        let default = registry.get_default().await;
        assert!(default.is_some());
        assert_eq!(default.unwrap().provider_type(), ProviderType::Moonshot);
    }

    #[tokio::test]
    async fn test_unregister() {
        let registry = ProviderRegistry::new();
        
        let provider = MoonshotProvider::new("test-key").unwrap();
        registry.register(Arc::new(provider)).await;
        
        let removed = registry.unregister(ProviderType::Moonshot).await;
        assert!(removed.is_some());
        assert!(!registry.is_registered(ProviderType::Moonshot).await);
    }
}
