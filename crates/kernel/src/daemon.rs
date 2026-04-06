//! Daemon mode for background operation
//!
//! Provides:
//! - Process management (PID file)
//! - Signal handling (SIGHUP, SIGTERM, SIGINT)
//! - Background/foreground operation
//! - Client-server IPC
//! - Graceful shutdown

use crate::{config::Config, error::Result, module::Module, state::SharedState};
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::{mpsc, RwLock};
use tracing::{info, warn, error};

/// Daemon configuration
#[derive(Debug, Clone)]
pub struct DaemonConfig {
    /// PID file path
    pub pid_file: PathBuf,
    /// Working directory
    pub work_dir: PathBuf,
    /// Log file path
    pub log_file: Option<PathBuf>,
    /// Run in foreground (don't daemonize)
    pub foreground: bool,
    /// Unix socket path for client communication
    pub socket_path: PathBuf,
}

impl Default for DaemonConfig {
    fn default() -> Self {
        Self {
            pid_file: std::env::temp_dir().join("d.pid"),
            work_dir: std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
            log_file: None,
            foreground: false,
            socket_path: std::env::temp_dir().join("d.sock"),
        }
    }
}

/// Daemon control commands
#[derive(Debug, Clone)]
pub enum DaemonCommand {
    /// Reload configuration
    Reload,
    /// Shutdown gracefully
    Shutdown,
    /// Get status
    Status,
    /// Ping (health check)
    Ping,
}

/// Daemon status
#[derive(Debug, Clone)]
pub struct DaemonStatus {
    pub pid: u32,
    pub uptime: std::time::Duration,
    pub active_modules: Vec<String>,
    pub memory_usage: usize,
}

/// Main daemon struct
pub struct Daemon {
    config: Config,
    daemon_config: DaemonConfig,
    state: SharedState,
    modules: Vec<Box<dyn Module>>,
    command_rx: mpsc::Receiver<DaemonCommand>,
    command_tx: mpsc::Sender<DaemonCommand>,
    start_time: std::time::Instant,
}

impl Daemon {
    /// Create a new daemon instance
    pub fn new(config: Config, daemon_config: DaemonConfig) -> Self {
        let (command_tx, command_rx) = mpsc::channel(100);
        
        Self {
            config,
            daemon_config,
            state: Arc::new(RwLock::new(crate::state::State::default())),
            modules: Vec::new(),
            command_rx,
            command_tx,
            start_time: std::time::Instant::now(),
        }
    }

    /// Get command sender for external control
    pub fn command_sender(&self) -> mpsc::Sender<DaemonCommand> {
        self.command_tx.clone()
    }

    /// Add a module to the daemon
    pub fn with_module(mut self, module: Box<dyn Module>) -> Self {
        self.modules.push(module);
        self
    }

    /// Run the daemon
    pub async fn run(mut self) -> Result<()> {
        // Check if already running
        if let Some(pid) = self.check_pid_file().await? {
            error!("Daemon already running with PID: {}", pid);
            return Err(crate::error::Error::Io(std::io::Error::new(
                std::io::ErrorKind::AlreadyExists,
                format!("Daemon already running with PID: {}", pid),
            )));
        }

        // Daemonize if not foreground
        if !self.daemon_config.foreground {
            self.daemonize().await?;
        }

        // Write PID file
        self.write_pid_file().await?;

        // Initialize modules
        for module in &self.modules {
            module.init(self.state.clone()).await?;
        }

        // Start modules
        for module in &self.modules {
            module.start().await?;
        }

        info!("Daemon started successfully (PID: {})", std::process::id());

        // Main daemon loop
        self.main_loop().await?;

        // Cleanup
        self.shutdown().await?;
        self.remove_pid_file().await?;

        info!("Daemon shutdown complete");
        Ok(())
    }

    /// Main daemon event loop
    async fn main_loop(&mut self) -> Result<()> {
        let mut sigterm = tokio::signal::unix::signal(
            tokio::signal::unix::SignalKind::terminate()
        )?;
        let mut sighup = tokio::signal::unix::signal(
            tokio::signal::unix::SignalKind::hangup()
        )?;
        let mut sigint = tokio::signal::unix::signal(
            tokio::signal::unix::SignalKind::interrupt()
        )?;

        loop {
            tokio::select! {
                // Handle daemon commands
                Some(cmd) = self.command_rx.recv() => {
                    match cmd {
                        DaemonCommand::Reload => {
                            info!("Reloading configuration...");
                            // Reload config
                            if let Ok(new_config) = Config::load() {
                                self.config = new_config;
                                info!("Configuration reloaded");
                            }
                        }
                        DaemonCommand::Shutdown => {
                            info!("Shutdown command received");
                            break;
                        }
                        DaemonCommand::Status => {
                            let status = self.get_status().await;
                            info!("Daemon status: {:?}", status);
                        }
                        DaemonCommand::Ping => {
                            // Health check - just log
                            trace!("Ping received");
                        }
                    }
                }

                // Handle SIGTERM
                _ = sigterm.recv() => {
                    info!("SIGTERM received, shutting down...");
                    break;
                }

                // Handle SIGHUP (reload)
                _ = sighup.recv() => {
                    info!("SIGHUP received, reloading...");
                    if let Ok(new_config) = Config::load() {
                        self.config = new_config;
                        info!("Configuration reloaded");
                    }
                }

                // Handle SIGINT
                _ = sigint.recv() => {
                    if self.daemon_config.foreground {
                        info!("SIGINT received, shutting down...");
                        break;
                    }
                }
            }
        }

        Ok(())
    }

