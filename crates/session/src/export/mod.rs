//! Session export/import functionality
//!
//! Provides portability for sessions:
//! - Export to JSON, Markdown, HTML
//! - Import from various formats
//! - Archive management

use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Export format
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExportFormat {
    /// JSON format (full data)
    Json,
    /// Markdown format (readable)
    Markdown,
    /// HTML format (rich presentation)
    Html,
    /// Plain text
    Text,
}

impl ExportFormat {
    pub fn extension(&self) -> &'static str {
        match self {
            ExportFormat::Json => "json",
            ExportFormat::Markdown => "md",
            ExportFormat::Html => "html",
            ExportFormat::Text => "txt",
        }
    }

    pub fn content_type(&self) -> &'static str {
        match self {
            ExportFormat::Json => "application/json",
            ExportFormat::Markdown => "text/markdown",
            ExportFormat::Html => "text/html",
            ExportFormat::Text => "text/plain",
        }
    }
}

/// Session export data
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionExport {
    /// Export metadata
    pub meta: ExportMeta,
    /// Session data
    pub session: ExportedSession,
    /// Additional context
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context: Option<HashMap<String, serde_json::Value>>,
}

/// Export metadata
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExportMeta {
    pub version: String,
    pub exported_at: String,
    pub exporter: String,
    pub format: String,
}

impl ExportMeta {
    pub fn new(format: &str) -> Self {
        Self {
            version: env!("CARGO_PKG_VERSION").to_string(),
            exported_at: Utc::now().to_rfc3339(),
            exporter: "chat-session".to_string(),
            format: format.to_string(),
        }
    }
}

/// Exported session
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExportedSession {
    pub id: String,
    pub title: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    pub messages: Vec<ExportedMessage>,
    pub metadata: HashMap<String, String>,
}

/// Exported message
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExportedMessage {
    pub role: String,
    pub content: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timestamp: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<HashMap<String, String>>,
}

/// Session exporter
pub struct SessionExporter;

impl SessionExporter {
    /// Export session to JSON
    pub fn to_json(export: &SessionExport) -> anyhow::Result<String> {
        Ok(serde_json::to_string_pretty(export)?)
    }

    /// Export session to Markdown
    pub fn to_markdown(export: &SessionExport) -> String {
        let mut lines = Vec::new();

        // Header
        lines.push(format!("# Session: {}", export.session.id));
        if let Some(ref title) = export.session.title {
            lines.push(format!("## {}", title));
        }
        lines.push(String::new());
        
        // Metadata
        lines.push("## Metadata".to_string());
        lines.push(format!("- **Created:** {}", export.session.created_at));
        lines.push(format!("- **Updated:** {}", export.session.updated_at));
        lines.push(format!("- **Exported:** {}", export.meta.exported_at));
        lines.push(String::new());

        // Messages
        lines.push("## Conversation".to_string());
        lines.push(String::new());

        for msg in &export.session.messages {
            let role_icon = match msg.role.as_str() {
                "user" => "👤",
                "assistant" => "🤖",
                "system" => "⚙️",
                _ => "💬",
            };

            lines.push(format!("### {} **{}**", role_icon, capitalize(&msg.role)));
            if let Some(ref ts) = msg.timestamp {
                lines.push(format!("*{ts}*"));
            }
            lines.push(String::new());
            lines.push(msg.content.clone());
            lines.push(String::new());
        }

        lines.join("\n")
    }

