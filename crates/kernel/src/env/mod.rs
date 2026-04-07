//! Environment configuration and management
//!
//! Provides:
//! - Environment variable handling
//! - Configuration profiles
//! - Secret management
//! - Environment validation

use std::collections::HashMap;
use std::env;
use std::path::PathBuf;
use tracing::info;

/// Environment variable source
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    /// From environment
    Environment,
    /// From .env file
    DotEnv,
    /// From config file
    ConfigFile,
    /// Default value
    Default,
    /// Explicitly set
    Explicit,
}

/// Environment variable entry
#[derive(Debug, Clone)]
pub struct EnvVar {
    /// Variable name
    pub name: String,
    /// Current value
    pub value: String,
    /// Where it came from
    pub source: Source,
    /// Whether it's a secret
    pub is_secret: bool,
    /// Description
    pub description: Option<String>,
}

impl EnvVar {
    /// Create new environment variable
    pub fn new(name: impl Into<String>, value: impl Into<String>, source: Source) -> Self {
        Self {
            name: name.into(),
            value: value.into(),
            source,
            is_secret: false,
            description: None,
        }
    }

    /// Mark as secret
    pub fn secret(mut self) -> Self {
        self.is_secret = true;
        self
    }

    /// Add description
    pub fn with_description(mut self, desc: impl Into<String>) -> Self {
        self.description = Some(desc.into());
        self
    }

    /// Get masked value for display
    pub fn display_value(&self) -> String {
        if self.is_secret && !self.value.is_empty() {
            format!("{}***", &self.value[..self.value.len().min(3)])
        } else {
            self.value.clone()
        }
    }
}

/// Environment manager
pub struct Environment {
    vars: HashMap<String, EnvVar>,
    prefix: String,
}

impl Environment {
    /// Create new environment with prefix
    pub fn with_prefix(prefix: impl Into<String>) -> Self {
        Self {
            vars: HashMap::new(),
            prefix: prefix.into(),
        }
    }

    /// Load from environment variables
    pub fn load_from_env(&mut self) {
        let prefix = &self.prefix;
        
        for (key, value) in env::vars() {
            if key.starts_with(prefix) {
                let var = EnvVar::new(&key, value, Source::Environment);
                self.vars.insert(key, var);
            }
        }
        
        info!("Loaded {} vars with prefix '{}'", self.vars.len(), prefix);
    }

    /// Load from .env file
    pub fn load_dotenv(&mut self, path: impl AsRef<std::path::Path>) -> anyhow::Result<()> {
        let content = std::fs::read_to_string(path)?;
        
        for line in content.lines() {
            let line = line.trim();
            
            // Skip comments and empty lines
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            
            // Parse KEY=VALUE
            if let Some(pos) = line.find('=') {
                let key = line[..pos].trim().to_string();
                let value = line[pos + 1..].trim().to_string();
                
                // Remove quotes
                let value = value
                    .strip_prefix('"')
                    .and_then(|v| v.strip_suffix('"'))
                    .unwrap_or(&value)
                    .to_string();
                
                // Only load if it matches prefix or has no prefix restriction
                if self.prefix.is_empty() || key.starts_with(&self.prefix) {
                    let var = EnvVar::new(&key, value, Source::DotEnv);
                    self.vars.insert(key, var);
                }
            }
        }
        
        Ok(())
    }

    /// Set a variable
    pub fn set(&mut self, name: impl Into<String>, value: impl Into<String>) {
        let name = name.into();
        let var = EnvVar::new(&name, value, Source::Explicit);
        self.vars.insert(name, var);
    }

    /// Set a secret variable
    pub fn set_secret(&mut self, name: impl Into<String>, value: impl Into<String>) {
        let name = name.into();
        let var = EnvVar::new(&name, value, Source::Explicit).secret();
        self.vars.insert(name, var);
    }

    /// Get a variable
    pub fn get(&self, name: &str) -> Option<&EnvVar> {
        self.vars.get(name)
    }

    /// Get value or default
    pub fn get_or(&self, name: &str, default: impl Into<String>) -> String {
        self.get(name)
            .map(|v| v.value.clone())
            .unwrap_or_else(|| default.into())
    }

    /// Get value or empty
    pub fn get_string(&self, name: &str) -> String {
        self.get_or(name, "")
    }

    /// Get as integer
    pub fn get_int(&self, name: &str) -> Option<i64> {
        self.get(name)?.value.parse().ok()
    }

    /// Get as boolean
    pub fn get_bool(&self, name: &str) -> Option<bool> {
        let value = self.get(name)?.value.to_lowercase();
        match value.as_str() {
            "true" | "1" | "yes" | "on" => Some(true),
            "false" | "0" | "no" | "off" => Some(false),
            _ => None,
        }
    }

    /// Get as path
    pub fn get_path(&self, name: &str) -> Option<PathBuf> {
        self.get(name).map(|v| PathBuf::from(&v.value))
    }

    /// Check if variable exists
    pub fn has(&self, name: &str) -> bool {
        self.vars.contains_key(name)
    }

    /// Require a variable
    pub fn require(&self, name: &str) -> anyhow::Result<&EnvVar> {
        self.get(name)
            .ok_or_else(|| anyhow::anyhow!("Required environment variable missing: {}", name))
    }

    /// List all variables
    pub fn list(&self) -> Vec<&EnvVar> {
        self.vars.values().collect()
    }

