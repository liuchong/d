//! Self-correction system for AI responses
//!
//! Detects errors in tool execution and LLM responses,
//! generates corrections, and attempts automatic fixes.

use std::collections::VecDeque;

/// Error type that can be corrected
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CorrectableError {
    /// Tool execution failed
    ToolError { tool: String, error: String },
    /// LLM produced invalid output
    InvalidOutput { expected: String, got: String },
    /// Missing context or information
    MissingContext { detail: String },
    /// Syntax or parse error
    SyntaxError { location: String, message: String },
}

impl CorrectableError {
    /// Get error description
    pub fn description(&self) -> String {
        match self {
            CorrectableError::ToolError { tool, error } => {
                format!("Tool '{}' failed: {}", tool, error)
            }
            CorrectableError::InvalidOutput { expected, got } => {
                format!("Invalid output. Expected: {}, Got: {}", expected, got)
            }
            CorrectableError::MissingContext { detail } => {
                format!("Missing context: {}", detail)
            }
            CorrectableError::SyntaxError { location, message } => {
                format!("Syntax error at {}: {}", location, message)
            }
        }
    }

    /// Check if error is auto-correctable
    pub fn is_auto_correctable(&self) -> bool {
        match self {
            // Tool errors can often be retried with different args
            CorrectableError::ToolError { error, .. } => {
                !error.contains("Permission denied")
                    && !error.contains("not found")
                    && !error.contains("does not exist")
            }
            // Syntax errors might be correctable
            CorrectableError::SyntaxError { .. } => true,
            // Others usually need user input
            _ => false,
        }
    }
}

/// Correction suggestion
#[derive(Debug, Clone)]
pub struct Correction {
    pub error: CorrectableError,
    pub suggestion: String,
    pub confidence: f32,
    pub action: CorrectionAction,
}

/// Correction action
#[derive(Debug, Clone)]
pub enum CorrectionAction {
    /// Retry with modified parameters
    Retry { modified_params: serde_json::Value },
    /// Request more information
    RequestInfo { question: String },
    /// Use alternative approach
    Alternative { description: String },
    /// Manual fix required
    Manual,
}

/// Self-correction engine
pub struct SelfCorrection {
    /// Maximum correction attempts
    max_attempts: usize,
    /// Recent errors for pattern learning
    error_history: VecDeque<CorrectableError>,
    /// History size limit
    history_limit: usize,
}

impl Default for SelfCorrection {
    fn default() -> Self {
        Self::new()
    }
}

impl SelfCorrection {
    pub fn new() -> Self {
        Self {
            max_attempts: 3,
            error_history: VecDeque::new(),
            history_limit: 10,
        }
    }

    /// Analyze error and generate correction
    pub fn analyze_error(
        &mut self,
        error: &CorrectableError,
        context: &str,
    ) -> Option<Correction> {
        // Record error for pattern analysis
        self.record_error(error.clone());

        // Check if we've seen this error pattern before
        let similar_errors = self.find_similar_errors(error);

        let correction = match error {
            CorrectableError::ToolError { tool, error } => {
                self.analyze_tool_error(tool, error, context, &similar_errors)
            }
            CorrectableError::SyntaxError { location, message } => {
                self.analyze_syntax_error(location, message, context)
            }
            CorrectableError::MissingContext { detail } => {
                self.analyze_missing_context(detail, context)
            }
            CorrectableError::InvalidOutput { expected, got } => {
                self.analyze_invalid_output(expected, got, context)
            }
        };

        correction
    }

    /// Check if should attempt correction
    pub fn should_correct(&self, attempt: usize) -> bool {
        attempt < self.max_attempts
    }

    /// Get correction prompt for LLM
    pub fn get_correction_prompt(&self, error: &CorrectableError, previous_attempt: &str) -> String {
        format!(
            "The previous action failed with error: {}\n\n\
             Previous attempt: {}\n\n\
             Please analyze the error and provide a corrected approach. \
             Consider:\n\
             1. What caused the error?\n\
             2. How can we fix it?\n\
             3. What alternative approaches might work?",
            error.description(),
            previous_attempt
        )
    }

    /// Record error in history
    fn record_error(&mut self, error: CorrectableError) {
        self.error_history.push_back(error);
        if self.error_history.len() > self.history_limit {
            self.error_history.pop_front();
        }
    }

    /// Find similar past errors
    fn find_similar_errors(&self, error: &CorrectableError) -> Vec<&CorrectableError> {
        self.error_history
            .iter()
            .filter(|e| std::mem::discriminant(*e) == std::mem::discriminant(error))
            .collect()
    }

