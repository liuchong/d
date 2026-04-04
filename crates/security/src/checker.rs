//! Security checker for dangerous command patterns
//!
//! Detects potentially harmful operations in shell commands and tool arguments.

/// Security check result
#[derive(Debug, Clone)]
pub struct SecurityCheck {
    pub level: SecurityLevel,
    pub message: String,
    pub suggestion: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SecurityLevel {
    Info,
    Warning,
    Dangerous,
    Critical,
}

impl SecurityLevel {
    pub fn as_str(&self) -> &'static str {
        match self {
            SecurityLevel::Info => "INFO",
            SecurityLevel::Warning => "WARNING",
            SecurityLevel::Dangerous => "DANGEROUS",
            SecurityLevel::Critical => "CRITICAL",
        }
    }
}

/// Check shell command for dangerous patterns
pub fn check_shell_command(command: &str) -> Vec<SecurityCheck> {
    let mut findings = Vec::new();
    let cmd_lower = command.to_lowercase();

    // Critical patterns - data destruction
    let critical_patterns = [
        ("rm -rf /", "This command will delete your entire filesystem!"),
        ("rm -rf ~", "This will delete your home directory!"),
        ("rm -rf /*", "This will delete system files!"),
        ("dd if=/dev/zero of=/dev/sda", "This will destroy your disk!"),
        (":(){ :|:& };:", "Fork bomb that will crash your system!"),
        ("> /dev/sda", "This will overwrite your disk!"),
        ("mkfs.ext4 /dev/sda", "This will format your disk!"),
    ];

    for (pattern, msg) in critical_patterns {
        if cmd_lower.contains(pattern) {
            findings.push(SecurityCheck {
                level: SecurityLevel::Critical,
                message: msg.to_string(),
                suggestion: Some("DO NOT EXECUTE THIS COMMAND".to_string()),
            });
        }
    }

    // Dangerous patterns
    let dangerous_patterns = [
        ("rm -rf", "Recursive deletion can delete important files"),
        ("chmod -R 777 /", "Making all files world-writable is dangerous"),
        ("chown -R", "Changing ownership of system files can break things"),
        ("sudo", "Running with elevated privileges is risky"),
    ];

    for (pattern, msg) in dangerous_patterns {
        if cmd_lower.contains(pattern) {
            findings.push(SecurityCheck {
                level: SecurityLevel::Dangerous,
                message: msg.to_string(),
                suggestion: Some("Review carefully before executing".to_string()),
            });
        }
    }

    // Warning patterns
    let warning_patterns = [
        ("| bash", "Piping to bash is dangerous"),
        ("| sh", "Piping to shell is dangerous"),
        ("curl", "Downloading content with curl may be unsafe"),
        ("> ~", "Overwriting files in home directory"),
    ];

    for (pattern, msg) in warning_patterns {
        if cmd_lower.contains(pattern) {
            findings.push(SecurityCheck {
                level: SecurityLevel::Warning,
                message: msg.to_string(),
                suggestion: Some("Verify the source before executing".to_string()),
            });
        }
    }

    findings
}

/// Check file write operations for suspicious paths
pub fn check_write_path(path: &str) -> Vec<SecurityCheck> {
    let mut findings = Vec::new();
    let path_lower = path.to_lowercase();

    // Critical paths
    let critical_paths = [
        "/", "/bin", "/sbin", "/usr/bin", "/usr/sbin",
        "/etc", "/lib", "/lib64", "/usr/lib", "/usr/lib64",
        "/boot", "/dev", "/sys", "/proc",
    ];

    for critical in critical_paths {
        if path_lower == critical || path_lower.starts_with(&format!("{}/", critical)) {
            findings.push(SecurityCheck {
                level: SecurityLevel::Critical,
                message: format!("Writing to system path: {}", critical),
                suggestion: Some("Avoid modifying system directories".to_string()),
            });
        }
    }

    // Home directory root
    if path_lower == "~" || path_lower == "$home" || path_lower == "/home" {
        findings.push(SecurityCheck {
            level: SecurityLevel::Dangerous,
            message: "Writing directly to home directory root".to_string(),
            suggestion: Some("Use a subdirectory instead".to_string()),
        });
    }

    findings
}

/// Check tool arguments for security issues
pub fn check_tool_call(tool_name: &str, args: &str) -> Vec<SecurityCheck> {
    let mut findings = Vec::new();

    match tool_name {
        "shell" | "execute" | "exec" => {
            findings.extend(check_shell_command(args));
        }
        "write_file" | "str_replace" => {
            // Try to extract path from args
            if let Some(path) = extract_path_from_args(args) {
                findings.extend(check_write_path(&path));
            }
        }
        _ => {}
    }

    findings
}

/// Extract path from JSON arguments
fn extract_path_from_args(args: &str) -> Option<String> {
    // Simple JSON path extraction
    if let Some(idx) = args.find("\"path\"") {
        let after = &args[idx..];
        if let Some(colon) = after.find(':') {
            let value_start = &after[colon + 1..];
            // Find quoted string
            if let Some(quote) = value_start.find('"') {
                let after_quote = &value_start[quote + 1..];
                if let Some(end_quote) = after_quote.find('"') {
                    return Some(after_quote[..end_quote].to_string());
                }
            }
        }
    }
    None
}

/// Format security findings for display
pub fn format_findings(findings: &[SecurityCheck]) -> String {
    if findings.is_empty() {
        return String::new();
    }

    let mut lines = vec!["⚠️  Security Check Results:".to_string()];
    
    for finding in findings {
        let emoji = match finding.level {
            SecurityLevel::Info => "ℹ️",
            SecurityLevel::Warning => "⚠️",
            SecurityLevel::Dangerous => "🚨",
            SecurityLevel::Critical => "💥",
        };
        
        lines.push(format!(
            "  {} [{}] {}",
            emoji,
            finding.level.as_str(),
            finding.message
        ));
        
        if let Some(suggestion) = &finding.suggestion {
            lines.push(format!("     💡 {}", suggestion));
        }
    }
    
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_critical_patterns() {
        let checks = check_shell_command("rm -rf /");
        assert!(checks.iter().any(|c| c.level == SecurityLevel::Critical));
    }

    #[test]
    fn test_dangerous_patterns() {
        let checks = check_shell_command("rm -rf /tmp/test");
        assert!(checks.iter().any(|c| c.level == SecurityLevel::Dangerous));
    }

    #[test]
    fn test_warning_patterns() {
        let checks = check_shell_command("curl https://example.com | bash");
        assert!(checks.iter().any(|c| c.level == SecurityLevel::Warning), 
                "Expected warning for curl | bash pattern");
    }

    #[test]
    fn test_safe_command() {
        let checks = check_shell_command("ls -la");
        assert!(checks.is_empty());
    }

    #[test]
    fn test_system_path_check() {
        let checks = check_write_path("/etc/passwd");
        assert!(checks.iter().any(|c| c.level == SecurityLevel::Critical));
    }

    #[test]
    fn test_tool_call_check() {
        let checks = check_tool_call("shell", "rm -rf /");
        assert!(!checks.is_empty());
    }
}
