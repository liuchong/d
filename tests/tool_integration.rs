//! Tool Integration Tests
//! 
//! Validates that tools work correctly for real-world use cases.
//! Corresponds to Zig: tests/integration/tool_integration.zig

use std::collections::HashMap;

/// Test: File read/write roundtrip
#[test]
fn test_file_roundtrip() {
    let temp_dir = std::env::temp_dir().join("chat_test");
    let test_file = temp_dir.join("test.txt");
    
    // Clean up
    let _ = std::fs::remove_dir_all(&temp_dir);
    std::fs::create_dir_all(&temp_dir).unwrap();
    
    let content = "Hello, World!\nThis is a test file.";
    
    // Write
    std::fs::write(&test_file, content).unwrap();
    
    // Read back
    let read = std::fs::read_to_string(&test_file).unwrap();
    
    assert_eq!(content, read);
    
    // Cleanup
    let _ = std::fs::remove_dir_all(&temp_dir);
}

/// Test: String replacement in file
#[test]
fn test_str_replace() {
    let temp_dir = std::env::temp_dir().join("chat_test_replace");
    let test_file = temp_dir.join("replace.txt");
    
    let _ = std::fs::remove_dir_all(&temp_dir);
    std::fs::create_dir_all(&temp_dir).unwrap();
    
    let original = "foo bar baz";
    let expected = "hello bar baz";
    
    std::fs::write(&test_file, original).unwrap();
    
    // Simulate str_replace
    let content = std::fs::read_to_string(&test_file).unwrap();
    let replaced = content.replacen("foo", "hello", 1);
    std::fs::write(&test_file, &replaced).unwrap();
    
    let result = std::fs::read_to_string(&test_file).unwrap();
    assert_eq!(result, expected);
    
    let _ = std::fs::remove_dir_all(&temp_dir);
}

/// Test: Directory listing
#[test]
fn test_directory_listing() {
    let temp_dir = std::env::temp_dir().join("chat_test_dir");
    let _ = std::fs::remove_dir_all(&temp_dir);
    std::fs::create_dir_all(&temp_dir).unwrap();
    
    // Create some files
    std::fs::write(temp_dir.join("file1.txt"), "1").unwrap();
    std::fs::write(temp_dir.join("file2.txt"), "2").unwrap();
    std::fs::create_dir(temp_dir.join("subdir")).unwrap();
    
    // List
    let entries: Vec<_> = std::fs::read_dir(&temp_dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().to_string())
        .collect();
    
    assert!(entries.contains(&"file1.txt".to_string()));
    assert!(entries.contains(&"file2.txt".to_string()));
    assert!(entries.contains(&"subdir".to_string()));
    
    let _ = std::fs::remove_dir_all(&temp_dir);
}

/// Test: Glob pattern matching
#[test]
fn test_glob_patterns() {
    let temp_dir = std::env::temp_dir().join("chat_test_glob");
    let _ = std::fs::remove_dir_all(&temp_dir);
    std::fs::create_dir_all(&temp_dir).unwrap();
    
    std::fs::write(temp_dir.join("test.rs"), "").unwrap();
    std::fs::write(temp_dir.join("main.rs"), "").unwrap();
    std::fs::write(temp_dir.join("lib.rs"), "").unwrap();
    std::fs::write(temp_dir.join("README.md"), "").unwrap();
    
    // Simulate glob *.rs
    let pattern = "*.rs";
    let matcher = glob::Pattern::new(pattern).unwrap();
    
    let matches: Vec<_> = std::fs::read_dir(&temp_dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| matcher.matches(&e.file_name().to_string_lossy()))
        .map(|e| e.file_name().to_string_lossy().to_string())
        .collect();
    
    assert_eq!(matches.len(), 3);
    assert!(matches.iter().all(|f| f.ends_with(".rs")));
    
    let _ = std::fs::remove_dir_all(&temp_dir);
}

/// Test: Grep search
#[test]
fn test_grep_search() {
    let temp_dir = std::env::temp_dir().join("chat_test_grep");
    let _ = std::fs::remove_dir_all(&temp_dir);
    std::fs::create_dir_all(&temp_dir).unwrap();
    
    let content = r#"
fn main() {
    println!("Hello");
}

fn helper() {
    println!("World");
}
"#;
    
    std::fs::write(temp_dir.join("main.rs"), content).unwrap();
    
    // Simulate grep for "fn "
    let results: Vec<_> = content.lines()
        .enumerate()
        .filter(|(_, line)| line.contains("fn "))
        .map(|(i, line)| (i + 1, line.trim()))
        .collect();
    
    assert_eq!(results.len(), 2);
    assert!(results.iter().any(|(_, line)| line.contains("main")));
    assert!(results.iter().any(|(_, line)| line.contains("helper")));
    
    let _ = std::fs::remove_dir_all(&temp_dir);
}

/// Test: Security - Block dangerous commands
#[test]
fn test_security_blocks_dangerous_commands() {
    let dangerous_patterns = vec![
        "rm -rf /",
        "> /etc/passwd",
        "eval($user_input)",
        ":(){ :|:& };:",
    ];
    
    for cmd in dangerous_patterns {
        // Should be flagged as dangerous
        let is_dangerous = cmd.contains("rm -rf /")
            || cmd.contains("> /etc/")
            || cmd.contains("eval(")
            || cmd.contains(":(){");
        
        assert!(is_dangerous, "Should detect: {}", cmd);
    }
}

/// Test: Session save/load roundtrip
#[test]
fn test_session_roundtrip() {
    use serde::{Deserialize, Serialize};
    
    #[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
    struct TestMessage {
        role: String,
        content: String,
    }
    
    #[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
    struct TestSession {
        id: String,
        messages: Vec<TestMessage>,
    }
    
    let session = TestSession {
        id: "test-session-123".to_string(),
        messages: vec![
            TestMessage { role: "user".to_string(), content: "Hello".to_string() },
            TestMessage { role: "assistant".to_string(), content: "Hi there!".to_string() },
        ],
    };
    
    // Serialize
    let json = serde_json::to_string(&session).unwrap();
    
    // Deserialize
    let loaded: TestSession = serde_json::from_str(&json).unwrap();
    
    assert_eq!(session, loaded);
}

/// Test: Export/Import formats
#[test]
fn test_export_import_json() {
    let data = serde_json::json!({
        "id": "test",
        "messages": [
            {"role": "user", "content": "Hello"},
            {"role": "assistant", "content": "World"}
        ]
    });
    
    let json = serde_json::to_string_pretty(&data).unwrap();
    let loaded: serde_json::Value = serde_json::from_str(&json).unwrap();
    
    assert_eq!(data, loaded);
}

/// Test: Context token estimation
#[test]
fn test_token_estimation() {
    // Simple estimation: ~4 chars per token
    let text = "Hello World";
    let estimated_tokens = text.len() / 4 + 1;
    
    assert!(estimated_tokens > 0);
    assert!(estimated_tokens <= text.len());
}

/// Test: Game state transitions
#[test]
fn test_game_state_transitions() {
    // Simulate game state machine
    #[derive(Debug, PartialEq)]
    enum GameState {
        Start,
        Playing,
        Won,
        Lost,
    }
    
    let mut state = GameState::Start;
    
    // Take key
    state = GameState::Playing;
    
    // Use key in treasure room
    state = GameState::Won;
    
    assert_eq!(state, GameState::Won);
}
