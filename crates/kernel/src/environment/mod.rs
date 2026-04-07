//! Environment detection and analysis
//!
//! Provides:
//! - OS detection
//! - Shell detection
//! - Terminal capabilities
//! - System resource detection

use std::collections::HashMap;
use std::env;
use std::path::PathBuf;

/// Operating system type
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OperatingSystem {
    Windows,
    Linux,
    MacOS,
    FreeBSD,
    Other(&'static str),
}

impl std::fmt::Display for OperatingSystem {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            OperatingSystem::Windows => write!(f, "Windows"),
            OperatingSystem::Linux => write!(f, "Linux"),
            OperatingSystem::MacOS => write!(f, "macOS"),
            OperatingSystem::FreeBSD => write!(f, "FreeBSD"),
            OperatingSystem::Other(s) => write!(f, "{}", s),
        }
    }
}

/// Shell type
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Shell {
    Bash(PathBuf),
    Zsh(PathBuf),
    Fish(PathBuf),
    PowerShell(PathBuf),
    Cmd(PathBuf),
    Sh(PathBuf),
    Other(String, PathBuf),
}

impl Shell {
    /// Get shell name
    pub fn name(&self) -> &str {
        match self {
            Shell::Bash(_) => "bash",
            Shell::Zsh(_) => "zsh",
            Shell::Fish(_) => "fish",
            Shell::PowerShell(_) => "powershell",
            Shell::Cmd(_) => "cmd",
            Shell::Sh(_) => "sh",
            Shell::Other(n, _) => n,
        }
    }

    /// Get shell path
    pub fn path(&self) -> &PathBuf {
        match self {
            Shell::Bash(p) => p,
            Shell::Zsh(p) => p,
            Shell::Fish(p) => p,
            Shell::PowerShell(p) => p,
            Shell::Cmd(p) => p,
            Shell::Sh(p) => p,
            Shell::Other(_, p) => p,
        }
    }

    /// Get configuration file
    pub fn config_file(&self) -> Option<PathBuf> {
        let home = dirs::home_dir()?;
        
        match self {
            Shell::Bash(_) => Some(home.join(".bashrc")),
            Shell::Zsh(_) => Some(home.join(".zshrc")),
            Shell::Fish(_) => Some(home.join(".config/fish/config.fish")),
            Shell::PowerShell(_) => {
                #[cfg(windows)]
                {
                    Some(home.join("Documents/PowerShell/Microsoft.PowerShell_profile.ps1"))
                }
                #[cfg(not(windows))]
                {
                    None
                }
            }
            _ => None,
        }
    }
}

/// Terminal capabilities
#[derive(Debug, Clone, Default)]
pub struct TerminalCapabilities {
    /// Supports colors
    pub colors: bool,
    /// Number of colors supported
    pub color_count: u16,
    /// Supports Unicode
    pub unicode: bool,
    /// Terminal width
    pub width: u16,
    /// Terminal height
    pub height: u16,
    /// Supports true color (24-bit)
    pub true_color: bool,
    /// Supports hyperlinks
    pub hyperlinks: bool,
}

/// Environment information
#[derive(Debug, Clone)]
pub struct EnvironmentInfo {
    /// Operating system
    pub os: OperatingSystem,
    /// OS version
    pub os_version: String,
    /// Architecture
    pub arch: String,
    /// Current shell
    pub shell: Option<Shell>,
    /// Terminal capabilities
    pub terminal: TerminalCapabilities,
    /// Desktop environment
    pub desktop: Option<String>,
    /// Environment variables
    pub env_vars: HashMap<String, String>,
}

impl EnvironmentInfo {
    /// Detect current environment
    pub fn detect() -> Self {
        Self {
            os: detect_os(),
            os_version: detect_os_version(),
            arch: detect_arch(),
            shell: detect_shell(),
            terminal: detect_terminal(),
            desktop: detect_desktop(),
            env_vars: env::vars().collect(),
        }
    }

    /// Check if running in CI
    pub fn is_ci(&self) -> bool {
        self.env_vars.contains_key("CI")
            || self.env_vars.contains_key("CONTINUOUS_INTEGRATION")
            || self.env_vars.contains_key("GITHUB_ACTIONS")
            || self.env_vars.contains_key("GITLAB_CI")
            || self.env_vars.contains_key("TRAVIS")
    }

    /// Check if running in container
    pub fn is_container(&self) -> bool {
        // Check for Docker
        if PathBuf::from("/.dockerenv").exists() {
            return true;
        }
        
        // Check cgroup
        if let Ok(cgroup) = std::fs::read_to_string("/proc/1/cgroup") {
            if cgroup.contains("docker") || cgroup.contains("containerd") {
                return true;
            }
        }
        
        false
    }

    /// Check if running on WSL
    pub fn is_wsl(&self) -> bool {
        self.os == OperatingSystem::Linux && 
            std::fs::read_to_string("/proc/version")
                .map(|v| v.to_lowercase().contains("microsoft"))
                .unwrap_or(false)
    }

    /// Get home directory
    pub fn home_dir(&self) -> Option<PathBuf> {
        dirs::home_dir()
    }

    /// Get config directory
    pub fn config_dir(&self) -> Option<PathBuf> {
        dirs::config_dir()
    }

    /// Get cache directory
    pub fn cache_dir(&self) -> Option<PathBuf> {
        dirs::cache_dir()
    }

    /// Get data directory
    pub fn data_dir(&self) -> Option<PathBuf> {
        dirs::data_dir()
    }
}

