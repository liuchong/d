//! Web tools for searching and fetching content
//!
//! Provides web search via DuckDuckGo and URL content fetching.

use crate::{Tool, ToolContext, ToolResult};
use serde_json::Value;

/// Web search tool using DuckDuckGo
pub struct WebSearchTool;

impl Tool for WebSearchTool {
    fn name(&self) -> &str {
        "web_search"
    }

    fn description(&self) -> &str {
        "Search the web for information. \
         Use this when you need current information or facts not in your training data. \
         Returns search results with titles and snippets."
    }

    fn parameters(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "Search query"
                },
                "limit": {
                    "type": "integer",
                    "description": "Maximum number of results (default: 5, max: 10)",
                    "minimum": 1,
                    "maximum": 10
                }
            },
            "required": ["query"]
        })
    }

    fn execute<'a>(
        &'a self,
        args: Value,
        _ctx: &'a ToolContext,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ToolResult> + Send + 'a>> {
        Box::pin(async move {
            let query = args["query"].as_str().unwrap_or("");
            let limit = args["limit"].as_u64().unwrap_or(5).clamp(1, 10) as usize;

            if query.is_empty() {
                return ToolResult::error("No query provided");
            }

            // Use DuckDuckGo HTML search
            let encoded_query = urlencoding::encode(query);
            let url = format!("https://html.duckduckgo.com/html/?q={}", encoded_query);

            let client = reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(10))
                .user_agent("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36")
                .build()
                .map_err(|e| format!("Failed to create client: {}", e));

            let client = match client {
                Ok(c) => c,
                Err(e) => return ToolResult::error(e),
            };

            let response = client.get(&url).send().await;
            let response = match response {
                Ok(r) => r,
                Err(e) => return ToolResult::error(format!("Search request failed: {}", e)),
            };

            let html = match response.text().await {
                Ok(h) => h,
                Err(e) => return ToolResult::error(format!("Failed to read response: {}", e)),
            };

            // Parse results from HTML
            let results = parse_duckduckgo_results(&html, limit);
            
            if results.is_empty() {
                return ToolResult::success("No results found.");
            }

            let output = results.join("\n\n");
            ToolResult::success(output)
        })
    }
}

/// Parse DuckDuckGo HTML results
fn parse_duckduckgo_results(html: &str, limit: usize) -> Vec<String> {
    let mut results = Vec::new();
    
    // Simple HTML parsing - look for result divs
    // DuckDuckGo HTML format: <div class="result">...</div>
    let result_pattern = r#"class="result""#;
    let mut search_start = 0;
    
    while let Some(pos) = html[search_start..].find(result_pattern) {
        if results.len() >= limit {
            break;
        }
        
        let start = search_start + pos;
        let div_start = html[..start].rfind('<').unwrap_or(start);
        
        // Find the end of this div
        if let Some(div_end) = find_div_end(&html[div_start..]) {
            let result_html = &html[div_start..div_start + div_end];
            
            // Extract title and URL
            if let Some(result) = extract_result_info(result_html) {
                results.push(result);
            }
        }
        
        search_start = start + result_pattern.len();
        if search_start >= html.len() {
            break;
        }
    }
    
    results
}

/// Find the end of a div element
fn find_div_end(html: &str) -> Option<usize> {
    let mut depth = 0;
    let mut chars = html.char_indices().peekable();
    
    while let Some((i, c)) = chars.next() {
        if c == '<' {
            if let Some(&(_, next_c)) = chars.peek() {
                if next_c == '/' {
                    // Check if this is </div>
                    if html[i..].starts_with("</div>") {
                        if depth == 0 {
                            return Some(i + 6);
                        }
                        depth -= 1;
                    }
                } else if html[i..].starts_with("<div") {
                    depth += 1;
                }
            }
        }
    }
    
    None
}

/// Extract title and snippet from result HTML
fn extract_result_info(html: &str) -> Option<String> {
    // Extract title from <a class="result__a"> tag
    let title = extract_tag_content(html, r#"class="result__a""#, "a");
    let snippet = extract_tag_content(html, r#"class="result__snippet""#, "a");
    
    if title.is_empty() && snippet.is_empty() {
        return None;
    }
    
    let mut result = String::new();
    if !title.is_empty() {
        result.push_str(&format!("Title: {}\n", title));
    }
    if !snippet.is_empty() {
        result.push_str(&format!("Snippet: {}", snippet));
    }
    
    Some(result)
}

/// Extract content from a tag
fn extract_tag_content(html: &str, attr_pattern: &str, tag: &str) -> String {
    if let Some(pos) = html.find(attr_pattern) {
        let start = html[pos..].find('>').map(|i| pos + i + 1).unwrap_or(pos);
        let end_tag = format!("</{}>", tag);
        if let Some(end) = html[start..].find(&end_tag) {
            let content = &html[start..start + end];
            // Strip any nested HTML tags
            return strip_html_tags(content);
        }
    }
    String::new()
}

/// Strip HTML tags from text
fn strip_html_tags(html: &str) -> String {
    let mut result = String::new();
    let mut in_tag = false;
    
    for c in html.chars() {
        if c == '<' {
            in_tag = true;
        } else if c == '>' {
            in_tag = false;
        } else if !in_tag {
            result.push(c);
        }
    }
    
    // Decode common HTML entities
    result
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#x27;", "'")
        .replace("&#x2F;", "/")
        .trim()
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_strip_html_tags() {
        let html = "<b>Bold</b> and <i>italic</i>";
        assert_eq!(strip_html_tags(html), "Bold and italic");
    }

    #[test]
    fn test_html_entities() {
        let html = "Test &amp; Example &lt;tag&gt;";
        assert_eq!(strip_html_tags(html), "Test & Example <tag>");
    }
}
