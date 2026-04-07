//! End-to-End Tests
//!
//! Tests that simulate real user complete workflows.
//! Full system black-box testing, focus on usability.

use std::process::Command;

fn bin_path() -> std::path::PathBuf {
    std::env::current_exe()
        .unwrap()
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("d")
}

#[test]
fn test_cli_help() {
    let output = Command::new(bin_path())
        .arg("--help")
        .output()
        .expect("Failed to execute command");
    
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("AI Daemon") || stdout.contains("Usage:"));
}

#[test]
fn test_cli_version() {
    let output = Command::new(bin_path())
        .arg("--version")
        .output()
        .expect("Failed to execute command");
    
    assert!(output.status.success());
}
