//! File system tools

use super::{Tool, ToolContext, ToolResult};

use serde_json::json;
use serde_json::Value;
use std::path::PathBuf;

/// Read file tool
pub struct ReadFileTool;

impl Tool for ReadFileTool {
    fn name(&self) -> &str {
        "read_file"
    }

    fn description(&self) -> &str {
        "Read the contents of a file. Returns the file content as a string. \
         Use this to view source code, configuration files, or any text file. \
         Maximum file size is 1MB."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Absolute or relative path to the file"
                }
            },
            "required": ["path"]
        })
    }

    fn execute<'a>(
        &'a self,
        args: Value,
        ctx: &'a ToolContext,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ToolResult> + Send + 'a>> {
        Box::pin(async move {
            let path = args["path"].as_str().unwrap_or("");
            if path.is_empty() {
                return ToolResult::error("Path is required");
            }

            let full_path = resolve_path(&ctx.working_dir, path);
            
            // Check file size first (1MB limit)
            match tokio::fs::metadata(&full_path).await {
                Ok(metadata) => {
                    if metadata.len() > 1024 * 1024 {
                        return ToolResult::error("File too large (max 1MB)");
                    }
                    if !metadata.is_file() {
                        return ToolResult::error("Path is not a file");
                    }
                }
                Err(e) => return ToolResult::error(format!("Cannot access file: {}", e)),
            }

            match tokio::fs::read_to_string(&full_path).await {
                Ok(content) => ToolResult::success(content),
                Err(e) => ToolResult::error(format!("Failed to read file: {}", e)),
            }
        })
    }
}

/// Write file tool
pub struct WriteFileTool;

impl Tool for WriteFileTool {
    fn name(&self) -> &str {
        "write_file"
    }

    fn description(&self) -> &str {
        "Write content to a file. Creates the file if it doesn't exist, \
         overwrites if it does. Use with caution."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Path to the file"
                },
                "content": {
                    "type": "string",
                    "description": "Content to write"
                }
            },
            "required": ["path", "content"]
        })
    }

    fn execute<'a>(
        &'a self,
        args: Value,
        ctx: &'a ToolContext,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ToolResult> + Send + 'a>> {
        Box::pin(async move {
            if !ctx.allow_dangerous {
                return ToolResult::error("Write operations require approval");
            }

            let path = args["path"].as_str().unwrap_or("");
            let content = args["content"].as_str().unwrap_or("");

            if path.is_empty() {
                return ToolResult::error("Path is required");
            }

            let full_path = resolve_path(&ctx.working_dir, path);
            
            // Ensure parent directory exists
            if let Some(parent) = full_path.parent() {
                if let Err(e) = tokio::fs::create_dir_all(parent).await {
                    return ToolResult::error(format!("Failed to create directory: {}", e));
                }
            }

            match tokio::fs::write(&full_path, content).await {
                Ok(_) => ToolResult::success(format!("File written: {}", full_path.display())),
                Err(e) => ToolResult::error(format!("Failed to write file: {}", e)),
            }
        })
    }
}

/// List directory tool
pub struct ListDirectoryTool;

impl Tool for ListDirectoryTool {
    fn name(&self) -> &str {
        "list_directory"
    }

    fn description(&self) -> &str {
        "List files and directories in the given path. \
         Returns a formatted list with file types and sizes."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Directory path (defaults to current directory)"
                }
            },
            "required": []
        })
    }

    fn execute<'a>(
        &'a self,
        args: Value,
        ctx: &'a ToolContext,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ToolResult> + Send + 'a>> {
        Box::pin(async move {
            let path = args["path"].as_str().unwrap_or(".");
            let full_path = resolve_path(&ctx.working_dir, path);

            let mut entries = match tokio::fs::read_dir(&full_path).await {
                Ok(entries) => entries,
                Err(e) => return ToolResult::error(format!("Cannot read directory: {}", e)),
            };

            let mut output = String::new();
            output.push_str(&format!("Directory: {}\n\n", full_path.display()));

            while let Ok(Some(entry)) = entries.next_entry().await {
                let name = entry.file_name().to_string_lossy().to_string();
                let metadata = entry.metadata().await.ok();
                
                let prefix = if metadata.as_ref().map(|m| m.is_dir()).unwrap_or(false) {
                    "📁"
                } else {
                    "📄"
                };
                
                let size = metadata.as_ref()
                    .map(|m| format_size(m.len()))
                    .unwrap_or_else(|| "-".to_string());
                
                output.push_str(&format!("{} {:<40} {}\n", prefix, name, size));
            }

            ToolResult::success(output)
        })
    }
}

