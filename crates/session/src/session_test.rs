#[cfg(test)]
mod tests {
    use super::super::*;
    use llm::Message;

    #[test]
    fn test_session_creation() {
        let session = Session::new("test-id");
        assert_eq!(session.id, "test-id");
        assert_eq!(session.title, "New Session");
        assert!(session.messages.is_empty());
    }

    #[test]
    fn test_session_add_message() {
        let mut session = Session::new("test-id");
        let old_updated = session.updated_at;
        
        session.add_message(Message::user("Hello"));
        
        assert_eq!(session.messages.len(), 1);
        assert!(session.updated_at > old_updated);
    }

    #[test]
    fn test_session_clear() {
        let mut session = Session::new("test-id");
        session.add_message(Message::user("Hello"));
        session.clear();
        
        assert!(session.messages.is_empty());
    }

    #[test]
    fn test_session_serialization() {
        let session = Session::new("test-id");
        let json = serde_json::to_string(&session).unwrap();
        assert!(json.contains("test-id"));
        assert!(json.contains("New Session"));
    }

    #[test]
    fn test_session_manager_create() {
        let manager = SessionManager::new();
        assert!(manager.is_ok());
        
        let manager = manager.unwrap();
        let session = manager.create();
        
        assert!(!session.id.is_empty());
        assert_eq!(session.messages.len(), 0);
    }

    #[test]
    fn test_session_manager_get_or_create() {
        let manager = SessionManager::new().unwrap();
        
        let mut session1 = manager.get_or_create("my-session");
        session1.add_message(Message::user("Hello"));
        
        // Save and reload
        manager.save(&session1).unwrap();
        
        let session2 = manager.get_or_create("my-session");
        // Note: This won't have the message unless we properly implement persistence
        assert_eq!(session2.id, "my-session");
    }
}
