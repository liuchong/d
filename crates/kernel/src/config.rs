use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use tracing::{info, warn};

/// Main configuration struct (nested format for XDG config)
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

/// Configuration file names to search for
const CONFIG_NAMES: &[&str] = &["config.toml", ".d.toml", ".d/config.toml"];

impl Config {
    /// Load configuration with multi-level search
    /// 
    /// Search order (highest to lowest priority):
    /// 1. Current directory and ancestors (project-specific)
    /// 2. XDG_CONFIG_HOME/d/ (user-specific)
    /// 3. ~/.config/d/ (fallback XDG)
    /// 4. ~/.d/ (legacy home config)
    /// 5. /etc/d/ (system-wide)
    /// 6. Default values
    pub fn load() -> anyhow::Result<Self> {
        // Search paths in priority order
        let search_paths = Self::collect_search_paths();
        
        for path in &search_paths {
            if let Some(config) = Self::try_load_from(path) {
                return Ok(config);
            }
        }
        
        // No config found, create default in XDG location
        info!("No config found, creating default");
        let default_config = Config::default();
        default_config.save_default()?;
        Ok(default_config)
    }
    
    /// Collect all config search paths in priority order
    fn collect_search_paths() -> Vec<PathBuf> {
        let mut paths = Vec::new();
        
        // 1. Current directory and ancestors
        if let Ok(cwd) = std::env::current_dir() {
            let mut current = cwd.clone();
            loop {
                for name in CONFIG_NAMES {
                    paths.push(current.join(name));
                }
                
                // Stop at home directory or root
                if let Some(home) = dirs::home_dir() {
                    if current == home {
                        break;
                    }
                }
                if !current.pop() {
                    break;
                }
            }
        }
        
        // 2. XDG_CONFIG_HOME/d/
        if let Some(xdg_config) = dirs::config_dir() {
            for name in CONFIG_NAMES {
                paths.push(xdg_config.join("d").join(name));
            }
        }
        
        // 3. ~/.d/ (legacy)
        if let Some(home) = dirs::home_dir() {
            for name in CONFIG_NAMES {
                paths.push(home.join(".d").join(name));
            }
        }
        
        // 4. /etc/d/ (system-wide, Unix only)
        #[cfg(unix)]
        for name in CONFIG_NAMES {
            paths.push(PathBuf::from("/etc/d").join(name));
        }
        
        paths
    }
    
    /// Try to load config from a specific path
    fn try_load_from(path: &Path) -> Option<Config> {
        if !path.exists() {
            return None;
        }
        
        info!("Trying config: {}", path.display());
        
        let content = match std::fs::read_to_string(path) {
            Ok(c) => c,
            Err(e) => {
                warn!("Failed to read {}: {}", path.display(), e);
                return None;
            }
        };
        
        // Try flat format first (chat.zig compatible)
        if let Ok(flat_config) = toml::from_str::<FlatConfig>(&content) {
            info!("Loaded flat config from: {}", path.display());
            return Some(flat_config.into());
        }
        
        // Try nested format
        if let Ok(config) = toml::from_str::<Config>(&content) {
            info!("Loaded nested config from: {}", path.display());
            return Some(config);
        }
        
        warn!("Failed to parse config: {}", path.display());
        None
    }
    
    /// Save default config to XDG location
    fn save_default(&self) -> anyhow::Result<()> {
        let config_dir = dirs::config_dir()
            .ok_or_else(|| anyhow::anyhow!("Cannot find config directory"))?
            .join("d");
        
        let config_file = config_dir.join("config.toml");
        
        std::fs::create_dir_all(&config_dir)?;
        
        // Create template with comments
        let template = r#"# D - AI Daemon Configuration
# 
# Place this file in one of these locations (priority order):
# 1. ./config.toml  (current directory)
# 2. ~/.config/d/config.toml  (XDG config)
# 3. ~/.d/config.toml  (legacy)
# 4. /etc/d/config.toml  (system-wide)
#
# Get your API key from: https://platform.moonshot.cn/

[ai]
api_key = ""
base_url = "https://api.moonshot.cn/v1"
model = "kimi-k2-5"
temperature = 0.7
max_tokens = 32768

[server]
host = "localhost"
port = 8080
root = "."
"#;
        
        std::fs::write(&config_file, template)?;
        info!("Created default config at: {}", config_file.display());
        Ok(())
    }
    
    /// Get the config file path that was loaded (for debugging)
    pub fn loaded_path() -> Option<PathBuf> {
        let search_paths = Self::collect_search_paths();
        for path in &search_paths {
            if path.exists() {
                // Verify it's parseable
                if let Ok(content) = std::fs::read_to_string(path) {
                    if toml::from_str::<FlatConfig>(&content).is_ok() 
                        || toml::from_str::<Config>(&content).is_ok() {
                        return Some(path.clone());
                    }
                }
            }
        }
        None
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
    #[serde(default)]
    root: Option<PathBuf>,
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
                root: flat.root.unwrap_or_else(|| PathBuf::from(".")),
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
            root: Some(PathBuf::from("/tmp")),
        };
        
        let config: Config = flat.into();
        
        assert_eq!(config.ai.api_key, "test-key");
        assert_eq!(config.ai.base_url, "https://api.test.com");
        assert_eq!(config.ai.model, "test-model");
        assert_eq!(config.ai.temperature, 0.5);
        assert_eq!(config.ai.max_tokens, 1000);
        assert_eq!(config.server.host, "0.0.0.0");
        assert_eq!(config.server.port, 3000);
        assert_eq!(config.server.root, PathBuf::from("/tmp"));
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
    
    #[test]
    fn test_search_paths_collected() {
        let paths = Config::collect_search_paths();
        // Should have multiple paths from different locations
        assert!(!paths.is_empty());
        
        // Should include config.toml variants
        let has_config_toml = paths.iter().any(|p| {
            p.to_string_lossy().ends_with("config.toml")
        });
        assert!(has_config_toml);
        
        // May include XDG path (depending on environment)
        // Just verify we have reasonable number of paths
        assert!(paths.len() >= 3);
    }
}
