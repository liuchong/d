//! Thinking mode for extended reasoning with token budgets
//!
//! Provides different levels of reasoning depth:
//! - None: No extended reasoning
//! - Minimal: 1K tokens
//! - Light: 4K tokens
//! - Standard: 8K tokens
//! - Deep: 16K tokens
//! - Exhaustive: 32K tokens

use serde::{Deserialize, Serialize};
use std::fmt;
use std::time::{Duration, Instant};

/// Token budget levels for thinking mode
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum ThinkingBudget {
    /// No extended reasoning
    None,
    /// Minimal thinking (1K tokens)
    Minimal,
    /// Light thinking (4K tokens)
    Light,
    /// Standard thinking (8K tokens)
    Standard,
    /// Deep thinking (16K tokens)
    Deep,
    /// Exhaustive thinking (32K tokens)
    Exhaustive,
}

impl ThinkingBudget {
    /// Get token limit for this budget level
    pub fn token_limit(&self) -> usize {
        match self {
            ThinkingBudget::None => 0,
            ThinkingBudget::Minimal => 1_000,
            ThinkingBudget::Light => 4_000,
            ThinkingBudget::Standard => 8_000,
            ThinkingBudget::Deep => 16_000,
            ThinkingBudget::Exhaustive => 32_000,
        }
    }

    /// Get display name
    pub fn name(&self) -> &'static str {
        match self {
            ThinkingBudget::None => "none",
            ThinkingBudget::Minimal => "minimal",
            ThinkingBudget::Light => "light",
            ThinkingBudget::Standard => "standard",
            ThinkingBudget::Deep => "deep",
            ThinkingBudget::Exhaustive => "exhaustive",
        }
    }

    /// Get description
    pub fn description(&self) -> &'static str {
        match self {
            ThinkingBudget::None => "No extended reasoning",
            ThinkingBudget::Minimal => "Quick reasoning (1K tokens)",
            ThinkingBudget::Light => "Light reasoning (4K tokens)",
            ThinkingBudget::Standard => "Standard reasoning (8K tokens)",
            ThinkingBudget::Deep => "Deep analysis (16K tokens)",
            ThinkingBudget::Exhaustive => "Exhaustive analysis (32K tokens)",
        }
    }

    /// Get all variants
    pub fn all() -> &'static [ThinkingBudget] {
        &[
            ThinkingBudget::None,
            ThinkingBudget::Minimal,
            ThinkingBudget::Light,
            ThinkingBudget::Standard,
            ThinkingBudget::Deep,
            ThinkingBudget::Exhaustive,
        ]
    }

    /// Parse from string
    pub fn parse(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "none" | "off" => Some(ThinkingBudget::None),
            "minimal" | "min" => Some(ThinkingBudget::Minimal),
            "light" => Some(ThinkingBudget::Light),
            "standard" | "normal" => Some(ThinkingBudget::Standard),
            "deep" => Some(ThinkingBudget::Deep),
            "exhaustive" | "max" => Some(ThinkingBudget::Exhaustive),
            _ => None,
        }
    }
}

impl fmt::Display for ThinkingBudget {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.name())
    }
}

impl Default for ThinkingBudget {
    fn default() -> Self {
        ThinkingBudget::None
    }
}

/// Thinking session tracking
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThinkingSession {
    pub budget: ThinkingBudget,
    pub started_at: String,
    #[serde(skip)]
    pub start_time: Option<Instant>,
    pub tokens_used: usize,
    pub status: ThinkingStatus,
}

/// Thinking status
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ThinkingStatus {
    Idle,
    Thinking,
    Completed,
    Error,
}

impl ThinkingSession {
    /// Create a new thinking session
    pub fn new(budget: ThinkingBudget) -> Self {
        Self {
            budget,
            started_at: chrono::Utc::now().to_rfc3339(),
            start_time: Some(Instant::now()),
            tokens_used: 0,
            status: ThinkingStatus::Thinking,
        }
    }

    /// Record tokens used
    pub fn record_tokens(&mut self, tokens: usize) {
        self.tokens_used += tokens;
    }

