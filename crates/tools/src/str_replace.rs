//! String replacement tool for file editing
//!
//! Provides precise text replacement in files with safety checks.

use crate::{Tool, ToolContext, ToolResult};
use serde_json::Value;

/// StrReplace tool for replacing text in files
pub struct StrReplaceTool;

impl Tool for StrReplaceTool {
    fn name(&self) -> &str {
        "str_replace"
    }

    fn description(&self) -> &str {
        "Replace text in a file. The old_string must match exactly. \
         Use this for precise text replacements. \
         If the old_string appears multiple times, this tool will fail - \
         you must provide a more specific old_string."
    }

    fn parameters(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Path to the file to modify"
                },
                "old_string": {
                    "type": "string",
                    "description": "Text to replace (must match exactly)"
                },
                "new_string": {
                    "type": "string",
                    "description": "New text to insert"
                }
            },
            "required": ["path", "old_string", "new_string"]
        })
    }

    fn execute<'a>(
        &'a self,
        args: Value,
        _ctx: &'a ToolContext,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ToolResult> + Send + 'a>> {
        Box::pin(async move {
            let path = args["path"].as_str().unwrap_or("");
            let old_string = args["old_string"].as_str().unwrap_or("");
            let new_string = args["new_string"].as_str().unwrap_or("");

            if path.is_empty() {
                return ToolResult::error("No path provided");
            }

            if old_string.is_empty() {
                return ToolResult::error("No old_string provided");
            }

            // Read file
            let content = match tokio::fs::read_to_string(path).await {
                Ok(c) => c,
                Err(e) => return ToolResult::error(format!("Failed to read file: {}", e)),
            };

            // Check if old_string exists
            if !content.contains(old_string) {
                return ToolResult::error(format!(
                    "Could not find the text to replace in {}. \
                     The text must match exactly.",
                    path
                ));
            }

            // Count occurrences
            let count = content.matches(old_string).count();
            if count > 1 {
                return ToolResult::error(format!(
                    "Found {} occurrences of the text in {}. \
                     This tool only supports unique replacements. \
                     Please use a more specific old_string.",
                    count, path
                ));
            }

            // Perform replacement
            let new_content = content.replacen(old_string, new_string, 1);

            // Write back
            if let Err(e) = tokio::fs::write(path, new_content).await {
                return ToolResult::error(format!("Failed to write file: {}", e));
            }

            ToolResult::success(format!(
                "Successfully replaced text in {} ({} -> {})",
                path,
                old_string.len(),
                new_string.len()
            ))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[tokio::test]
    async fn test_str_replace_success() {
        // Create temp file
        let temp_dir = tempfile::tempdir().unwrap();
        let file_path = temp_dir.path().join("test.txt");
        let mut file = std::fs::File::create(&file_path).unwrap();
        writeln!(file, "Hello World").unwrap();

        let tool = StrReplaceTool;
        let ctx = ToolContext::default();
        let args = serde_json::json!({
            "path": file_path.to_str().unwrap(),
            "old_string": "Hello",
            "new_string": "Hi"
        });

        let result = tool.execute(args, &ctx).await;
        assert!(matches!(result, ToolResult::Success(_)));

        // Verify content
        let content = std::fs::read_to_string(&file_path).unwrap();
        assert!(content.contains("Hi World"));
    }

    #[tokio::test]
    async fn test_str_replace_multiple_matches() {
        let temp_dir = tempfile::tempdir().unwrap();
        let file_path = temp_dir.path().join("test.txt");
        let mut file = std::fs::File::create(&file_path).unwrap();
        writeln!(file, "Hello Hello World").unwrap();

        let tool = StrReplaceTool;
        let ctx = ToolContext::default();
        let args = serde_json::json!({
            "path": file_path.to_str().unwrap(),
            "old_string": "Hello",
            "new_string": "Hi"
        });

        let result = tool.execute(args, &ctx).await;
        assert!(matches!(result, ToolResult::Error(_)));
        let ToolResult::Error(msg) = result else { unreachable!() };
        // Check for case-insensitive "multiple" or the count number
        assert!(msg.contains("2 occurrences") || msg.to_lowercase().contains("multiple"), 
                "Expected error about multiple occurrences, got: {}", msg);
    }

    #[tokio::test]
    async fn test_str_replace_not_found() {
        let temp_dir = tempfile::tempdir().unwrap();
        let file_path = temp_dir.path().join("test.txt");
        let mut file = std::fs::File::create(&file_path).unwrap();
        writeln!(file, "Hello World").unwrap();

        let tool = StrReplaceTool;
        let ctx = ToolContext::default();
        let args = serde_json::json!({
            "path": file_path.to_str().unwrap(),
            "old_string": "Foo",
            "new_string": "Bar"
        });

        let result = tool.execute(args, &ctx).await;
        assert!(matches!(result, ToolResult::Error(_)));
    }
}
