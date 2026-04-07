//! Integration tests for end-to-end workflows

use std::process::Command;

/// Test binary path
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

#[tokio::test]
async fn test_session_lifecycle() {
    use session::{SessionStore, SessionSearch};
    
    // Create temporary directory for test
    let temp_dir = std::env::temp_dir().join(format!("test_session_{}", std::process::id()));
    std::fs::create_dir_all(&temp_dir).unwrap();
    
    // Create store
    let mut store = SessionStore::with_path(&temp_dir).await.unwrap();
    
    // Create session
    let session = store.create(Some("Test Session".to_string())).await.unwrap();
    let id = session.id.clone();
    
    // Verify session exists
    assert!(store.get(&id).is_some());
    
    // Add message
    store.add_message(&id, session::SessionMessage::user("Hello")).await.unwrap();
    
    // Verify message added
    let updated = store.get(&id).unwrap();
    assert_eq!(updated.message_count(), 1);
    
    // Save to disk
    store.save(&id).await.unwrap();
    
    // Create new store instance and load
    let mut store2 = SessionStore::with_path(&temp_dir).await.unwrap();
    assert!(store2.get(&id).is_some());
    
    // Search
    let results = store2.search(SessionSearch::new().query("Test"));
    assert_eq!(results.len(), 1);
    
    // Delete
    store2.delete(&id).await.unwrap();
    assert!(store2.get(&id).is_none());
    
    // Cleanup
    let _ = std::fs::remove_dir_all(&temp_dir);
}

#[tokio::test]
async fn test_provider_registry() {
    use llm::{ProviderRegistry, ProviderConfig, ProviderType};
    
    let registry = ProviderRegistry::new();
    
    // Initially empty
    let available = registry.list_available().await;
    assert!(available.is_empty());
    
    // Create a mock provider config (without actual API key)
    let config = ProviderConfig::new(ProviderType::Ollama);
    
    // Register should work even without API key for Ollama
    let _result = registry.create_and_register(config).await;
    // This might fail if Ollama is not running, that's ok for this test
    
    // Just verify registry operations don't panic
    let _ = registry.list_all().await;
}

#[test]
fn test_tool_registry() {
    use tools::default_registry;
    
    let registry = default_registry();
    let tools = registry.to_llm_tools();
    
    // Verify tools are registered
    assert!(!tools.is_empty());
    
    // Check specific tools exist
    let tool_names: Vec<_> = tools.iter().map(|t| t.function.name.clone()).collect();
    assert!(tool_names.contains(&"shell".to_string()));
    assert!(tool_names.contains(&"read_file".to_string()));
}

#[test]
fn test_fuzzy_matcher() {
    use cli::completion::FuzzyMatcher;
    
    // Test exact match
    assert_eq!(FuzzyMatcher::score("help", "help"), 100.0);
    
    // Test prefix match
    let score = FuzzyMatcher::score("he", "help");
    assert!(score > 80.0 && score < 100.0);
    
    // Test contains match
    let score = FuzzyMatcher::score("el", "help");
    assert!(score > 50.0 && score < 80.0);
    
    // Test fuzzy match
    let score = FuzzyMatcher::score("hl", "help");
    assert!(score > 0.0);
    
    // Test no match
    assert_eq!(FuzzyMatcher::score("xyz", "help"), 0.0);
}

#[test]
fn test_input_processing() {
    use cli::input::{is_pasted_input, normalize_newlines};
    
    // Test paste detection
    assert!(is_pasted_input("fn main() {\n    println!();\n}"));
    assert!(is_pasted_input("```rust\ncode\n```"));
    assert!(!is_pasted_input("Hello"));
    
    // Test newline normalization
    let input = "Line 1\r\nLine 2\rLine 3";
    let normalized = normalize_newlines(input);
    assert!(!normalized.contains('\r'));
}

#[test]
fn test_environment_detection() {
    use kernel::environment::EnvironmentInfo;
    
    let info = EnvironmentInfo::detect();
    
    // Should detect OS
    assert!(!info.os.to_string().is_empty());
    
    // Should detect architecture
    assert!(!info.arch.is_empty());
    
    // CI detection should work
    let _ = info.is_ci();
}

#[test]
fn test_colors() {
    use cli::ui::{Color, Styled, red, green, blue};
    
    // Test color codes
    assert_eq!(Color::Red.fg_code(), "\x1b[31m");
    assert_eq!(Color::Green.bg_code(), "\x1b[42m");
    
    // Test styled creation
    let styled = Styled::new("test").fg(Color::Red).bold();
    let _ = styled.to_string();
    
    // Test convenience functions
    let _ = red("error");
    let _ = green("ok");
    let _ = blue("info");
}
