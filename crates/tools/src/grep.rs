//! Grep tool for content search

use super::{Tool, ToolContext, ToolResult};
use regex::Regex;
use serde_json::json;
use serde_json::Value;


/// Grep tool for searching file contents
pub struct GrepTool;

impl Tool for GrepTool {
    fn name(&self) -> &str {
        "grep"
    }

    fn description(&self) -> &str {
        "Search for patterns in files using regex. \
         Returns matching lines with file names and line numbers."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "pattern": {
                    "type": "string",
                    "description": "Regex pattern to search for"
                },
                "path": {
                    "type": "string",
                    "description": "File or directory to search in (defaults to current directory)"
                },
                "file_pattern": {
                    "type": "string",
                    "description": "Optional glob pattern to filter files (e.g., '*.rs')"
                }
            },
            "required": ["pattern"]
        })
    }

    fn execute<'a>(
        &'a self,
        args: Value,
        ctx: &'a ToolContext,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ToolResult> + Send + 'a>> {
        Box::pin(async move {
            let pattern_str = args["pattern"].as_str().unwrap_or("");
            if pattern_str.is_empty() {
                return ToolResult::error("Pattern is required");
            }

            let path = args["path"].as_str().unwrap_or(".");
            let file_pattern = args["file_pattern"].as_str();

            // Compile regex
            let regex = match Regex::new(pattern_str) {
                Ok(re) => re,
                Err(e) => return ToolResult::error(format!("Invalid regex: {}", e)),
            };

            let search_path = ctx.working_dir.join(path);
            let mut matches = Vec::new();

            // Determine if path is file or directory
            let metadata = match tokio::fs::metadata(&search_path).await {
                Ok(m) => m,
                Err(e) => return ToolResult::error(format!("Cannot access path: {}", e)),
            };

            if metadata.is_file() {
                // Search single file
                if let Err(e) = search_file(&search_path, &regex, &mut matches).await {
                    return ToolResult::error(format!("Error searching file: {}", e));
                }
            } else {
                // Search directory
                let walker = walkdir::WalkDir::new(&search_path)
                    .max_depth(10)
                    .follow_links(false);

                for entry in walker {
                    let entry = match entry {
                        Ok(e) => e,
                        Err(_) => continue,
                    };

                    if !entry.file_type().is_file() {
                        continue;
                    }

                    let path = entry.path();

                    // Apply file pattern filter
                    if let Some(pattern) = file_pattern {
                        let file_name = path.file_name()
                            .and_then(|n| n.to_str())
                            .unwrap_or("");
                        if !glob::Pattern::new(pattern).map(|p| p.matches(file_name)).unwrap_or(false) {
                            continue;
                        }
                    }

                    // Skip binary files (simple heuristic)
                    let ext = path.extension()
                        .and_then(|e| e.to_str())
                        .unwrap_or("");
                    if is_binary_extension(ext) {
                        continue;
                    }

                    if let Err(_) = search_file(path, &regex, &mut matches).await {
                        continue; // Skip files we can't read
                    }
                }
            }

            if matches.is_empty() {
                ToolResult::success("No matches found".to_string())
            } else {
                // Limit output size
                let output = if matches.len() > 100 {
                    let mut output = matches[..100].join("\n");
                    output.push_str(&format!("\n... and {} more matches", matches.len() - 100));
                    output
                } else {
                    matches.join("\n")
                };
                ToolResult::success(output)
            }
        })
    }
}

/// Search a single file for pattern matches
async fn search_file(path: &std::path::Path, regex: &Regex, matches: &mut Vec<String>) -> Result<(), std::io::Error> {
    let content = tokio::fs::read_to_string(path).await?;
    let file_name = path.to_string_lossy();

    for (line_num, line) in content.lines().enumerate() {
        if regex.is_match(line) {
            // Truncate very long lines
            let display_line = if line.len() > 200 {
                format!("{}...", &line[..200])
            } else {
                line.to_string()
            };
            
            matches.push(format!("{}:{}: {}", file_name, line_num + 1, display_line));
        }
    }

    Ok(())
}

/// Check if file extension indicates binary file
fn is_binary_extension(ext: &str) -> bool {
    let binary_exts = [
        "exe", "dll", "so", "dylib", "bin",
        "jpg", "jpeg", "png", "gif", "bmp", "webp", "ico",
        "mp3", "mp4", "avi", "mov", "webm",
        "zip", "tar", "gz", "bz2", "7z", "rar",
        "pdf", "doc", "docx", "xls", "xlsx",
        "o", "obj", "class", "pyc",
    ];
    binary_exts.contains(&ext.to_lowercase().as_str())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_binary_extension() {
        assert!(is_binary_extension("exe"));
        assert!(is_binary_extension("png"));
        assert!(is_binary_extension("zip"));
        assert!(!is_binary_extension("rs"));
        assert!(!is_binary_extension("txt"));
    }

    #[tokio::test]
    async fn test_grep_tool_params() {
        let tool = GrepTool;
        let ctx = ToolContext::default();
        
        // Test missing pattern
        let result = tool.execute(json!({}), &ctx).await;
        assert!(matches!(result, ToolResult::Error(_)));

        // Test empty pattern
        let result = tool.execute(json!({"pattern": ""}), &ctx).await;
        assert!(matches!(result, ToolResult::Error(_)));
    }
}
