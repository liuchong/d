//! Plugin system for extensibility
//!
//! Provides:
//! - Plugin loading and lifecycle
//! - Hook system for extension points
//! - Capability-based permissions
//! - Isolated plugin contexts

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use tracing::{info, warn, error};

/// Plugin metadata
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginManifest {
    pub name: String,
    pub version: String,
    pub description: String,
    pub author: String,
    pub entry: String,
    pub capabilities: Vec<Capability>,
    pub hooks: Vec<HookType>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dependencies: Option<Vec<String>>,
}

/// Plugin capabilities
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Capability {
    /// Network access
    Network,
    /// Filesystem access
    Filesystem,
    /// Shell execution
    Execution,
    /// LLM API access
    Llm,
    /// UI modification
    Ui,
    /// Tool registration
    Tools,
}

/// Hook types for plugin extension
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum HookType {
    /// Before command execution
    PreCommand,
    /// After command execution
    PostCommand,
    /// On message received
    OnMessage,
    /// On startup
    OnStartup,
    /// On shutdown
    OnShutdown,
    /// Periodic tick
    OnTick,
}

/// Plugin trait - implement this to create plugins
#[async_trait::async_trait]
pub trait Plugin: Send + Sync {
    /// Get plugin name
    fn name(&self) -> &str;
    
    /// Get plugin version
    fn version(&self) -> &str;
    
    /// Initialize the plugin
    async fn init(&mut self, context: PluginContext) -> anyhow::Result<()>;
    
    /// Shutdown the plugin
    async fn shutdown(&mut self) -> anyhow::Result<()>;
    
    /// Handle a hook
    async fn on_hook(&mut self, hook: HookType, data: HookData) -> HookResult;
}

/// Plugin context - provides APIs to plugins
#[derive(Debug, Clone)]
pub struct PluginContext {
    /// Plugin configuration
    pub config: HashMap<String, serde_json::Value>,
    /// Data directory for plugin
    pub data_dir: PathBuf,
    /// Logger
    pub logger: PluginLogger,
}

/// Plugin logger
#[derive(Debug, Clone)]
pub struct PluginLogger {
    plugin_name: String,
}

impl PluginLogger {
    pub fn new(plugin_name: impl Into<String>) -> Self {
        Self {
            plugin_name: plugin_name.into(),
        }
    }
    
    pub fn info(&self, message: &str) {
        info!("[{}] {}", self.plugin_name, message);
    }
    
    pub fn warn(&self, message: &str) {
        warn!("[{}] {}", self.plugin_name, message);
    }
    
    pub fn error(&self, message: &str) {
        error!("[{}] {}", self.plugin_name, message);
    }
}

/// Hook data passed to plugins
#[derive(Debug, Clone)]
pub enum HookData {
    /// Command data
    Command { name: String, args: Vec<String> },
    /// Message data
    Message { role: String, content: String },
    /// Empty (for startup/shutdown/tick)
    None,
}

/// Hook result
#[derive(Debug, Clone)]
pub enum HookResult {
    /// Continue normally
    Continue,
    /// Cancel the operation
    Cancel { reason: String },
    /// Modified data
    Modified(HookData),
    /// Error
    Error(String),
}

/// Plugin manager
pub struct PluginManager {
    plugins: HashMap<String, Box<dyn Plugin>>,
    hooks: HashMap<HookType, Vec<String>>,
    plugin_dir: PathBuf,
}

impl PluginManager {
    /// Create a new plugin manager
    pub fn new(plugin_dir: impl Into<PathBuf>) -> Self {
        Self {
            plugins: HashMap::new(),
            hooks: HashMap::new(),
            plugin_dir: plugin_dir.into(),
        }
    }

    /// Load all plugins from directory
    pub async fn load_all(&mut self) -> anyhow::Result<()> {
        if !self.plugin_dir.exists() {
            info!("Plugin directory does not exist: {:?}", self.plugin_dir);
            return Ok(());
        }

        let mut entries = tokio::fs::read_dir(&self.plugin_dir).await?;
        
        while let Some(entry) = entries.next_entry().await? {
            let path = entry.path();
            if path.is_dir() || path.extension().map(|e| e == "rs").unwrap_or(false) {
                if let Err(e) = self.load_plugin(&path).await {
                    warn!("Failed to load plugin from {:?}: {}", path, e);
                }
            }
        }

        info!("Loaded {} plugins", self.plugins.len());
        Ok(())
    }

    /// Load a single plugin
    async fn load_plugin(&mut self, path: &Path) -> anyhow::Result<()> {
        info!("Loading plugin from {:?}", path);
        
        // Look for manifest
        let manifest_path = if path.is_dir() {
            path.join("plugin.toml")
        } else {
            path.with_extension("toml")
        };

        if !manifest_path.exists() {
            anyhow::bail!("No manifest found at {:?}", manifest_path);
        }

        let manifest_content = tokio::fs::read_to_string(&manifest_path).await?;
        let manifest: PluginManifest = toml::from_str(&manifest_content)?;

        info!("Found plugin: {} v{}", manifest.name, manifest.version);

        // Register hooks
        for hook_type in &manifest.hooks {
            self.hooks
                .entry(*hook_type)
                .or_default()
                .push(manifest.name.clone());
        }

        // TODO: Actually load and instantiate the plugin
        // For now, we just register the metadata
        
        Ok(())
    }