    /// Check if budget exceeded
    pub fn is_budget_exceeded(&self) -> bool {
        self.tokens_used > self.budget.token_limit()
    }

    /// Get elapsed time
    pub fn elapsed(&self) -> Option<Duration> {
        self.start_time.map(|t| t.elapsed())
    }

    /// Complete the session
    pub fn complete(&mut self) {
        self.status = ThinkingStatus::Completed;
    }

    /// Mark as error
    pub fn error(&mut self) {
        self.status = ThinkingStatus::Error;
    }

    /// Get summary
    pub fn summary(&self) -> String {
        let elapsed = self.elapsed()
            .map(|d| format!("{:?}", d))
            .unwrap_or_else(|| "unknown".to_string());
        
        format!(
            "Thinking ({}): {}/{} tokens, time: {}",
            self.budget.name(),
            self.tokens_used,
            self.budget.token_limit(),
            elapsed
        )
    }
}

/// Thinking mode configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThinkingConfig {
    /// Default budget level
    pub default_budget: ThinkingBudget,
    /// Whether thinking mode is enabled
    pub enabled: bool,
    /// Auto-enable for complex queries
    pub auto_enable: bool,
    /// Threshold for auto-enable (token estimate)
    pub auto_threshold: usize,
}

impl Default for ThinkingConfig {
    fn default() -> Self {
        Self {
            default_budget: ThinkingBudget::Standard,
            enabled: false,
            auto_enable: true,
            auto_threshold: 500,
        }
    }
}

/// Thinking mode manager
pub struct ThinkingManager {
    config: ThinkingConfig,
    current_session: Option<ThinkingSession>,
    history: Vec<ThinkingSession>,
}

impl ThinkingManager {
    /// Create a new thinking manager
    pub fn new(config: ThinkingConfig) -> Self {
        Self {
            config,
            current_session: None,
            history: Vec::new(),
        }
    }

    /// Check if thinking mode is active
    pub fn is_active(&self) -> bool {
        self.current_session.is_some()
    }

    /// Get current budget level
    pub fn current_budget(&self) -> ThinkingBudget {
        self.current_session
            .as_ref()
            .map(|s| s.budget)
            .unwrap_or(self.config.default_budget)
    }

    /// Start a thinking session
    pub fn start(&mut self, budget: Option<ThinkingBudget>) -> &ThinkingSession {
        // Complete any existing session
        if let Some(mut session) = self.current_session.take() {
            session.complete();
            self.history.push(session);
        }

        let budget = budget.unwrap_or(self.config.default_budget);
        self.current_session = Some(ThinkingSession::new(budget));
        self.current_session.as_ref().unwrap()
    }

    /// Stop current session
    pub fn stop(&mut self) -> Option<ThinkingSession> {
        if let Some(mut session) = self.current_session.take() {
            session.complete();
            self.history.push(session.clone());
            Some(session)
        } else {
            None
        }
    }

    /// Record tokens used in current session
    pub fn record_tokens(&mut self, tokens: usize) {
        if let Some(session) = &mut self.current_session {
            session.record_tokens(tokens);
        }
    }

    /// Get current session
    pub fn current_session(&self) -> Option<&ThinkingSession> {
        self.current_session.as_ref()
    }

    /// Get session history
    pub fn history(&self) -> &[ThinkingSession] {
        &self.history
    }

    /// Toggle thinking mode
    pub fn toggle(&mut self) -> bool {
        self.config.enabled = !self.config.enabled;
        self.config.enabled
    }

    /// Set budget level
    pub fn set_budget(&mut self, budget: ThinkingBudget) {
        self.config.default_budget = budget;
    }

    /// Get configuration
    pub fn config(&self) -> &ThinkingConfig {
        &self.config
    }

    /// Get total tokens used across all sessions
    pub fn total_tokens_used(&self) -> usize {
        let current = self.current_session.as_ref().map(|s| s.tokens_used).unwrap_or(0);
        let history: usize = self.history.iter().map(|s| s.tokens_used).sum();
        current + history
    }