    /// Export session to HTML
    pub fn to_html(export: &SessionExport) -> String {
        let mut html = String::new();

        html.push_str("<!DOCTYPE html>\n");
        html.push_str("<html>\n<head>\n");
        html.push_str(&format!("<title>Session: {}</title>\n", export.session.id));
        html.push_str("<style>\n");
        html.push_str(include_str!("default.css"));
        html.push_str("</style>\n");
        html.push_str("</head>\n<body>\n");

        // Header
        html.push_str("<div class=\"header\">\n");
        html.push_str(&format!("<h1>Session: {}</h1>\n", export.session.id));
        if let Some(ref title) = export.session.title {
            html.push_str(&format!("<h2>{}</h2>\n", html_escape(title)));
        }
        html.push_str("</div>\n");

        // Metadata
        html.push_str("<div class=\"metadata\">\n");
        html.push_str(&format!("<p><strong>Created:</strong> {}</p>\n", export.session.created_at));
        html.push_str(&format!("<p><strong>Updated:</strong> {}</p>\n", export.session.updated_at));
        html.push_str(&format!("<p><strong>Exported:</strong> {}</p>\n", export.meta.exported_at));
        html.push_str("</div>\n");

        // Messages
        html.push_str("<div class=\"messages\">\n");
        
        for msg in &export.session.messages {
            let role_class = match msg.role.as_str() {
                "user" => "user",
                "assistant" => "assistant",
                "system" => "system",
                _ => "other",
            };

            html.push_str(&format!("<div class=\"message {}\">\n", role_class));
            html.push_str(&format!("<div class=\"role\">{}</div>\n", capitalize(&msg.role)));
            
            if let Some(ref ts) = msg.timestamp {
                html.push_str(&format!("<div class=\"timestamp\">{}</div>\n", ts));
            }

            html.push_str("<div class=\"content\"><pre>");
            html.push_str(&html_escape(&msg.content));
            html.push_str("</pre></div>\n");
            html.push_str("</div>\n");
        }

        html.push_str("</div>\n");
        html.push_str("</body>\n</html>");

        html
    }

    /// Export session to plain text
    pub fn to_text(export: &SessionExport) -> String {
        let mut lines = Vec::new();

        lines.push(format!("Session: {}", export.session.id));
        if let Some(ref title) = export.session.title {
            lines.push(format!("Title: {}", title));
        }
        lines.push(format!("Exported: {}", export.meta.exported_at));
        lines.push(String::new());
        lines.push("=".repeat(50));
        lines.push(String::new());

        for msg in &export.session.messages {
            lines.push(format!("[{}]", capitalize(&msg.role)));
            if let Some(ref ts) = msg.timestamp {
                lines.push(format!("Time: {}", ts));
            }
            lines.push(String::new());
            lines.push(msg.content.clone());
            lines.push(String::new());
            lines.push("-".repeat(50));
            lines.push(String::new());
        }

        lines.join("\n")
    }
}

/// Session importer
pub struct SessionImporter;

impl SessionImporter {
    /// Import from JSON
    pub fn from_json(json: &str) -> anyhow::Result<SessionExport> {
        Ok(serde_json::from_str(json)?)
    }

    /// Detect format from content
    pub fn detect_format(content: &str) -> Option<ExportFormat> {
        let trimmed = content.trim();
        
        if trimmed.starts_with('{') || trimmed.starts_with('[') {
            // Try to parse as JSON
            if serde_json::from_str::<serde_json::Value>(trimmed).is_ok() {
                return Some(ExportFormat::Json);
            }
        }
        
        if trimmed.starts_with("<!DOCTYPE html") || trimmed.starts_with("<html") {
            return Some(ExportFormat::Html);
        }
        
        if trimmed.starts_with("# Session:") || trimmed.starts_with("## ") {
            return Some(ExportFormat::Markdown);
        }
        
        // Default to text
        Some(ExportFormat::Text)
    }

