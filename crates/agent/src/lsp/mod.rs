//! Language Server Protocol (LSP) client for code intelligence
//!
//! Provides integration with language servers for:
//! - Code completion
//! - Go to definition
//! - Find references
//! - Hover information
//! - Symbol search
//! - Code actions

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicI64, Ordering};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};
use tokio::sync::{mpsc, oneshot, Mutex};
use tracing::{debug, error, info, warn};

mod client;
mod types;

pub use client::{LspClient, LspClientHandle};
pub use types::*;

/// LSP client error
#[derive(Debug, thiserror::Error)]
pub enum LspError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("LSP error: {code} - {message}")]
    Lsp { code: i32, message: String },
    #[error("Server not initialized")]
    NotInitialized,
    #[error("Request timeout")]
    Timeout,
    #[error("Request cancelled")]
    Cancelled,
    #[error("Server process error: {0}")]
    ServerError(String),
}

/// LSP client builder
pub struct LspClientBuilder {
    command: String,
    args: Vec<String>,
    workspace: Option<PathBuf>,
    root_uri: Option<String>,
}

impl LspClientBuilder {
    /// Create a new builder with language server command
    pub fn new(command: impl Into<String>) -> Self {
        Self {
            command: command.into(),
            args: Vec::new(),
            workspace: None,
            root_uri: None,
        }
    }

    /// Add command argument
    pub fn arg(mut self, arg: impl Into<String>) -> Self {
        self.args.push(arg.into());
        self
    }

    /// Set workspace root
    pub fn workspace(mut self, path: impl AsRef<Path>) -> Self {
        self.workspace = Some(path.as_ref().to_path_buf());
        self
    }

    /// Build and start the client
    pub async fn build(self) -> Result<LspClient, LspError> {
        let workspace = self.workspace.unwrap_or_else(|| std::env::current_dir().unwrap());
        let root_uri = format!("file://{}", workspace.display());

        LspClient::new(&self.command, &self.args, &root_uri).await
    }
}

/// Language server configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LanguageServerConfig {
    pub language_id: String,
    pub command: String,
    pub args: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub initialization_options: Option<serde_json::Value>,
}

impl LanguageServerConfig {
    /// Get configuration for a language
    pub fn for_language(language: &str) -> Option<Self> {
        match language {
            "rust" => Some(Self {
                language_id: "rust".to_string(),
                command: "rust-analyzer".to_string(),
                args: vec![],
                initialization_options: None,
            }),
            "python" => Some(Self {
                language_id: "python".to_string(),
                command: "pylsp".to_string(),
                args: vec![],
                initialization_options: None,
            }),
            "typescript" | "javascript" => Some(Self {
                language_id: language.to_string(),
                command: "typescript-language-server".to_string(),
                args: vec!["--stdio".to_string()],
                initialization_options: None,
            }),
            "go" => Some(Self {
                language_id: "go".to_string(),
                command: "gopls".to_string(),
                args: vec![],
                initialization_options: None,
            }),
            _ => None,
        }
    }
}

/// LSP server manager for multiple language servers
pub struct LspServerManager {
    clients: Mutex<HashMap<String, LspClientHandle>>,
    configs: HashMap<String, LanguageServerConfig>,
}

impl LspServerManager {
    /// Create a new manager
    pub fn new() -> Self {
        let mut configs = HashMap::new();
        
        // Register default configs
        for lang in &["rust", "python", "typescript", "javascript", "go"] {
            if let Some(config) = LanguageServerConfig::for_language(lang) {
                configs.insert(lang.to_string(), config);
            }
        }

        Self {
            clients: Mutex::new(HashMap::new()),
            configs,
        }
    }

    /// Register a language server configuration
    pub fn register(&mut self, config: LanguageServerConfig) {
        self.configs.insert(config.language_id.clone(), config);
    }

    /// Start a language server for a language
    pub async fn start(
        &self,
        language: &str,
        workspace: impl AsRef<Path>,
    ) -> Result<LspClientHandle, LspError> {
        let config = self
            .configs
            .get(language)
            .cloned()
            .ok_or_else(|| LspError::ServerError(format!("No config for language: {}", language)))?;

        let client = LspClientBuilder::new(&config.command)
            .workspace(workspace)
            .build()
            .await?;

        let handle = client.spawn().await?;

        let mut clients = self.clients.lock().await;
        clients.insert(language.to_string(), handle.clone());

        Ok(handle)
    }

    /// Get client for a language
    pub async fn get(&self, language: &str) -> Option<LspClientHandle> {
        let clients = self.clients.lock().await;
        clients.get(language).cloned()
    }

    /// Check if server is running for language
    pub async fn is_running(&self, language: &str) -> bool {
        self.get(language).await.is_some()
    }

    /// Shutdown all servers
    pub async fn shutdown_all(&self) -> Result<(), LspError> {
        let mut clients = self.clients.lock().await;
        for (lang, handle) in clients.drain() {
            info!("Shutting down {} language server", lang);
            let _ = handle.shutdown().await;
        }
        Ok(())
    }
}

impl Default for LspServerManager {
    fn default() -> Self {
        Self::new()
    }
}

/// Utility to detect language from file extension
pub fn detect_language(path: &Path) -> Option<String> {
    let ext = path.extension()?.to_str()?;
    match ext {
        "rs" => Some("rust".to_string()),
        "py" => Some("python".to_string()),
        "js" => Some("javascript".to_string()),
        "ts" => Some("typescript".to_string()),
        "go" => Some("go".to_string()),
        "java" => Some("java".to_string()),
        "c" | "h" => Some("c".to_string()),
        "cpp" | "hpp" | "cc" => Some("cpp".to_string()),
        "zig" => Some("zig".to_string()),
        "rb" => Some("ruby".to_string()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_detect_language() {
        assert_eq!(detect_language(Path::new("test.rs")), Some("rust".to_string()));
        assert_eq!(detect_language(Path::new("test.py")), Some("python".to_string()));
        assert_eq!(detect_language(Path::new("test.ts")), Some("typescript".to_string()));
        assert_eq!(detect_language(Path::new("test.go")), Some("go".to_string()));
        assert_eq!(detect_language(Path::new("test.unknown")), None);
    }

    #[test]
    fn test_language_server_config() {
        let rust = LanguageServerConfig::for_language("rust").unwrap();
        assert_eq!(rust.language_id, "rust");
        assert_eq!(rust.command, "rust-analyzer");

        let python = LanguageServerConfig::for_language("python").unwrap();
        assert_eq!(python.language_id, "python");
        assert_eq!(python.command, "pylsp");
    }

    #[tokio::test]
    async fn test_lsp_manager() {
        let manager = LspServerManager::new();
        assert!(!manager.is_running("rust").await);
        
        // Test config registration
        let mut manager = LspServerManager::new();
        manager.register(LanguageServerConfig {
            language_id: "test".to_string(),
            command: "echo".to_string(),
            args: vec![],
            initialization_options: None,
        });
        assert!(manager.configs.contains_key("test"));
    }
}
