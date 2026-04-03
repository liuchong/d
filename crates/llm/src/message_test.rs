#[cfg(test)]
mod tests {
    use super::super::*;

    #[test]
    fn test_message_system() {
        let msg = Message::system("System prompt");
        assert!(matches!(msg.role, MessageRole::System));
        assert_eq!(msg.content, "System prompt");
        assert!(msg.tool_calls.is_none());
    }

    #[test]
    fn test_message_user() {
        let msg = Message::user("Hello");
        assert!(matches!(msg.role, MessageRole::User));
        assert_eq!(msg.content, "Hello");
    }

    #[test]
    fn test_message_assistant() {
        let msg = Message::assistant("Hi there");
        assert!(matches!(msg.role, MessageRole::Assistant));
        assert_eq!(msg.content, "Hi there");
    }

    #[test]
    fn test_message_with_tool_calls() {
        let msg = Message::assistant("Let me help")
            .with_tool_calls(vec![serde_json::json!({"id": "1"})]);
        assert!(msg.tool_calls.is_some());
        assert_eq!(msg.tool_calls.unwrap().len(), 1);
    }

    #[test]
    fn test_message_serialization() {
        let msg = Message::user("Test");
        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains("user"));
        assert!(json.contains("Test"));
    }
}
