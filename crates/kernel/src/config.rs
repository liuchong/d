use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use tracing::{info, warn};

/// Main configuration struct (nested format for ~/.config/d/config.toml)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub ai: AiConfig,
    pub server: ServerConfig,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            ai: AiConfig::default(),
            server: ServerConfig::default(),
        }
    }
}

impl Config {
    /// Load configuration with chat.zig compatibility
    /// 
    /// Priority:
    /// 1. Current directory config.toml (chat.zig compatible)
    /// 2. ~/.config/d/config.toml (legacy nested format)
    /// 3. Default values
    pub fn load() -> anyhow::Result<Self> {
        // First, try current directory config.toml (chat.zig compatible)
        let local_config = PathBuf::from("config.toml");
        if local_config.exists() {
            info!("Loading config from {}", local_config.display());
            let content = std::fs::read_to_string(&local_config)?;
            
            // Try flat format first (chat.zig compatible)
            if let Ok(flat_config) = toml::from_str::<FlatConfig>(&content) {
                info!("Loaded flat config format (chat.zig compatible)");
                return Ok(flat_config.into());
            }
            
            // Try nested format
            if let Ok(config) = toml::from_str::<Config>(&content) {
                info!("Loaded nested config format");
                return Ok(config);
            }
            
            warn!("Failed to parse config.toml, trying legacy location");
        }
        
        // Second, try legacy ~/.config/d/config.toml
        let config_dir = dirs::config_dir()
            .ok_or_else(|| anyhow::anyhow!("Cannot find config directory"))?
            .join("d");
        let config_file = config_dir.join("config.toml");
        
        if config_file.exists() {
            info!("Loading config from {}", config_file.display());
            let content = std::fs::read_to_string(&config_file)?;
            let config: Config = toml::from_str(&content)?;
            return Ok(config);
        }
        
        // Create default config in legacy location
        info!("Creating default config at {}", config_file.display());
        let config = Config::default();
        std::fs::create_dir_all(&config_dir)?;
        std::fs::write(&config_file, toml::to_string_pretty(&config)?)?;
        Ok(config)
    }
}

/// Flat configuration format (chat.zig compatible)
#[derive(Debug, Clone, Deserialize)]
struct FlatConfig {
    api_key: String,
    #[serde(default = "default_base_url")]
    base_url: String,
    #[serde(default = "default_model")]
    model: String,
    #[serde(default = "default_temperature")]
    temperature: f32,
    #[serde(default = "default_max_tokens")]
    max_tokens: u32,
    
    // Optional server settings
    #[serde(default)]
    host: Option<String>,
    #[serde(default)]
    port: Option<u16>,
}

fn default_base_url() -> String {
    "https://api.moonshot.cn/v1".to_string()
}

fn default_model() -> String {
    "kimi-k2-5".to_string()
}

fn default_temperature() -> f32 {
    0.7
}

fn default_max_tokens() -> u32 {
    32768
}

impl From<FlatConfig> for Config {
    fn from(flat: FlatConfig) -> Self {
        Self {
            ai: AiConfig {
                api_key: flat.api_key,
                base_url: flat.base_url,
                model: flat.model,
                temperature: flat.temperature,
                max_tokens: flat.max_tokens,
            },
            server: ServerConfig {
                host: flat.host.unwrap_or_else(|| "localhost".to_string()),
                port: flat.port.unwrap_or(8080),
                root: PathBuf::from("."),
            },
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AiConfig {
    pub api_key: String,
    pub base_url: String,
    pub model: String,
    pub temperature: f32,
    pub max_tokens: u32,
}

impl Default for AiConfig {
    fn default() -> Self {
        Self {
            api_key: String::new(),
            base_url: "https://api.moonshot.cn/v1".to_string(),
            model: "kimi-k2-5".to_string(),
            temperature: 0.7,
            max_tokens: 32768,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerConfig {
    pub host: String,
    pub port: u16,
    pub root: PathBuf,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            host: "localhost".to_string(),
            port: 8080,
            root: PathBuf::from("."),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_flat_config_conversion() {
        let flat = FlatConfig {
            api_key: "test-key".to_string(),
            base_url: "https://api.test.com".to_string(),
            model: "test-model".to_string(),
            temperature: 0.5,
            max_tokens: 1000,
            host: Some("0.0.0.0".to_string()),
            port: Some(3000),
        };
        
        let config: Config = flat.into();
        
        assert_eq!(config.ai.api_key, "test-key");
        assert_eq!(config.ai.base_url, "https://api.test.com");
        assert_eq!(config.ai.model, "test-model");
        assert_eq!(config.ai.temperature, 0.5);
        assert_eq!(config.ai.max_tokens, 1000);
        assert_eq!(config.server.host, "0.0.0.0");
        assert_eq!(config.server.port, 3000);
    }

    #[test]
    fn test_flat_config_parse() {
        let toml = r#"
api_key = "sk-test"
base_url = "https://api.kimi.com/coding/v1"
model = "kimi-for-coding"
temperature = 0.7
max_tokens = 8192
"#;
        
        let flat: FlatConfig = toml::from_str(toml).unwrap();
        assert_eq!(flat.api_key, "sk-test");
        assert_eq!(flat.base_url, "https://api.kimi.com/coding/v1");
        assert_eq!(flat.model, "kimi-for-coding");
        assert_eq!(flat.temperature, 0.7);
        assert_eq!(flat.max_tokens, 8192);
    }

    #[test]
    fn test_flat_config_defaults() {
        let toml = r#"api_key = "sk-test""#;
        
        let flat: FlatConfig = toml::from_str(toml).unwrap();
        assert_eq!(flat.api_key, "sk-test");
        assert_eq!(flat.base_url, "https://api.moonshot.cn/v1");
        assert_eq!(flat.model, "kimi-k2-5");
        assert_eq!(flat.temperature, 0.7);
        assert_eq!(flat.max_tokens, 32768);
    }
}