/// Glob tool for pattern matching
pub struct GlobTool;

impl Tool for GlobTool {
    fn name(&self) -> &str {
        "glob"
    }

    fn description(&self) -> &str {
        "Find files matching a glob pattern. \
         Supports wildcards like *.rs, src/**/*.toml, etc."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "pattern": {
                    "type": "string",
                    "description": "Glob pattern to match"
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
            let pattern = args["pattern"].as_str().unwrap_or("");
            if pattern.is_empty() {
                return ToolResult::error("Pattern is required");
            }

            let full_pattern = resolve_path(&ctx.working_dir, pattern);
            let pattern_str = full_pattern.to_string_lossy();

            match glob::glob(&pattern_str) {
                Ok(paths) => {
                    let matches: Vec<String> = paths
                        .filter_map(|p| p.ok())
                        .map(|p| p.to_string_lossy().to_string())
                        .collect();
                    
                    if matches.is_empty() {
                        ToolResult::success("No files matched the pattern".to_string())
                    } else {
                        ToolResult::success(matches.join("\n"))
                    }
                }
                Err(e) => ToolResult::error(format!("Invalid pattern: {}", e)),
            }
        })
    }
}

/// Resolve a path relative to working directory
fn resolve_path(working_dir: &std::path::Path, path: &str) -> PathBuf {
    let path = PathBuf::from(path);
    if path.is_absolute() {
        path
    } else {
        working_dir.join(path)
    }
}

/// Format file size for display
fn format_size(size: u64) -> String {
    const UNITS: &[&str] = &["B", "KB", "MB", "GB"];
    if size == 0 {
        return "0 B".to_string();
    }
    let exp = (size as f64).log(1024.0).min(UNITS.len() as f64 - 1.0) as usize;
    let value = size as f64 / 1024f64.powi(exp as i32);
    if exp == 0 {
        format!("{} {}", size, UNITS[0])
    } else {
        format!("{:.1} {}", value, UNITS[exp])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_resolve_path() {
        let cwd = std::path::Path::new("/home/user");
        
        // Relative path
        let resolved = resolve_path(cwd, "file.txt");
        assert_eq!(resolved, PathBuf::from("/home/user/file.txt"));
        
        // Absolute path
        let resolved = resolve_path(cwd, "/etc/config");
        assert_eq!(resolved, PathBuf::from("/etc/config"));
    }

    #[test]
    fn test_format_size() {
        assert_eq!(format_size(0), "0 B");
        assert_eq!(format_size(512), "512 B");
        assert_eq!(format_size(1024), "1.0 KB");
        assert_eq!(format_size(1024 * 1024), "1.0 MB");
    }

    #[tokio::test]
    async fn test_read_file_tool() {
        let tool = ReadFileTool;
        let ctx = ToolContext::default();
        
        // Test missing path
        let result = tool.execute(json!({}), &ctx).await;
        assert!(matches!(result, ToolResult::Error(_)));
    }

    #[tokio::test]
    async fn test_list_directory_tool() {
        let tool = ListDirectoryTool;
        let ctx = ToolContext::default();
        
        let result = tool.execute(json!({"path": "."}), &ctx).await;
        assert!(matches!(result, ToolResult::Success(_)));
    }
}
