//! Integration Tests
//!
//! Tests that verify module/service/component collaboration.
//! Involves databases, caches, models, internal APIs.

// Session integration tests
mod session {
    use session::{SessionStore, SessionSearch};

    #[tokio::test]
    async fn test_session_lifecycle() {
        let temp_dir = std::env::temp_dir().join(format!("test_session_{}", std::process::id()));
        std::fs::create_dir_all(&temp_dir).unwrap();
        
        let mut store = SessionStore::with_path(&temp_dir).await.unwrap();
        let test_session = store.create(Some("Test Session".to_string())).await.unwrap();
        let id = test_session.id.clone();
        
        assert!(store.get(&id).is_some());
        store.add_message(&id, session::SessionMessage::user("Hello")).await.unwrap();
        
        let updated = store.get(&id).unwrap();
        assert_eq!(updated.message_count(), 1);
        
        store.save(&id).await.unwrap();
        
        let mut store2 = SessionStore::with_path(&temp_dir).await.unwrap();
        assert!(store2.get(&id).is_some());
        
        let results = store2.search(SessionSearch::new().query("Test"));
        assert_eq!(results.len(), 1);
        
        store2.delete(&id).await.unwrap();
        assert!(store2.get(&id).is_none());
        
        let _ = std::fs::remove_dir_all(&temp_dir);
    }
}

// Provider integration tests
mod provider {
    #[tokio::test]
    async fn test_provider_registry() {
        use llm::{ProviderRegistry, ProviderConfig, ProviderType};
        
        let registry = ProviderRegistry::new();
        let available = registry.list_available().await;
        assert!(available.is_empty());
        
        let config = ProviderConfig::new(ProviderType::Ollama);
        let _result = registry.create_and_register(config).await;
        let _ = registry.list_all().await;
    }
}

// File operations integration tests
mod files {
    #[test]
    fn test_file_roundtrip() {
        let temp_dir = std::env::temp_dir().join("chat_test");
        let test_file = temp_dir.join("test.txt");
        
        let _ = std::fs::remove_dir_all(&temp_dir);
        std::fs::create_dir_all(&temp_dir).unwrap();
        
        let content = "Hello, World!\nThis is a test file.";
        std::fs::write(&test_file, content).unwrap();
        
        let read = std::fs::read_to_string(&test_file).unwrap();
        assert_eq!(content, read);
        
        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_directory_listing() {
        let temp_dir = std::env::temp_dir().join("chat_test_dir");
        let _ = std::fs::remove_dir_all(&temp_dir);
        std::fs::create_dir_all(&temp_dir).unwrap();
        
        std::fs::write(temp_dir.join("file1.txt"), "1").unwrap();
        std::fs::write(temp_dir.join("file2.txt"), "2").unwrap();
        std::fs::create_dir(temp_dir.join("subdir")).unwrap();
        
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

    #[test]
    fn test_grep_with_files() {
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
        
        let read_content = std::fs::read_to_string(temp_dir.join("main.rs")).unwrap();
        let results: Vec<_> = read_content.lines()
            .enumerate()
            .filter(|(_, line)| line.contains("fn "))
            .map(|(i, line)| (i + 1, line.trim()))
            .collect();
        
        assert_eq!(results.len(), 2);
        assert!(results.iter().any(|(_, line)| line.contains("main")));
        assert!(results.iter().any(|(_, line)| line.contains("helper")));
        
        let _ = std::fs::remove_dir_all(&temp_dir);
    }
}
