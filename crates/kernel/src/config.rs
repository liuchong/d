use serde::{Deserialize, Serialize};
use std::path::PathBuf;

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
    pub fn load() -> anyhow::Result<Self> {
        let config_dir = dirs::config_dir()
            .ok_or_else(|| anyhow::anyhow!("Cannot find config directory"))?
            .join("d");
        
        let config_file = config_dir.join("config.toml");
        
        if config_file.exists() {
            let content = std::fs::read_to_string(&config_file)?;
            let config: Config = toml::from_str(&content)?;
            Ok(config)
        } else {
            let config = Config::default();
            std::fs::create_dir_all(&config_dir)?;
            std::fs::write(&config_file, toml::to_string_pretty(&config)?)?;
            Ok(config)
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