/// Detect operating system
fn detect_os() -> OperatingSystem {
    #[cfg(target_os = "windows")]
    return OperatingSystem::Windows;
    
    #[cfg(target_os = "linux")]
    return OperatingSystem::Linux;
    
    #[cfg(target_os = "macos")]
    return OperatingSystem::MacOS;
    
    #[cfg(target_os = "freebsd")]
    return OperatingSystem::FreeBSD;
    
    #[cfg(not(any(target_os = "windows", target_os = "linux", target_os = "macos", target_os = "freebsd")))]
    return OperatingSystem::Other("unknown");
}

/// Detect OS version
fn detect_os_version() -> String {
    #[cfg(target_os = "macos")]
    {
        if let Ok(output) = std::process::Command::new("sw_vers")
            .arg("-productVersion")
            .output()
        {
            if output.status.success() {
                return String::from_utf8_lossy(&output.stdout).trim().to_string();
            }
        }
    }
    
    #[cfg(target_os = "linux")]
    {
        if let Ok(release) = std::fs::read_to_string("/etc/os-release") {
            for line in release.lines() {
                if line.starts_with("PRETTY_NAME=") {
                    return line[12..].trim_matches('"').to_string();
                }
            }
        }
    }
    
    #[cfg(target_os = "windows")]
    {
        if let Ok(output) = std::process::Command::new("cmd")
            .args(["/C", "ver"])
            .output()
        {
            return String::from_utf8_lossy(&output.stdout).trim().to_string();
        }
    }
    
    "unknown".to_string()
}

/// Detect architecture
fn detect_arch() -> String {
    std::env::consts::ARCH.to_string()
}

/// Detect current shell
fn detect_shell() -> Option<Shell> {
    // Try SHELL env var
    if let Ok(shell_path) = env::var("SHELL") {
        let path = PathBuf::from(&shell_path);
        let name = path.file_name()?.to_str()?;
        
        return Some(match name {
            "bash" => Shell::Bash(path),
            "zsh" => Shell::Zsh(path),
            "fish" => Shell::Fish(path),
            "sh" => Shell::Sh(path),
            "powershell" | "pwsh" => Shell::PowerShell(path),
            _ => Shell::Other(name.to_string(), path),
        });
    }
    
    // Windows
    #[cfg(windows)]
    {
        if env::var("PSModulePath").is_ok() {
            return Some(Shell::PowerShell(PathBuf::from("powershell")));
        }
        return Some(Shell::Cmd(PathBuf::from("cmd")));
    }
    
    None
}

/// Detect terminal capabilities
fn detect_terminal() -> TerminalCapabilities {
    let mut caps = TerminalCapabilities::default();
    
    // Check TERM
    if let Ok(term) = env::var("TERM") {
        caps.colors = !term.contains("dumb");
        
        if term.contains("256color") {
            caps.color_count = 256;
        } else if term.contains("color") {
            caps.color_count = 8;
        }
        
        caps.true_color = term.contains("truecolor") || term.contains("24bit");
    }
    
    // Check COLORTERM
    if let Ok(colorterm) = env::var("COLORTERM") {
        if colorterm.contains("truecolor") || colorterm.contains("24bit") {
            caps.true_color = true;
            caps.color_count = 256;
        }
    }
    
    // Check for Unicode support
    caps.unicode = env::var("LANG")
        .or_else(|_| env::var("LC_ALL"))
        .map(|l| l.to_lowercase().contains("utf-8") || l.to_lowercase().contains("utf8"))
        .unwrap_or(false);
    
    // Try to get terminal size
    if let Some((w, h)) = term_size::dimensions() {
        caps.width = w as u16;
        caps.height = h as u16;
    }
    
    // Check for hyperlink support (modern terminals)
    if let Ok(vte_version) = env::var("VTE_VERSION") {
        // VTE-based terminals (GNOME Terminal, etc.)
        caps.hyperlinks = vte_version.parse::<u32>().unwrap_or(0) >= 5102;
    }
    
    caps
}

/// Detect desktop environment
fn detect_desktop() -> Option<String> {
    env::var("XDG_CURRENT_DESKTOP")
        .or_else(|_| env::var("DESKTOP_SESSION"))
        .or_else(|_| env::var("DESKTOP"))
        .ok()
}

/// System resources
#[derive(Debug, Clone)]
pub struct SystemResources {
    /// Total memory in bytes
    pub total_memory: u64,
    /// Available memory in bytes
    pub available_memory: u64,
    /// CPU count
    pub cpu_count: usize,
    /// CPU usage percentage
    pub cpu_usage: f32,
}

impl SystemResources {
    /// Get system resources
    pub fn get() -> Option<Self> {
        // This is a simplified version - in production you'd use sysinfo crate
        Some(Self {
            total_memory: 0,
            available_memory: 0,
            cpu_count: num_cpus::get(),
            cpu_usage: 0.0,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_detect_os() {
        let os = detect_os();
        assert!(!matches!(os, OperatingSystem::Other(_)));
    }

    #[test]
    fn test_environment_info() {
        let info = EnvironmentInfo::detect();
        assert!(!info.arch.is_empty());
    }

    #[test]
    fn test_is_ci() {
        let mut info = EnvironmentInfo::detect();
        info.env_vars.insert("CI".to_string(), "true".to_string());
        assert!(info.is_ci());
    }

    #[test]
    fn test_shell_name() {
        let bash = Shell::Bash(PathBuf::from("/bin/bash"));
        assert_eq!(bash.name(), "bash");
        
        let zsh = Shell::Zsh(PathBuf::from("/bin/zsh"));
        assert_eq!(zsh.name(), "zsh");
    }
}
