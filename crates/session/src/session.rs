//! Session management with persistence and metadata
//!
//! Provides:
//! - Session creation and management
//! - Automatic persistence
//! - Git branch tracking
//! - Message history with metadata

use std::collections::HashMap;
use std::path::PathBuf;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};



/// Session metadata for lightweight listing
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionInfo {
    pub id: String,
    pub name: String,
    pub created_at: DateTime<Utc>,
    pub last_accessed: DateTime<Utc>,
    pub message_count: usize,
    pub working_dir: PathBuf,
    pub git_branch: Option<String>,
    pub summary: Option<String>,
    pub model: Option<String>,
}

impl SessionInfo {
    /// Create from full session
    pub fn from_session(session: &Session) -> Self {
        Self {
            id: session.id.clone(),
            name: session.name.clone(),
            created_at: session.created_at,
            last_accessed: session.last_accessed,
            message_count: session.messages.len(),
            working_dir: session.working_dir.clone(),
            git_branch: session.git_branch.clone(),
            summary: session.summary.clone(),
            model: session.model.clone(),
        }
    }

    /// Format for display
    pub fn display(&self) -> String {
        let time_str = self.last_accessed.format("%Y-%m-%d %H:%M");
        let msg_info = format!("{} messages", self.message_count);
        let git_info = self.git_branch.as_ref()
            .map(|b| format!(" [{}]", b))
            .unwrap_or_default();
        
        format!(
            "{} | {} | {} | {}{}",
            self.id[..8].to_string(),
            self.name,
            time_str,
            msg_info,
            git_info
        )
    }
}

/// Chat message with metadata
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionMessage {
    pub role: String,
    pub content: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning_content: Option<String>,
    pub timestamp: DateTime<Utc>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tokens: Option<usize>,
}

impl SessionMessage {
    /// Create user message
    pub fn user(content: impl Into<String>) -> Self {
        Self {
            role: "user".to_string(),
            content: content.into(),
            reasoning_content: None,
            timestamp: Utc::now(),
            model: None,
            tokens: None,
        }
    }

    /// Create assistant message
    pub fn assistant(content: impl Into<String>) -> Self {
        Self {
            role: "assistant".to_string(),
            content: content.into(),
            reasoning_content: None,
            timestamp: Utc::now(),
            model: None,
            tokens: None,
        }
    }

    /// Create assistant message with reasoning
    pub fn assistant_with_reasoning(
        content: impl Into<String>,
        reasoning: impl Into<String>,
    ) -> Self {
        Self {
            role: "assistant".to_string(),
            content: content.into(),
            reasoning_content: Some(reasoning.into()),
            timestamp: Utc::now(),
            model: None,
            tokens: None,
        }
    }

    /// Create system message
    pub fn system(content: impl Into<String>) -> Self {
        Self {
            role: "system".to_string(),
            content: content.into(),
            reasoning_content: None,
            timestamp: Utc::now(),
            model: None,
            tokens: None,
        }
    }

    /// Create tool message
    pub fn tool(_tool_call_id: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            role: "tool".to_string(),
            content: content.into(),
            reasoning_content: None,
            timestamp: Utc::now(),
            model: None,
            tokens: None,
        }
    }

    /// Set model info
    pub fn with_model(mut self, model: impl Into<String>) -> Self {
        self.model = Some(model.into());
        self
    }

    /// Estimate tokens (simple approximation)
    pub fn estimate_tokens(&self) -> usize {
        self.tokens.unwrap_or_else(|| {
            // Rough estimate: 1 token ≈ 4 characters
            (self.content.len() / 4) + (self.reasoning_content.as_ref().map(|r| r.len() / 4).unwrap_or(0))
        })
    }
}

/// Full session data
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    pub id: String,
    pub name: String,
    pub messages: Vec<SessionMessage>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub last_accessed: DateTime<Utc>,
    pub working_dir: PathBuf,
    pub git_branch: Option<String>,
    pub summary: Option<String>,
    pub model: Option<String>,
    pub provider: Option<String>,
    pub metadata: HashMap<String, String>,
}

impl Session {
    /// Create new session with auto-generated ID
    pub fn new(name: Option<String>) -> Self {
        let id = uuid::Uuid::new_v4().to_string();
        let now = Utc::now();
        let working_dir = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        let git_branch = detect_git_branch(&working_dir);

        Self {
            id,
            name: name.unwrap_or_else(|| "New Session".to_string()),
            messages: Vec::new(),
            created_at: now,
            updated_at: now,
            last_accessed: now,
            working_dir,
            git_branch,
            summary: None,
            model: None,
            provider: None,
            metadata: HashMap::new(),
        }
    }

    /// Create with specific ID
    pub fn with_id(id: impl Into<String>) -> Self {
        let id = id.into();
        let now = Utc::now();
        let working_dir = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        let git_branch = detect_git_branch(&working_dir);

        Self {
            id,
            name: "New Session".to_string(),
            messages: Vec::new(),
            created_at: now,
            updated_at: now,
            last_accessed: now,
            working_dir,
            git_branch,
            summary: None,
            model: None,
            provider: None,
            metadata: HashMap::new(),
        }
    }

    /// Add message to session
    pub fn add_message(&mut self, message: SessionMessage) {
        self.messages.push(message);
        self.updated_at = Utc::now();
        self.last_accessed = self.updated_at;
    }

    /// Add message from llm::Message
    pub fn add_llm_message(&mut self, message: &llm::Message) {
        let session_msg = SessionMessage {
            role: match message.role {
                llm::MessageRole::System => "system".to_string(),
                llm::MessageRole::User => "user".to_string(),
                llm::MessageRole::Assistant => "assistant".to_string(),
                llm::MessageRole::Tool => "tool".to_string(),
            },
            content: message.content.clone(),
            reasoning_content: message.reasoning_content.clone(),
            timestamp: Utc::now(),
            model: None,
            tokens: None,
        };
        self.add_message(session_msg);
    }

