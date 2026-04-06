//! Cmdlet system for scriptable commands
//!
//! Provides:
//! - Named command sequences
//! - Parameterized scripts
//! - Command aliases
//! - Batch operations

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use tracing::{info, warn, error};

/// Cmdlet definition
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Cmdlet {
    /// Name of the cmdlet
    pub name: String,
    /// Description
    pub description: String,
    /// Command sequence
    pub commands: Vec<String>,
    /// Parameters
    pub parameters: Vec<Parameter>,
    /// Working directory
    #[serde(skip_serializing_if = "Option::is_none")]
    pub working_dir: Option<String>,
    /// Environment variables
    #[serde(skip_serializing_if = "Option::is_none")]
    pub env: Option<HashMap<String, String>>,
}

/// Parameter definition
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Parameter {
    pub name: String,
    pub description: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default: Option<String>,
    pub required: bool,
}

impl Cmdlet {
    /// Create a new cmdlet
    pub fn new(name: impl Into<String>, description: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            description: description.into(),
            commands: Vec::new(),
            parameters: Vec::new(),
            working_dir: None,
            env: None,
        }
    }

    /// Add a command
    pub fn with_command(mut self, cmd: impl Into<String>) -> Self {
        self.commands.push(cmd.into());
        self
    }

    /// Add a parameter
    pub fn with_parameter(mut self, param: Parameter) -> Self {
        self.parameters.push(param);
        self
    }

    /// Set working directory
    pub fn with_working_dir(mut self, dir: impl Into<String>) -> Self {
        self.working_dir = Some(dir.into());
        self
    }

    /// Execute the cmdlet with given arguments
    pub async fn execute(&self, args: &HashMap<String, String>) -> anyhow::Result<Vec<String>> {
        info!("Executing cmdlet: {}", self.name);

        // Validate required parameters
        for param in &self.parameters {
            if param.required && !args.contains_key(&param.name) && param.default.is_none() {
                anyhow::bail!("Missing required parameter: {}", param.name);
            }
        }

        let mut results = Vec::new();

        for template in &self.commands {
            // Substitute parameters
            let command = self.substitute_params(template, args);
            info!("Running: {}", command);
            
            // Execute (placeholder - would actually run)
            results.push(command);
        }

        Ok(results)
    }

    /// Substitute parameters in template
    fn substitute_params(&self, template: &str, args: &HashMap<String, String>) -> String {
        let mut result = template.to_string();

        for param in &self.parameters {
            let value = args
                .get(&param.name)
                .cloned()
                .or_else(|| param.default.clone())
                .unwrap_or_default();

            let placeholder = format!("${{{}}}", param.name);
            result = result.replace(&placeholder, &value);
        }

        result
    }
}

/// Cmdlet registry
pub struct CmdletRegistry {
    cmdlets: HashMap<String, Cmdlet>,
    config_dir: PathBuf,
}

impl CmdletRegistry {
    /// Create a new registry
    pub fn new(config_dir: impl Into<PathBuf>) -> Self {
        Self {
            cmdlets: HashMap::new(),
            config_dir: config_dir.into(),
        }
    }

    /// Register a cmdlet
    pub fn register(&mut self, cmdlet: Cmdlet) {
        info!("Registering cmdlet: {}", cmdlet.name);
        self.cmdlets.insert(cmdlet.name.clone(), cmdlet);
    }

    /// Get a cmdlet by name
    pub fn get(&self, name: &str) -> Option<&Cmdlet> {
        self.cmdlets.get(name)
    }

    /// List all cmdlets
    pub fn list(&self) -> Vec<&Cmdlet> {
        self.cmdlets.values().collect()
    }

    /// Load cmdlets from directory
    pub async fn load_from_dir(&mut self, dir: &Path) -> anyhow::Result<()> {
        if !dir.exists() {
            return Ok(());
        }

        let mut entries = tokio::fs::read_dir(dir).await?;

        while let Some(entry) = entries.next_entry().await? {
            let path = entry.path();
            if path.extension().map(|e| e == "toml").unwrap_or(false) {
                if let Err(e) = self.load_cmdlet(&path).await {
                    warn!("Failed to load cmdlet from {:?}: {}", path, e);
                }
            }
        }

        info!("Loaded {} cmdlets", self.cmdlets.len());
        Ok(())
    }

    /// Load a single cmdlet
    async fn load_cmdlet(&mut self, path: &Path) -> anyhow::Result<()> {
        let content = tokio::fs::read_to_string(path).await?;
        let cmdlet: Cmdlet = toml::from_str(&content)?;
        self.register(cmdlet);
        Ok(())
    }

    /// Save cmdlet to file
    pub async fn save_cmdlet(&self, cmdlet: &Cmdlet) -> anyhow::Result<()> {
        let path = self.config_dir.join(format!("{}.toml", cmdlet.name));
        tokio::fs::create_dir_all(&self.config_dir).await?;
        
        let content = toml::to_string_pretty(cmdlet)?;
        tokio::fs::write(&path, content).await?;
        
        info!("Saved cmdlet to {:?}", path);
        Ok(())
    }

