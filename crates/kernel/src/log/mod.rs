//! Logging utilities
//!
//! Provides:
//! - File-based logging with rotation
//! - Log level management
//! - Structured logging support

use std::path::PathBuf;
use tracing::info;

/// Log configuration
#[derive(Debug, Clone)]
pub struct LogConfig {
    /// Log directory
    pub log_dir: PathBuf,
    /// Log file name
    pub log_file: String,
    /// Maximum log file size in MB
    pub max_size_mb: u64,
    /// Number of log files to keep
    pub max_files: usize,
    /// Log level
    pub level: LogLevel,
    /// Also log to console
    pub console: bool,
}

impl Default for LogConfig {
    fn default() -> Self {
        Self {
            log_dir: dirs::data_dir().unwrap_or_else(|| PathBuf::from(".")).join("d").join("logs"),
            log_file: "d.log".to_string(),
            max_size_mb: 10,
            max_files: 5,
            level: LogLevel::Info,
            console: true,
        }
    }
}

/// Log levels
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogLevel {
    Error,
    Warn,
    Info,
    Debug,
    Trace,
}

impl LogLevel {
    pub fn as_str(&self) -> &'static str {
        match self {
            LogLevel::Error => "ERROR",
            LogLevel::Warn => "WARN",
            LogLevel::Info => "INFO",
            LogLevel::Debug => "DEBUG",
            LogLevel::Trace => "TRACE",
        }
    }

    pub fn from_str(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "error" => Some(LogLevel::Error),
            "warn" => Some(LogLevel::Warn),
            "info" => Some(LogLevel::Info),
            "debug" => Some(LogLevel::Debug),
            "trace" => Some(LogLevel::Trace),
            _ => None,
        }
    }
}

/// Log manager
pub struct LogManager {
    config: LogConfig,
}

impl LogManager {
    /// Create a new log manager
    pub fn new(config: LogConfig) -> Self {
        Self { config }
    }

    /// Initialize logging
    pub fn init(&self) -> anyhow::Result<()> {
        // Create log directory if needed
        std::fs::create_dir_all(&self.config.log_dir)?;

        let log_path = self.config.log_dir.join(&self.config.log_file);

        // Build subscriber
        let subscriber = tracing_subscriber::fmt()
            .with_env_filter(
                tracing_subscriber::EnvFilter::try_from_default_env()
                    .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new(self.config.level.as_str()))
            )
            .with_target(false)
            .with_thread_ids(false)
            .with_file(false)
            .with_line_number(false);

        if self.config.console {
            // Log to both console and file
            subscriber
                .with_writer(move || {
                    let file = std::fs::OpenOptions::new()
                        .create(true)
                        .append(true)
                        .open(&log_path)
                        .unwrap();
                    std::io::BufWriter::new(file)
                })
                .try_init()
                .ok();
        } else {
            // Log to file only
            subscriber
                .with_writer(move || {
                    let file = std::fs::OpenOptions::new()
                        .create(true)
                        .append(true)
                        .open(&log_path)
                        .unwrap();
                    std::io::BufWriter::new(file)
                })
                .without_time()
                .try_init()
                .ok();
        }

        info!("Logging initialized at level: {}", self.config.level.as_str());
        Ok(())
    }

    /// Rotate log files if needed
    pub fn rotate_if_needed(&self) -> anyhow::Result<()> {
        let log_path = self.config.log_dir.join(&self.config.log_file);
        
        if !log_path.exists() {
            return Ok(());
        }

        let metadata = std::fs::metadata(&log_path)?;
        let size_mb = metadata.len() / (1024 * 1024);

        if size_mb >= self.config.max_size_mb {
            self.rotate()?;
        }

        Ok(())
    }

    /// Rotate log files
    fn rotate(&self) -> anyhow::Result<()> {
        let base_path = self.config.log_dir.join(&self.config.log_file);

        // Remove oldest log if at max
        let oldest = self.config.log_dir.join(format!("{}.{}.{}", 
            self.config.log_file, 
            self.config.max_files - 1,
            "old"
        ));
        if oldest.exists() {
            std::fs::remove_file(&oldest)?;
        }

        // Shift existing logs
        for i in (1..self.config.max_files).rev() {
            let old_path = self.config.log_dir.join(format!("{}.{}.{}",
                self.config.log_file,
                i - 1,
                "old"
            ));
            let new_path = self.config.log_dir.join(format!("{}.{}.{}",
                self.config.log_file,
                i,
                "old"
            ));
            
            if old_path.exists() {
                std::fs::rename(&old_path, &new_path)?;
            }
        }

        // Move current log to .0.old
        if base_path.exists() {
            let new_path = self.config.log_dir.join(format!("{}.{}.{}",
                self.config.log_file,
                0,
                "old"
            ));
            std::fs::rename(&base_path, &new_path)?;
        }

        info!("Log files rotated");
        Ok(())
    }

    /// Get current log file path
    pub fn current_log_path(&self) -> PathBuf {
        self.config.log_dir.join(&self.config.log_file)
    }

    /// List log files
    pub fn list_logs(&self) -> Vec<PathBuf> {
        let mut logs = Vec::new();
        
        if let Ok(entries) = std::fs::read_dir(&self.config.log_dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().map(|e| e == "log" || e == "old").unwrap_or(false) {
                    logs.push(path);
                }
            }
        }
        
        logs.sort();
        logs
    }
}

impl Default for LogManager {
    fn default() -> Self {
        Self::new(LogConfig::default())
    }
}

/// Initialize default logging
pub fn init_default() {
    let manager = LogManager::default();
    manager.init().ok();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_log_level() {
        assert_eq!(LogLevel::Info.as_str(), "INFO");
        assert_eq!(LogLevel::from_str("debug"), Some(LogLevel::Debug));
        assert_eq!(LogLevel::from_str("unknown"), None);
    }

    #[test]
    fn test_log_config_default() {
        let config = LogConfig::default();
        assert_eq!(config.max_size_mb, 10);
        assert_eq!(config.max_files, 5);
    }
}