    /// Add user message
    pub fn add_user_message(&mut self, content: impl Into<String>) {
        self.add_message(SessionMessage::user(content));
    }

    /// Add assistant message
    pub fn add_assistant_message(&mut self, content: impl Into<String>) {
        self.add_message(SessionMessage::assistant(content));
    }

    /// Update last accessed time
    pub fn touch(&mut self) {
        self.last_accessed = Utc::now();
    }

    /// Rename session
    pub fn rename(&mut self, name: impl Into<String>) {
        self.name = name.into();
        self.updated_at = Utc::now();
    }

    /// Set summary
    pub fn set_summary(&mut self, summary: impl Into<String>) {
        self.summary = Some(summary.into());
        self.updated_at = Utc::now();
    }

    /// Set model
    pub fn set_model(&mut self, model: impl Into<String>) {
        self.model = Some(model.into());
    }

    /// Set provider
    pub fn set_provider(&mut self, provider: impl Into<String>) {
        self.provider = Some(provider.into());
    }

    /// Get total message count
    pub fn message_count(&self) -> usize {
        self.messages.len()
    }

    /// Get total estimated tokens
    pub fn total_tokens(&self) -> usize {
        self.messages.iter().map(|m| m.estimate_tokens()).sum()
    }

    /// Get recent messages (last n)
    pub fn recent_messages(&self, n: usize) -> &[SessionMessage] {
        let start = self.messages.len().saturating_sub(n);
        &self.messages[start..]
    }

    /// Clear all messages
    pub fn clear(&mut self) {
        self.messages.clear();
        self.updated_at = Utc::now();
    }

    /// Get session info
    pub fn info(&self) -> SessionInfo {
        SessionInfo::from_session(self)
    }

    /// Convert to LLM messages
    pub fn to_llm_messages(&self) -> Vec<llm::Message> {
        self.messages.iter()
            .map(|m| llm::Message {
                role: match m.role.as_str() {
                    "system" => llm::MessageRole::System,
                    "assistant" => llm::MessageRole::Assistant,
                    "tool" => llm::MessageRole::Tool,
                    _ => llm::MessageRole::User,
                },
                content: m.content.clone(),
                reasoning_content: m.reasoning_content.clone(),
                tool_calls: None,
                tool_call_id: None,
                name: None,
            })
            .collect()
    }
}

impl Default for Session {
    fn default() -> Self {
        Self::new(None)
    }
}

/// Detect git branch for current directory
fn detect_git_branch(dir: &PathBuf) -> Option<String> {
    let git_dir = dir.join(".git");
    if !git_dir.exists() {
        // Try parent directories
        let mut current = dir.as_path();
        while let Some(parent) = current.parent() {
            if parent.join(".git").exists() {
                return read_git_branch(&parent.join(".git"));
            }
            current = parent;
        }
        return None;
    }
    
    read_git_branch(&git_dir)
}

/// Read git branch from .git directory
fn read_git_branch(git_dir: &PathBuf) -> Option<String> {
    // Try HEAD file first
    let head_file = git_dir.join("HEAD");
    if let Ok(content) = std::fs::read_to_string(&head_file) {
        let content = content.trim();
        if content.starts_with("ref: refs/heads/") {
            return Some(content[16..].to_string());
        }
        // Detached HEAD
        return Some(content[..8].to_string());
    }
    None
}

/// Session search criteria
#[derive(Debug, Clone, Default)]
pub struct SessionSearch {
    pub query: Option<String>,
    pub working_dir: Option<PathBuf>,
    pub git_branch: Option<String>,
    pub after: Option<DateTime<Utc>>,
    pub before: Option<DateTime<Utc>>,
    pub limit: Option<usize>,
}

impl SessionSearch {
    /// Create new search
    pub fn new() -> Self {
        Self::default()
    }

    /// Set search query
    pub fn query(mut self, q: impl Into<String>) -> Self {
        self.query = Some(q.into());
        self
    }

    /// Set working directory filter
    pub fn in_directory(mut self, dir: impl Into<PathBuf>) -> Self {
        self.working_dir = Some(dir.into());
        self
    }

    /// Set git branch filter
    pub fn on_branch(mut self, branch: impl Into<String>) -> Self {
        self.git_branch = Some(branch.into());
        self
    }

    /// Set time range
    pub fn after(mut self, time: DateTime<Utc>) -> Self {
        self.after = Some(time);
        self
    }

    /// Set time range
    pub fn before(mut self, time: DateTime<Utc>) -> Self {
        self.before = Some(time);
        self
    }

    /// Set result limit
    pub fn limit(mut self, n: usize) -> Self {
        self.limit = Some(n);
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_session_creation() {
        let session = Session::new(Some("Test Session".to_string()));
        assert_eq!(session.name, "Test Session");
        assert!(session.messages.is_empty());
    }

    #[test]
    fn test_add_messages() {
        let mut session = Session::new(None);
        session.add_user_message("Hello");
        session.add_assistant_message("Hi there!");
        
        assert_eq!(session.message_count(), 2);
    }

    #[test]
    fn test_session_info() {
        let mut session = Session::new(Some("My Session".to_string()));
        session.add_user_message("Test");
        
        let info = session.info();
        assert_eq!(info.name, "My Session");
        assert_eq!(info.message_count, 1);
    }

    #[test]
    fn test_message_tokens() {
        let msg = SessionMessage::user("Hello world, this is a test message.");
        let tokens = msg.estimate_tokens();
        assert!(tokens > 0);
    }
}