    /// Get variables by source
    pub fn by_source(&self, source: Source) -> Vec<&EnvVar> {
        self.vars
            .values()
            .filter(|v| v.source == source)
            .collect()
    }

    /// Remove a variable
    pub fn remove(&mut self, name: &str) -> Option<EnvVar> {
        self.vars.remove(name)
    }

    /// Clear all variables
    pub fn clear(&mut self) {
        self.vars.clear();
    }

    /// Export to environment
    pub fn export(&self) {
        for (name, var) in &self.vars {
            unsafe {
                env::set_var(name, &var.value);
            }
        }
    }

    /// Get all as HashMap
    pub fn to_map(&self) -> HashMap<String, String> {
        self.vars
            .iter()
            .map(|(k, v)| (k.clone(), v.value.clone()))
            .collect()
    }
}

impl Default for Environment {
    fn default() -> Self {
        Self::with_prefix("")
    }
}

/// Configuration profile
#[derive(Debug, Clone)]
pub struct Profile {
    /// Profile name
    pub name: String,
    /// Variables in this profile
    pub vars: HashMap<String, String>,
}

impl Profile {
    /// Create new profile
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            vars: HashMap::new(),
        }
    }

    /// Set variable
    pub fn set(&mut self, name: impl Into<String>, value: impl Into<String>) {
        self.vars.insert(name.into(), value.into());
    }

    /// Apply to environment
    pub fn apply(&self, env: &mut Environment) {
        for (name, value) in &self.vars {
            env.set(name, value);
        }
    }
}

/// Profile manager
pub struct ProfileManager {
    profiles: HashMap<String, Profile>,
    active: Option<String>,
}

impl ProfileManager {
    /// Create profile manager
    pub fn new() -> Self {
        Self {
            profiles: HashMap::new(),
            active: None,
        }
    }

    /// Add profile
    pub fn add(&mut self, profile: Profile) {
        self.profiles.insert(profile.name.clone(), profile);
    }

    /// Get profile
    pub fn get(&self, name: &str) -> Option<&Profile> {
        self.profiles.get(name)
    }

    /// Activate profile
    pub fn activate(&mut self, name: impl Into<String>) -> anyhow::Result<()> {
        let name = name.into();
        if !self.profiles.contains_key(&name) {
            anyhow::bail!("Profile not found: {}", name);
        }
        self.active = Some(name);
        Ok(())
    }

    /// Get active profile
    pub fn active(&self) -> Option<&Profile> {
        self.active.as_ref().and_then(|n| self.profiles.get(n))
    }

    /// List profiles
    pub fn list(&self) -> Vec<&Profile> {
        self.profiles.values().collect()
    }
}

impl Default for ProfileManager {
    fn default() -> Self {
        Self::new()
    }
}

/// Validation rule
pub trait ValidationRule: Send + Sync {
    /// Validate environment
    fn validate(&self, env: &Environment) -> Vec<ValidationError>;
}

/// Validation error
#[derive(Debug, Clone)]
pub struct ValidationError {
    pub var_name: String,
    pub message: String,
    pub severity: Severity,
}

/// Error severity
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    Warning,
    Error,
}

/// Required variable validator
pub struct RequiredValidator {
    var_name: String,
}

impl RequiredValidator {
    /// Create required validator
    pub fn new(var_name: impl Into<String>) -> Self {
        Self {
            var_name: var_name.into(),
        }
    }
}

impl ValidationRule for RequiredValidator {
    fn validate(&self, env: &Environment) -> Vec<ValidationError> {
        let mut errors = Vec::new();
        
        if !env.has(&self.var_name) {
            errors.push(ValidationError {
                var_name: self.var_name.clone(),
                message: format!("Required variable '{}' is missing", self.var_name),
                severity: Severity::Error,
            });
        }
        
        errors
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_env_var() {
        let var = EnvVar::new("TEST_VAR", "test_value", Source::Explicit);
        assert_eq!(var.name, "TEST_VAR");
        assert_eq!(var.value, "test_value");
        assert!(!var.is_secret);

        let secret = EnvVar::new("SECRET", "hidden", Source::Explicit).secret();
        assert!(secret.is_secret);
        assert_eq!(secret.display_value(), "hid***");
    }

    #[test]
    fn test_environment() {
        let mut env = Environment::default();
        env.set("FOO", "bar");
        env.set_secret("API_KEY", "super_secret_key");

        assert_eq!(env.get_string("FOO"), "bar");
        assert!(env.get("API_KEY").unwrap().is_secret);
        assert!(env.has("FOO"));
        assert!(!env.has("MISSING"));
    }

    #[test]
    fn test_environment_types() {
        let mut env = Environment::default();
        env.set("INT_VAR", "42");
        env.set("BOOL_VAR", "true");

        assert_eq!(env.get_int("INT_VAR"), Some(42));
        assert_eq!(env.get_bool("BOOL_VAR"), Some(true));
    }

    #[test]
    fn test_profile() {
        let mut profile = Profile::new("development");
        profile.set("DEBUG", "true");
        profile.set("LOG_LEVEL", "debug");

        let mut env = Environment::default();
        profile.apply(&mut env);

        assert_eq!(env.get_string("DEBUG"), "true");
        assert_eq!(env.get_string("LOG_LEVEL"), "debug");
    }

    #[test]
    fn test_validation() {
        let mut env = Environment::default();
        env.set("PRESENT", "value");

        let validator = RequiredValidator::new("MISSING");
        let errors = validator.validate(&env);

        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0].severity, Severity::Error);
    }
}