    /// Analyze tool error
    fn analyze_tool_error(
        &self,
        tool: &str,
        error: &str,
        _context: &str,
        _similar: &[&CorrectableError],
    ) -> Option<Correction> {
        // Common tool error patterns
        if error.contains("not found") || error.contains("does not exist") {
            return Some(Correction {
                error: CorrectableError::ToolError {
                    tool: tool.to_string(),
                    error: error.to_string(),
                },
                suggestion: "Check if the file/path exists and try again".to_string(),
                confidence: 0.8,
                action: CorrectionAction::RequestInfo {
                    question: "The specified file or path was not found. Please verify the correct path.".to_string(),
                },
            });
        }

        if error.contains("Permission denied") {
            return Some(Correction {
                error: CorrectableError::ToolError {
                    tool: tool.to_string(),
                    error: error.to_string(),
                },
                suggestion: "Permission denied. Try a different approach or request elevated privileges.".to_string(),
                confidence: 0.9,
                action: CorrectionAction::Manual,
            });
        }

        if error.contains("already exists") {
            return Some(Correction {
                error: CorrectableError::ToolError {
                    tool: tool.to_string(),
                    error: error.to_string(),
                },
                suggestion: "Target already exists. Consider using a different name or overwriting.".to_string(),
                confidence: 0.7,
                action: CorrectionAction::Alternative {
                    description: "Use a different file name or check if overwrite is intended".to_string(),
                },
            });
        }

        // Generic tool error
        Some(Correction {
            error: CorrectableError::ToolError {
                tool: tool.to_string(),
                error: error.to_string(),
            },
            suggestion: format!("Tool '{}' failed. Retrying with adjusted parameters may help.", tool),
            confidence: 0.5,
            action: CorrectionAction::Retry {
                modified_params: serde_json::json!({}),
            },
        })
    }

    /// Analyze syntax error
    fn analyze_syntax_error(
        &self,
        location: &str,
        message: &str,
        _context: &str,
    ) -> Option<Correction> {
        Some(Correction {
            error: CorrectableError::SyntaxError {
                location: location.to_string(),
                message: message.to_string(),
            },
            suggestion: format!("Syntax error at {}: {}. Review and fix the syntax.", location, message),
            confidence: 0.6,
            action: CorrectionAction::Manual,
        })
    }

    /// Analyze missing context
    fn analyze_missing_context(&self, detail: &str, _context: &str) -> Option<Correction> {
        Some(Correction {
            error: CorrectableError::MissingContext {
                detail: detail.to_string(),
            },
            suggestion: format!("More information needed: {}", detail),
            confidence: 0.9,
            action: CorrectionAction::RequestInfo {
                question: format!("Could you provide more details about: {}?", detail),
            },
        })
    }

    /// Analyze invalid output
    fn analyze_invalid_output(
        &self,
        expected: &str,
        got: &str,
        _context: &str,
    ) -> Option<Correction> {
        Some(Correction {
            error: CorrectableError::InvalidOutput {
                expected: expected.to_string(),
                got: got.to_string(),
            },
            suggestion: format!("Expected '{}' but got '{}'. Retrying with clearer instructions.", expected, got),
            confidence: 0.6,
            action: CorrectionAction::Retry {
                modified_params: serde_json::json!({}),
            },
        })
    }

    /// Get error statistics
    pub fn error_stats(&self) -> ErrorStats {
        let total = self.error_history.len();
        let tool_errors = self.error_history.iter().filter(|e| matches!(e, CorrectableError::ToolError { .. })).count();
        let syntax_errors = self.error_history.iter().filter(|e| matches!(e, CorrectableError::SyntaxError { .. })).count();

        ErrorStats {
            total,
            tool_errors,
            syntax_errors,
            context_errors: total - tool_errors - syntax_errors,
        }
    }
}

/// Error statistics
#[derive(Debug, Clone)]
pub struct ErrorStats {
    pub total: usize,
    pub tool_errors: usize,
    pub syntax_errors: usize,
    pub context_errors: usize,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_correctable_error_description() {
        let error = CorrectableError::ToolError {
            tool: "read_file".to_string(),
            error: "not found".to_string(),
        };
        assert!(error.description().contains("read_file"));
    }

    #[test]
    fn test_self_correction_analyze() {
        let mut correction = SelfCorrection::new();
        let error = CorrectableError::ToolError {
            tool: "test".to_string(),
            error: "Permission denied".to_string(),
        };

        let result = correction.analyze_error(&error, "test context");
        assert!(result.is_some());
        
        let correction = result.unwrap();
        assert!(matches!(correction.action, CorrectionAction::Manual));
    }

    #[test]
    fn test_should_correct() {
        let correction = SelfCorrection::new();
        assert!(correction.should_correct(0));
        assert!(correction.should_correct(2));
        assert!(!correction.should_correct(3));
    }

    #[test]
    fn test_error_stats() {
        let mut correction = SelfCorrection::new();
        correction.record_error(CorrectableError::ToolError {
            tool: "t1".to_string(),
            error: "e1".to_string(),
        });
        correction.record_error(CorrectableError::SyntaxError {
            location: "l1".to_string(),
            message: "m1".to_string(),
        });

        let stats = correction.error_stats();
        assert_eq!(stats.total, 2);
        assert_eq!(stats.tool_errors, 1);
        assert_eq!(stats.syntax_errors, 1);
    }
}
