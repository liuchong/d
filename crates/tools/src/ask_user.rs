//! Ask User tool - Interactive user prompts
//!
//! Allows the AI to ask clarifying questions to the user

use serde_json::{json, Value};

/// Ask User tool
pub struct AskUserTool;

impl super::Tool for AskUserTool {
    fn name(&self) -> &str {
        "ask_user"
    }

    fn description(&self) -> &str {
        "Ask the user a question and wait for their response. \
         Use this when you need clarification or additional information."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "required": ["question"],
            "properties": {
                "question": {
                    "type": "string",
                    "description": "The question to ask the user"
                },
                "options": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Optional list of choices for the user"
                },
                "sensitive": {
                    "type": "boolean",
                    "description": "Whether the input is sensitive (password, etc.)"
                }
            }
        })
    }

    fn execute<'a>(
        &'a self,
        args: Value,
        _ctx: &'a super::ToolContext,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = super::ToolResult> + Send + 'a>> {
        Box::pin(async move {
            let question = args
                .get("question")
                .and_then(|v| v.as_str())
                .unwrap_or("Please provide input:");
            
            let options = args.get("options").and_then(|v| v.as_array());
            let sensitive = args.get("sensitive").and_then(|v| v.as_bool()).unwrap_or(false);

            // In a real implementation, this would prompt the user
            // For now, we return a placeholder
            
            let response = if let Some(opts) = options {
                format!(
                    "Question: {}\nOptions: {}\n(Please respond with your choice)",
                    question,
                    opts.iter()
                        .filter_map(|v| v.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            } else if sensitive {
                format!(
                    "Question: {}\n(Sensitive input - will be hidden)",
                    question
                )
            } else {
                format!("Question: {}", question)
            };

            super::ToolResult::success(response)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Tool;

    #[test]
    fn test_ask_user_tool() {
        let tool = AskUserTool;
        assert_eq!(tool.name(), "ask_user");
    }
}
