#[cfg(test)]
mod tests {
    use super::super::*;

    #[test]
    fn test_tool_creation() {
        let tool = Tool::new("read_file", "Read a file");
        assert_eq!(tool.tool_type, "function");
        assert_eq!(tool.function.name, "read_file");
        assert_eq!(tool.function.description, "Read a file");
    }

    #[test]
    fn test_tool_with_parameters() {
        let params = serde_json::json!({
            "type": "object",
            "properties": {
                "path": {"type": "string"}
            }
        });
        let tool = Tool::new("read_file", "Read a file")
            .with_parameters(params.clone());
        assert_eq!(tool.function.parameters, params);
    }

    #[test]
    fn test_tool_result() {
        let result = ToolResult::new("call_1", "read_file", "File content");
        assert_eq!(result.tool_call_id, "call_1");
        assert_eq!(result.name, "read_file");
        assert_eq!(result.content, "File content");
        assert_eq!(result.role, "tool");
    }

    #[test]
    fn test_tool_call_serialization() {
        let call = ToolCall {
            id: "1".to_string(),
            call_type: "function".to_string(),
            function: FunctionCall {
                name: "test".to_string(),
                arguments: "{}".to_string(),
            },
        };
        let json = serde_json::to_string(&call).unwrap();
        assert!(json.contains("function"));
    }
}
