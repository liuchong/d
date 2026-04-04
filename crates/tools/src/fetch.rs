//! URL fetching tool
//!
//! Fetches and extracts text content from URLs.

use crate::{Tool, ToolContext, ToolResult};
use serde_json::Value;

/// Fetch URL tool for retrieving web content
pub struct FetchUrlTool;

impl Tool for FetchUrlTool {
    fn name(&self) -> &str {
        "fetch_url"
    }

    fn description(&self) -> &str {
        "Fetch and extract text content from a URL. \
         Use this to read web pages, documentation, or any online content. \
         Returns plain text content stripped of HTML tags."
    }

    fn parameters(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "url": {
                    "type": "string",
                    "description": "URL to fetch content from"
                },
                "max_length": {
                    "type": "integer",
                    "description": "Maximum content length in characters (default: 10000, max: 100000)",
                    "minimum": 100,
                    "maximum": 100000
                }
            },
            "required": ["url"]
        })
    }

    fn execute<'a>(
        &'a self,
        args: Value,
        _ctx: &'a ToolContext,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ToolResult> + Send + 'a>> {
        Box::pin(async move {
            let url = args["url"].as_str().unwrap_or("");
            let max_length = args["max_length"].as_u64().unwrap_or(10000).clamp(100, 100000) as usize;

            if url.is_empty() {
                return ToolResult::error("No URL provided");
            }

            // Validate URL
            if !url.starts_with("http://") && !url.starts_with("https://") {
                return ToolResult::error(format!(
                    "Invalid URL '{}'. URL must start with http:// or https://",
                    url
                ));
            }

            let client = reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(30))
                .user_agent("Mozilla/5.0 (compatible; D-Chat/0.3)")
                .build()
                .map_err(|e| format!("Failed to create client: {}", e));

            let client = match client {
                Ok(c) => c,
                Err(e) => return ToolResult::error(e),
            };

            let response = client.get(url).send().await;
            let response = match response {
                Ok(r) => r,
                Err(e) => {
                    return ToolResult::error(format!(
                        "Failed to fetch URL '{}': {}",
                        url, e
                    ))
                }
            };

            // Check content length
            let content_length = response.content_length().unwrap_or(0);
            if content_length > 10 * 1024 * 1024 {
                return ToolResult::error(format!(
                    "Content too large ({} MB). Maximum allowed is 10 MB.",
                    content_length / (1024 * 1024)
                ));
            }

            let content = match response.text().await {
                Ok(c) => c,
                Err(e) => return ToolResult::error(format!("Failed to read response: {}", e)),
            };

            // Extract text from HTML
            let text = extract_text_from_html(&content);

            // Truncate if needed
            let result = if text.len() > max_length {
                format!("{}\n\n[Content truncated. Total length: {} characters]", 
                    &text[..max_length], 
                    text.len()
                )
            } else {
                text
            };

            ToolResult::success(result)
        })
    }
}

/// Extract readable text from HTML
fn extract_text_from_html(html: &str) -> String {
    let mut result = String::new();
    let mut in_tag = false;
    let mut in_script = false;
    let mut in_style = false;
    let mut last_was_space = true;

    let mut chars = html.chars().peekable();

    while let Some(c) = chars.next() {
        if c == '<' {
            // Check what kind of tag this is
            let mut tag_name = String::new();
            let mut peek_iter = chars.clone();
            
            // Skip whitespace
            while let Some(&p) = peek_iter.peek() {
                if p.is_whitespace() || p == '>' {
                    break;
                }
                tag_name.push(p);
                peek_iter.next();
            }
            
            let tag_lower = tag_name.to_lowercase();
            
            if tag_lower.starts_with("script") {
                in_script = true;
            } else if tag_lower.starts_with("style") {
                in_style = true;
            } else if tag_lower == "/script" {
                in_script = false;
            } else if tag_lower == "/style" {
                in_style = false;
            }
            
            in_tag = true;
            continue;
        }

        if c == '>' {
            in_tag = false;
            continue;
        }

        if in_tag || in_script || in_style {
            continue;
        }

        // Handle whitespace
        if c.is_whitespace() {
            if !last_was_space {
                result.push(' ');
                last_was_space = true;
            }
        } else {
            result.push(c);
            last_was_space = false;
        }
    }

    // Decode common HTML entities
    let text = result
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#x27;", "'")
        .replace("&#x2F;", "/")
        .replace("&nbsp;", " ");

    text.trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_text_from_html() {
        let html = r#"
            <html>
                <head><title>Test</title></head>
                <body>
                    <h1>Hello World</h1>
                    <p>This is a test.</p>
                    <script>alert('ignore');</script>
                </body>
            </html>
        "#;
        
        let text = extract_text_from_html(html);
        assert!(text.contains("Hello World"));
        assert!(text.contains("This is a test"));
        assert!(!text.contains("alert"));
        assert!(!text.contains("<script>"));
    }

    #[test]
    fn test_html_entities() {
        let html = "Test &amp; Example &lt;tag&gt;";
        let text = extract_text_from_html(html);
        assert_eq!(text, "Test & Example <tag>");
    }
}