    /// Check if should auto-enable for a query
    pub fn should_auto_enable(&self, query: &str) -> bool {
        if !self.config.auto_enable || self.config.enabled {
            return false;
        }
        
        // Simple heuristic: long queries or complex keywords
        let tokens = query.split_whitespace().count();
        if tokens > self.config.auto_threshold {
            return true;
        }

        // Complex keywords
        let complex_keywords = [
            "analyze", "design", "architecture", "complex",
            "algorithm", "optimize", "refactor", "debug",
            "explain", "compare", "evaluate", "review",
        ];
        
        let query_lower = query.to_lowercase();
        complex_keywords.iter().any(|kw| query_lower.contains(kw))
    }

    /// Format status for display
    pub fn format_status(&self) -> String {
        if let Some(session) = &self.current_session {
            session.summary()
        } else {
            format!("Thinking: {}", if self.config.enabled { "enabled" } else { "disabled" })
        }
    }
}

/// Create system prompt addon for thinking mode
pub fn create_thinking_prompt(budget: ThinkingBudget) -> Option<String> {
    match budget {
        ThinkingBudget::None => None,
        _ => {
            let instruction = format!(
                "You are in {} thinking mode ({} token budget). \
                 Take time to think step by step, consider edge cases, \
                 and provide thorough analysis before giving your final answer.",
                budget.name(),
                budget.token_limit()
            );
            Some(instruction)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_thinking_budget_tokens() {
        assert_eq!(ThinkingBudget::None.token_limit(), 0);
        assert_eq!(ThinkingBudget::Minimal.token_limit(), 1_000);
        assert_eq!(ThinkingBudget::Standard.token_limit(), 8_000);
        assert_eq!(ThinkingBudget::Exhaustive.token_limit(), 32_000);
    }

    #[test]
    fn test_thinking_budget_parse() {
        assert_eq!(ThinkingBudget::parse("none"), Some(ThinkingBudget::None));
        assert_eq!(ThinkingBudget::parse("minimal"), Some(ThinkingBudget::Minimal));
        assert_eq!(ThinkingBudget::parse("deep"), Some(ThinkingBudget::Deep));
        assert_eq!(ThinkingBudget::parse("unknown"), None);
    }

    #[test]
    fn test_thinking_session() {
        let mut session = ThinkingSession::new(ThinkingBudget::Light);
        assert_eq!(session.tokens_used, 0);
        assert_eq!(session.budget, ThinkingBudget::Light);

        session.record_tokens(500);
        assert_eq!(session.tokens_used, 500);
        assert!(!session.is_budget_exceeded());

        session.record_tokens(4_000);
        assert!(session.is_budget_exceeded());
    }

    #[test]
    fn test_thinking_manager() {
        let config = ThinkingConfig::default();
        let mut manager = ThinkingManager::new(config);

        assert!(!manager.is_active());
        
        manager.start(Some(ThinkingBudget::Standard));
        assert!(manager.is_active());
        assert_eq!(manager.current_budget(), ThinkingBudget::Standard);

        manager.record_tokens(100);
        assert_eq!(manager.current_session().unwrap().tokens_used, 100);

        manager.stop();
        assert!(!manager.is_active());
        assert_eq!(manager.history().len(), 1);
    }

    #[test]
    fn test_auto_enable() {
        let config = ThinkingConfig {
            enabled: false,
            auto_enable: true,
            auto_threshold: 10,
            ..Default::default()
        };
        let manager = ThinkingManager::new(config);

        assert!(manager.should_auto_enable("This is a very long query with many words that should trigger auto enable"));
        assert!(manager.should_auto_enable("Please analyze this code"));
        assert!(!manager.should_auto_enable("Hi"));
    }

    #[test]
    fn test_thinking_prompt() {
        assert!(create_thinking_prompt(ThinkingBudget::None).is_none());
        assert!(create_thinking_prompt(ThinkingBudget::Deep).is_some());
        
        let prompt = create_thinking_prompt(ThinkingBudget::Standard).unwrap();
        assert!(prompt.contains("standard"));
        assert!(prompt.contains("8000"));
    }
}
