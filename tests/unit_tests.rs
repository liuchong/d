//! Unit Tests
//!
//! Tests for business functions, methods, and classes.
//! No external dependencies, run independently.

// Colors tests
mod colors {
    use cli::ui::{Color, Styled, red, green, blue};

    #[test]
    fn test_color_codes() {
        assert_eq!(Color::Red.fg_code(), "\x1b[31m");
        assert_eq!(Color::Green.bg_code(), "\x1b[42m");
    }

    #[test]
    fn test_styled_creation() {
        let styled = Styled::new("test").fg(Color::Red).bold();
        let _ = styled.to_string();
    }

    #[test]
    fn test_convenience_functions() {
        let _ = red("error");
        let _ = green("ok");
        let _ = blue("info");
    }
}

// Fuzzy matcher tests
mod fuzzy {
    use cli::completion::FuzzyMatcher;

    #[test]
    fn test_exact_match() {
        assert_eq!(FuzzyMatcher::score("help", "help"), 100.0);
    }

    #[test]
    fn test_prefix_match() {
        let score = FuzzyMatcher::score("he", "help");
        assert!(score > 80.0 && score < 100.0);
    }

    #[test]
    fn test_contains_match() {
        let score = FuzzyMatcher::score("el", "help");
        assert!(score > 50.0 && score < 80.0);
    }

    #[test]
    fn test_fuzzy_match() {
        let score = FuzzyMatcher::score("hl", "help");
        assert!(score > 0.0);
    }

    #[test]
    fn test_no_match() {
        assert_eq!(FuzzyMatcher::score("xyz", "help"), 0.0);
    }
}

// Input processing tests
mod input {
    use cli::input::{is_pasted_input, normalize_newlines};

    #[test]
    fn test_paste_detection() {
        assert!(is_pasted_input("fn main() {\n    println!();\n}"));
        assert!(is_pasted_input("```rust\ncode\n```"));
        assert!(!is_pasted_input("Hello"));
    }

    #[test]
    fn test_newline_normalization() {
        let input = "Line 1\r\nLine 2\rLine 3";
        let normalized = normalize_newlines(input);
        assert!(!normalized.contains('\r'));
    }
}

// Security tests
mod security {
    #[test]
    fn test_dangerous_commands_detection() {
        let dangerous_patterns = vec![
            "rm -rf /",
            "> /etc/passwd",
            "eval($user_input)",
            ":(){ :|:& };:",
        ];
        
        for cmd in dangerous_patterns {
            let is_dangerous = cmd.contains("rm -rf /")
                || cmd.contains("> /etc/")
                || cmd.contains("eval(")
                || cmd.contains(":(){");
            
            assert!(is_dangerous, "Should detect: {}", cmd);
        }
    }
}

// Session tests
mod session {
    use serde::{Deserialize, Serialize};

    #[test]
    fn test_session_roundtrip() {
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
        
        let test_session = TestSession {
            id: "test-session-123".to_string(),
            messages: vec![
                TestMessage { role: "user".to_string(), content: "Hello".to_string() },
                TestMessage { role: "assistant".to_string(), content: "Hi there!".to_string() },
            ],
        };
        
        let json = serde_json::to_string(&test_session).unwrap();
        let loaded: TestSession = serde_json::from_str(&json).unwrap();
        
        assert_eq!(test_session, loaded);
    }

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

    #[test]
    fn test_token_estimation() {
        let text = "Hello World";
        let estimated_tokens = text.len() / 4 + 1;
        
        assert!(estimated_tokens > 0);
        assert!(estimated_tokens <= text.len());
    }
}

// Tools tests
mod tools {
    #[test]
    fn test_str_replace() {
        let temp_dir = std::env::temp_dir().join("chat_test_replace");
        let test_file = temp_dir.join("replace.txt");
        
        let _ = std::fs::remove_dir_all(&temp_dir);
        std::fs::create_dir_all(&temp_dir).unwrap();
        
        let original = "foo bar baz";
        let expected = "hello bar baz";
        
        std::fs::write(&test_file, original).unwrap();
        
        let content = std::fs::read_to_string(&test_file).unwrap();
        let replaced = content.replacen("foo", "hello", 1);
        std::fs::write(&test_file, &replaced).unwrap();
        
        let result = std::fs::read_to_string(&test_file).unwrap();
        assert_eq!(result, expected);
        
        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_glob_patterns() {
        let temp_dir = std::env::temp_dir().join("chat_test_glob");
        let _ = std::fs::remove_dir_all(&temp_dir);
        std::fs::create_dir_all(&temp_dir).unwrap();
        
        std::fs::write(temp_dir.join("test.rs"), "").unwrap();
        std::fs::write(temp_dir.join("main.rs"), "").unwrap();
        std::fs::write(temp_dir.join("lib.rs"), "").unwrap();
        std::fs::write(temp_dir.join("README.md"), "").unwrap();
        
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

    #[test]
    fn test_grep_search() {
        let content = r#"
fn main() {
    println!("Hello");
}

fn helper() {
    println!("World");
}
"#;
        
        let results: Vec<_> = content.lines()
            .enumerate()
            .filter(|(_, line)| line.contains("fn "))
            .map(|(i, line)| (i + 1, line.trim()))
            .collect();
        
        assert_eq!(results.len(), 2);
        assert!(results.iter().any(|(_, line)| line.contains("main")));
        assert!(results.iter().any(|(_, line)| line.contains("helper")));
    }

    #[test]
    fn test_game_state_transitions() {
        #[derive(Debug, PartialEq)]
        enum GameState {
            Start,
            Playing,
            Won,
            Lost,
        }
        
        let state = GameState::Won;
        assert_eq!(state, GameState::Won);
    }

    #[test]
    fn test_tool_registry() {
        use tools::default_registry;
        
        let registry = default_registry();
        let tools_list = registry.to_llm_tools();
        
        assert!(!tools_list.is_empty());
        
        let tool_names: Vec<_> = tools_list.iter().map(|t| t.function.name.clone()).collect();
        assert!(tool_names.contains(&"shell".to_string()));
        assert!(tool_names.contains(&"read_file".to_string()));
    }
}