    /// Import from markdown (basic parsing)
    pub fn from_markdown(content: &str) -> anyhow::Result<SessionExport> {
        let mut lines = content.lines().peekable();
        
        // Parse header
        let mut session_id = "imported".to_string();
        let mut title = None;
        let mut created_at = Utc::now().to_rfc3339();
        let mut updated_at = created_at.clone();
        
        while let Some(line) = lines.next() {
            let line = line.trim();
            if line.starts_with("# Session: ") {
                session_id = line[11..].to_string();
            } else if line.starts_with("## ") && line != "## Conversation" {
                title = Some(line[3..].to_string());
            } else if line == "## Conversation" {
                break;
            }
        }

        // Parse messages
        let mut messages = Vec::new();
        let mut current_role = String::new();
        let mut current_content = Vec::new();

        while let Some(line) = lines.next() {
            let trimmed = line.trim();
            
            if trimmed.starts_with("### ") && trimmed.contains("**") {
                // Save previous message
                if !current_role.is_empty() && !current_content.is_empty() {
                    messages.push(ExportedMessage {
                        role: current_role.clone(),
                        content: current_content.join("\n").trim().to_string(),
                        timestamp: None,
                        metadata: None,
                    });
                }
                
                // Parse new role
                if let Some(start) = trimmed.find("**") {
                    let after_start = &trimmed[start + 2..];
                    if let Some(end) = after_start.find("**") {
                        current_role = after_start[..end].to_lowercase();
                    }
                }
                current_content.clear();
            } else if !trimmed.is_empty() && !trimmed.starts_with('*') {
                current_content.push(line.to_string());
            }
        }

        // Save last message
        if !current_role.is_empty() && !current_content.is_empty() {
            messages.push(ExportedMessage {
                role: current_role,
                content: current_content.join("\n").trim().to_string(),
                timestamp: None,
                metadata: None,
            });
        }

        let session = ExportedSession {
            id: session_id,
            title,
            created_at,
            updated_at,
            messages,
            metadata: HashMap::new(),
        };

        Ok(SessionExport {
            meta: ExportMeta::new("markdown"),
            session,
            context: None,
        })
    }
}

/// Export archive manager
pub struct ExportArchive {
    base_path: PathBuf,
}

impl ExportArchive {
    /// Create a new archive manager
    pub fn new(base_path: impl AsRef<Path>) -> Self {
        Self {
            base_path: base_path.as_ref().to_path_buf(),
        }
    }

    /// Save export to archive
    pub async fn save(&self, export: &SessionExport, format: ExportFormat) -> anyhow::Result<PathBuf> {
        tokio::fs::create_dir_all(&self.base_path).await?;
        
        let filename = format!("{}_{}.{}", 
            sanitize_filename(&export.session.id),
            chrono::Local::now().format("%Y%m%d_%H%M%S"),
            format.extension()
        );
        
        let path = self.base_path.join(&filename);
        
        let content = match format {
            ExportFormat::Json => SessionExporter::to_json(export)?,
            ExportFormat::Markdown => SessionExporter::to_markdown(export),
            ExportFormat::Html => SessionExporter::to_html(export),
            ExportFormat::Text => SessionExporter::to_text(export),
        };
        
        tokio::fs::write(&path, content).await?;
        
        Ok(path)
    }

    /// List archived exports
    pub async fn list(&self) -> anyhow::Result<Vec<ArchiveEntry>> {
        let mut entries = Vec::new();
        
        if !self.base_path.exists() {
            return Ok(entries);
        }

        let mut dir = tokio::fs::read_dir(&self.base_path).await?;
        
        while let Some(entry) = dir.next_entry().await? {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) == Some("json") {
                if let Ok(metadata) = entry.metadata().await {
                    if let Ok(created) = metadata.created() {
                        let filename = path.file_stem()
                            .and_then(|s| s.to_str())
                            .unwrap_or("unknown")
                            .to_string();
                        
                        entries.push(ArchiveEntry {
                            filename,
                            path,
                            size: metadata.len(),
                            created: created.into(),
                        });
                    }
                }
            }
        }
        
        entries.sort_by(|a, b| b.created.cmp(&a.created));
        Ok(entries)
    }

    /// Load export from archive
    pub async fn load(&self, filename: &str) -> anyhow::Result<SessionExport> {
        let path = self.base_path.join(filename);
        let content = tokio::fs::read_to_string(&path).await?;
        SessionImporter::from_json(&content)
    }
}