    /// Register a plugin
    pub fn register(&mut self, plugin: Box<dyn Plugin>) {
        let name = plugin.name().to_string();
        info!("Registering plugin: {}", name);
        self.plugins.insert(name, plugin);
    }

    /// Unload a plugin
    pub async fn unload(&mut self, name: &str) -> anyhow::Result<()> {
        if let Some(mut plugin) = self.plugins.remove(name) {
            info!("Unloading plugin: {}", name);
            plugin.shutdown().await?;
        }
        Ok(())
    }

    /// Execute a hook
    pub async fn execute_hook(&mut self, hook_type: HookType, data: HookData) -> HookResult {
        let plugin_names = self.hooks.get(&hook_type).cloned().unwrap_or_default();
        
        let mut current_data = data;
        
        for plugin_name in plugin_names {
            if let Some(plugin) = self.plugins.get_mut(&plugin_name) {
                match plugin.on_hook(hook_type, current_data.clone()).await {
                    HookResult::Continue => continue,
                    HookResult::Cancel { reason } => {
                        warn!("Plugin {} cancelled operation: {}", plugin_name, reason);
                        return HookResult::Cancel { reason };
                    }
                    HookResult::Modified(new_data) => {
                        current_data = new_data;
                    }
                    HookResult::Error(e) => {
                        error!("Plugin {} error: {}", plugin_name, e);
                    }
                }
            }
        }
        
        HookResult::Continue
    }

    /// Initialize all plugins
    pub async fn init_all(&mut self) -> anyhow::Result<()> {
        for (name, plugin) in &mut self.plugins {
            let context = PluginContext {
                config: HashMap::new(),
                data_dir: self.plugin_dir.join(name),
                logger: PluginLogger::new(name),
            };
            
            if let Err(e) = plugin.init(context).await {
                error!("Failed to initialize plugin {}: {}", name, e);
            }
        }
        
        // Execute startup hook
        self.execute_hook(HookType::OnStartup, HookData::None).await;
        
        Ok(())
    }

    /// Shutdown all plugins
    pub async fn shutdown_all(&mut self) -> anyhow::Result<()> {
        // Execute shutdown hook
        self.execute_hook(HookType::OnShutdown, HookData::None).await;
        
        for (name, plugin) in &mut self.plugins {
            if let Err(e) = plugin.shutdown().await {
                error!("Error shutting down plugin {}: {}", name, e);
            }
        }
        
        self.plugins.clear();
        Ok(())
    }

    /// Get list of loaded plugins
    pub fn list_plugins(&self) -> Vec<&str> {
        self.plugins.keys().map(|s| s.as_str()).collect()
    }

    /// Check if plugin is loaded
    pub fn is_loaded(&self, name: &str) -> bool {
        self.plugins.contains_key(name)
    }
}

impl Default for PluginManager {
    fn default() -> Self {
        Self::new("~/.config/d/plugins")
    }
}

/// Example built-in plugin
pub struct ExamplePlugin {
    name: String,
}

impl ExamplePlugin {
    pub fn new() -> Self {
        Self {
            name: "example".to_string(),
        }
    }
}

#[async_trait::async_trait]
impl Plugin for ExamplePlugin {
    fn name(&self) -> &str {
        &self.name
    }
    
    fn version(&self) -> &str {
        "0.1.0"
    }
    
    async fn init(&mut self, _context: PluginContext) -> anyhow::Result<()> {
        info!("Example plugin initialized");
        Ok(())
    }
    
    async fn shutdown(&mut self) -> anyhow::Result<()> {
        info!("Example plugin shutdown");
        Ok(())
    }
    
    async fn on_hook(&mut self, hook: HookType, data: HookData) -> HookResult {
        match hook {
            HookType::OnMessage => {
                if let HookData::Message { role, content } = &data {
                    info!("Example plugin saw message from {}: {}", role, content);
                }
            }
            _ => {}
        }
        HookResult::Continue
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_plugin_manager() {
        let manager = PluginManager::new("/tmp/test_plugins");
        assert!(manager.list_plugins().is_empty());
    }

    #[test]
    fn test_plugin_logger() {
        let logger = PluginLogger::new("test");
        logger.info("Test message");
    }

    #[tokio::test]
    async fn test_example_plugin() {
        let mut plugin = ExamplePlugin::new();
        assert_eq!(plugin.name(), "example");
        assert_eq!(plugin.version(), "0.1.0");
        
        let context = PluginContext {
            config: HashMap::new(),
            data_dir: PathBuf::from("/tmp"),
            logger: PluginLogger::new("test"),
        };
        
        plugin.init(context).await.unwrap();
        
        let result = plugin.on_hook(
            HookType::OnMessage,
            HookData::Message {
                role: "user".to_string(),
                content: "Hello".to_string(),
            }
        ).await;
        
        assert!(matches!(result, HookResult::Continue));
        
        plugin.shutdown().await.unwrap();
    }
}