    /// Remove a cmdlet
    pub fn remove(&mut self, name: &str) -> Option<Cmdlet> {
        self.cmdlets.remove(name)
    }

    /// Execute a cmdlet by name
    pub async fn execute(
        &self,
        name: &str,
        args: &HashMap<String, String>,
    ) -> anyhow::Result<Vec<String>> {
        let cmdlet = self
            .cmdlets
            .get(name)
            .ok_or_else(|| anyhow::anyhow!("Cmdlet not found: {}", name))?;

        cmdlet.execute(args).await
    }
}

impl Default for CmdletRegistry {
    fn default() -> Self {
        let config_dir = dirs::config_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("d")
            .join("cmdlets");
        
        Self::new(config_dir)
    }
}

/// Built-in cmdlets
pub fn builtin_cmdlets() -> Vec<Cmdlet> {
    vec![
        Cmdlet::new("status", "Show project status")
            .with_command("git status")
            .with_command("git log --oneline -5"),
        
        Cmdlet::new("clean", "Clean build artifacts")
            .with_command("cargo clean")
            .with_command("echo 'Build artifacts cleaned'"),
        
        Cmdlet::new("test-all", "Run all tests")
            .with_command("cargo test --workspace"),
        
        Cmdlet::new("fmt-check", "Format and check code")
            .with_command("cargo fmt")
            .with_command("cargo clippy --all-targets"),
    ]
}

/// Cmdlet runner
pub struct CmdletRunner {
    registry: CmdletRegistry,
}

impl CmdletRunner {
    /// Create a new runner
    pub fn new(registry: CmdletRegistry) -> Self {
        Self { registry }
    }

    /// Run a cmdlet
    pub async fn run(
        &self,
        name: &str,
        args: Vec<String>,
    ) -> anyhow::Result<Vec<String>> {
        let cmdlet = self
            .registry
            .get(name)
            .ok_or_else(|| anyhow::anyhow!("Unknown cmdlet: {}", name))?;

        // Parse arguments
        let mut arg_map = HashMap::new();
        for arg in args {
            if let Some((key, value)) = arg.split_once('=') {
                arg_map.insert(key.to_string(), value.to_string());
            }
        }

        cmdlet.execute(&arg_map).await
    }

    /// Get help for a cmdlet
    pub fn help(&self, name: &str) -> Option<String> {
        let cmdlet = self.registry.get(name)?;
        
        let mut help = format!("{} - {}\n\n", cmdlet.name, cmdlet.description);
        
        if !cmdlet.parameters.is_empty() {
            help.push_str("Parameters:\n");
            for param in &cmdlet.parameters {
                let required = if param.required { "(required)" } else { "" };
                let default = param.default.as_ref()
                    .map(|d| format!(" [default: {}]", d))
                    .unwrap_or_default();
                help.push_str(&format!(
                    "  ${{{}}} - {}{}{}\n",
                    param.name, param.description, required, default
                ));
            }
        }
        
        help.push_str("\nCommands:\n");
        for (i, cmd) in cmdlet.commands.iter().enumerate() {
            help.push_str(&format!("  {}. {}\n", i + 1, cmd));
        }
        
        Some(help)
    }

    /// List all available cmdlets
    pub fn list(&self) -> String {
        let mut output = String::from("Available cmdlets:\n");
        for cmdlet in self.registry.list() {
            output.push_str(&format!("  {} - {}\n", cmdlet.name, cmdlet.description));
        }
        output
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cmdlet_creation() {
        let cmdlet = Cmdlet::new("test", "Test cmdlet")
            .with_command("echo hello")
            .with_parameter(Parameter {
                name: "name".to_string(),
                description: "Name to greet".to_string(),
                default: Some("World".to_string()),
                required: false,
            });

        assert_eq!(cmdlet.name, "test");
        assert_eq!(cmdlet.commands.len(), 1);
    }

    #[test]
    fn test_param_substitution() {
        let cmdlet = Cmdlet::new("greet", "Greet someone")
            .with_command("echo Hello, ${name}!")
            .with_parameter(Parameter {
                name: "name".to_string(),
                description: "Name".to_string(),
                default: Some("World".to_string()),
                required: false,
            });

        let mut args = HashMap::new();
        args.insert("name".to_string(), "Alice".to_string());

        let result = cmdlet.substitute_params(&cmdlet.commands[0], &args);
        assert_eq!(result, "echo Hello, Alice!");
    }

    #[test]
    fn test_registry() {
        let mut registry = CmdletRegistry::default();
        let cmdlet = Cmdlet::new("test", "Test");
        registry.register(cmdlet);
        
        assert!(registry.get("test").is_some());
        assert_eq!(registry.list().len(), 1);
    }
}