/// Archive entry
#[derive(Debug, Clone)]
pub struct ArchiveEntry {
    pub filename: String,
    pub path: PathBuf,
    pub size: u64,
    pub created: std::time::SystemTime,
}

/// Helper: capitalize first letter
fn capitalize(s: &str) -> String {
    if s.is_empty() {
        return s.to_string();
    }
    let mut chars = s.chars();
    chars.next().unwrap().to_uppercase().to_string() + &chars.as_str().to_lowercase()
}

/// Helper: HTML escape
fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// Helper: sanitize filename
fn sanitize_filename(s: &str) -> String {
    s.chars()
        .map(|c| if c.is_alphanumeric() || c == '-' || c == '_' { c } else { '_' })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_export() -> SessionExport {
        SessionExport {
            meta: ExportMeta::new("json"),
            session: ExportedSession {
                id: "test-session".to_string(),
                title: Some("Test Session".to_string()),
                created_at: "2024-01-01T00:00:00Z".to_string(),
                updated_at: "2024-01-01T01:00:00Z".to_string(),
                messages: vec![
                    ExportedMessage {
                        role: "user".to_string(),
                        content: "Hello!".to_string(),
                        timestamp: Some("2024-01-01T00:00:00Z".to_string()),
                        metadata: None,
                    },
                    ExportedMessage {
                        role: "assistant".to_string(),
                        content: "Hi there!".to_string(),
                        timestamp: Some("2024-01-01T00:00:01Z".to_string()),
                        metadata: None,
                    },
                ],
                metadata: HashMap::new(),
            },
            context: None,
        }
    }

    #[test]
    fn test_export_to_markdown() {
        let export = create_test_export();
        let md = SessionExporter::to_markdown(&export);
        
        assert!(md.contains("# Session: test-session"));
        assert!(md.contains("## Test Session"));
        assert!(md.contains("### 👤 **User**"));
        assert!(md.contains("### 🤖 **Assistant**"));
        assert!(md.contains("Hello!"));
    }

    #[test]
    fn test_export_to_text() {
        let export = create_test_export();
        let text = SessionExporter::to_text(&export);
        
        assert!(text.contains("Session: test-session"));
        assert!(text.contains("[User]"));
        assert!(text.contains("Hello!"));
    }

    #[test]
    fn test_export_to_html() {
        let export = create_test_export();
        let html = SessionExporter::to_html(&export);
        
        assert!(html.contains("<!DOCTYPE html>"));
        assert!(html.contains("test-session"));
        assert!(html.contains("message user"));
        assert!(html.contains("message assistant"));
    }

    #[test]
    fn test_import_from_markdown() {
        let md = r#"# Session: my-session
## My Title

## Conversation

### 👤 **User**
*2024-01-01T00:00:00Z*

Hello!

### 🤖 **Assistant**
Hi there!
"#;

        let export = SessionImporter::from_markdown(md).unwrap();
        
        assert_eq!(export.session.id, "my-session");
        assert_eq!(export.session.title, Some("My Title".to_string()));
        assert_eq!(export.session.messages.len(), 2);
        assert_eq!(export.session.messages[0].role, "user");
    }

    #[test]
    fn test_detect_format() {
        assert_eq!(SessionImporter::detect_format("{}"), Some(ExportFormat::Json));
        assert_eq!(SessionImporter::detect_format("<!DOCTYPE html>"), Some(ExportFormat::Html));
        assert_eq!(SessionImporter::detect_format("# Session: test"), Some(ExportFormat::Markdown));
        assert_eq!(SessionImporter::detect_format("plain text"), Some(ExportFormat::Text));
    }

    #[test]
    fn test_html_escape() {
        assert_eq!(html_escape("<script>"), "&lt;script&gt;");
        assert_eq!(html_escape("a & b"), "a &amp; b");
    }
}