    /// Get current daemon status
    async fn get_status(&self) -> DaemonStatus {
        let active_modules: Vec<String> = self.modules.iter()
            .map(|m| m.name().to_string())
            .collect();

        DaemonStatus {
            pid: std::process::id(),
            uptime: self.start_time.elapsed(),
            active_modules,
            memory_usage: 0, // Would need sysinfo crate for real value
        }
    }

    /// Shutdown all modules
    async fn shutdown(&self) -> Result<()> {
        info!("Shutting down modules...");
        for module in &self.modules {
            if let Err(e) = module.shutdown().await {
                warn!("Error shutting down module {}: {}", module.name(), e);
            }
        }
        Ok(())
    }

    /// Check if daemon is already running
    async fn check_pid_file(&self) -> Result<Option<u32>> {
        if !self.daemon_config.pid_file.exists() {
            return Ok(None);
        }

        let pid_str = tokio::fs::read_to_string(&self.daemon_config.pid_file).await?;
        let pid: u32 = pid_str.trim().parse().unwrap_or(0);

        if pid > 0 {
            // Check if process actually exists (Unix-specific)
            #[cfg(unix)]
            {
                use std::process::Command;
                let output = Command::new("kill")
                    .args(["-0", &pid.to_string()])
                    .output()?;
                
                if output.status.success() {
                    return Ok(Some(pid));
                }
            }
        }

        // Stale PID file
        Ok(None)
    }

    /// Write PID file
    async fn write_pid_file(&self) -> Result<()> {
        let pid = std::process::id().to_string();
        tokio::fs::write(&self.daemon_config.pid_file, pid).await?;
        Ok(())
    }

    /// Remove PID file
    async fn remove_pid_file(&self) -> Result<()> {
        if self.daemon_config.pid_file.exists() {
            tokio::fs::remove_file(&self.daemon_config.pid_file).await?;
        }
        Ok(())
    }

    /// Daemonize (double fork)
    async fn daemonize(&self) -> Result<()> {
        #[cfg(unix)]
        {
            use std::process::Command;
            use std::os::unix::process::CommandExt;
            
            // First fork
            let child = unsafe {
                Command::new(std::env::current_exe()?)
                    .arg("daemon")
                    .arg("--foreground")
                    .pre_exec(|| {
                        // Create new session
                        libc::setsid();
                        Ok(())
                    })
                    .spawn()?
            };

            info!("Daemon started in background (PID: {})", child.id());
            std::process::exit(0);
        }

        #[cfg(not(unix))]
        {
            warn!("Daemonization not supported on this platform");
            Ok(())
        }
    }
}

/// Daemon client for sending commands
pub struct DaemonClient {
    socket_path: PathBuf,
}

impl DaemonClient {
    /// Create a new client
    pub fn new(socket_path: impl Into<PathBuf>) -> Self {
        Self {
            socket_path: socket_path.into(),
        }
    }

    /// Send command to daemon
    pub async fn send_command(&self, cmd: DaemonCommand) -> Result<()> {
        // Would implement Unix socket or TCP communication
        // For now, this is a placeholder
        info!("Would send command: {:?}", cmd);
        Ok(())
    }

    /// Check if daemon is running
    pub async fn is_running(&self) -> bool {
        // Check PID file
        let pid_file = std::env::temp_dir().join("d.pid");
        if let Ok(pid_str) = tokio::fs::read_to_string(pid_file).await {
            if let Ok(pid) = pid_str.trim().parse::<u32>() {
                #[cfg(unix)]
                {
                    use std::process::Command;
                    return Command::new("kill")
                        .args(["-0", &pid.to_string()])
                        .output()
                        .map(|o| o.status.success())
                        .unwrap_or(false);
                }
            }
        }
        false
    }

    /// Stop the daemon
    pub async fn stop(&self) -> Result<()> {
        let pid_file = std::env::temp_dir().join("d.pid");
        if let Ok(pid_str) = tokio::fs::read_to_string(pid_file).await {
            if let Ok(pid) = pid_str.trim().parse::<u32>() {
                #[cfg(unix)]
                {
                    use std::process::Command;
                    Command::new("kill")
                        .arg("-TERM")
                        .arg(pid.to_string())
                        .spawn()?;
                    info!("Sent TERM signal to daemon (PID: {})", pid);
                }
            }
        }
        Ok(())
    }
}

use tracing::trace;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_daemon_config_default() {
        let config = DaemonConfig::default();
        assert!(!config.foreground);
    }

    #[tokio::test]
    async fn test_daemon_client_not_running() {
        let client = DaemonClient::new("/tmp/nonexistent.sock");
        assert!(!client.is_running().await);
    }
}
