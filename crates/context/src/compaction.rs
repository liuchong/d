//! Context compaction strategies

use crate::token::{estimate_message_tokens, estimate_messages_tokens, estimate_tokens};
use llm::Message;
use std::collections::HashSet;

/// Compaction configuration
#[derive(Debug, Clone)]
pub struct CompactionConfig {
    /// Maximum tokens before triggering compaction
    pub max_context_tokens: usize,
    /// Target tokens after compaction
    pub target_context_tokens: usize,
    /// Minimum messages to always keep
    pub min_messages_to_keep: usize,
    /// Preserve system messages
    pub preserve_system_messages: bool,
    /// Preserve first user message
    pub preserve_first_user_message: bool,
}

impl Default for CompactionConfig {
    fn default() -> Self {
        Self {
            max_context_tokens: 24000,
            target_context_tokens: 12000,
            min_messages_to_keep: 4,
            preserve_system_messages: true,
            preserve_first_user_message: true,
        }
    }
}

impl CompactionConfig {
    /// Small context configuration (for smaller models)
    pub fn small_context() -> Self {
        Self {
            max_context_tokens: 6000,
            target_context_tokens: 3000,
            min_messages_to_keep: 4,
            preserve_system_messages: true,
            preserve_first_user_message: true,
        }
    }

    /// Large context configuration (for larger models)
    pub fn large_context() -> Self {
        Self {
            max_context_tokens: 80000,
            target_context_tokens: 40000,
            min_messages_to_keep: 6,
            preserve_system_messages: true,
            preserve_first_user_message: true,
        }
    }
}

/// Compaction result
#[derive(Debug, Clone)]
pub struct CompactionResult {
    /// Compacted messages
    pub messages: Vec<Message>,
    /// Estimated token count after compaction
    pub estimated_tokens: usize,
    /// Number of messages compacted/removed
    pub compacted_count: usize,
}

/// Compaction statistics
#[derive(Debug, Default, Clone)]
pub struct CompactionStats {
    pub total_compactions: usize,
    pub total_messages_removed: usize,
    pub total_tokens_saved: usize,
    pub last_compaction_time: Option<chrono::DateTime<chrono::Utc>>,
}

impl CompactionStats {
    pub fn record_compaction(&mut self, messages_removed: usize, tokens_saved: usize) {
        self.total_compactions += 1;
        self.total_messages_removed += messages_removed;
        self.total_tokens_saved += tokens_saved;
        self.last_compaction_time = Some(chrono::Utc::now());
    }
}

/// Compaction strategy
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompactionStrategy {
    /// Simple strategy: summarize old messages
    Simple,
    /// Importance-based: keep most important messages
    ImportanceBased,
    /// Sliding window: keep recent messages
    SlidingWindow,
}

/// Simple compaction strategy
pub struct SimpleCompaction {
    config: CompactionConfig,
}

impl SimpleCompaction {
    pub fn new() -> Self {
        Self {
            config: CompactionConfig::default(),
        }
    }

    pub fn with_config(config: CompactionConfig) -> Self {
        Self { config }
    }

    pub fn should_compact(&self, messages: &[Message], system_prompt: Option<&str>) -> bool {
        let mut total = 0;
        if let Some(sp) = system_prompt {
            total += estimate_tokens(sp);
        }
        total += estimate_messages_tokens(messages);
        total > self.config.max_context_tokens
    }

    pub fn compact(&self, messages: &[Message]) -> CompactionResult {
        if messages.len() <= self.config.min_messages_to_keep {
            return CompactionResult {
                messages: messages.to_vec(),
                estimated_tokens: estimate_messages_tokens(messages),
                compacted_count: 0,
            };
        }

        let keep_recent = 6;
        let keep_first = if self.config.preserve_first_user_message {
            2
        } else {
            1
        };

        let summary_threshold = messages.len().saturating_sub(keep_recent);

        let mut new_messages: Vec<Message> = Vec::new();

        // Add first messages
        for msg in messages.iter().take(keep_first.min(messages.len())) {
            new_messages.push(msg.clone());
        }

        let mut compacted_count = 0;

        if summary_threshold > keep_first {
            let mut summary_text = String::from("[Earlier conversation summary]:\n\n");
            let mut has_content = false;

            for msg in messages.iter().take(summary_threshold).skip(keep_first) {
                compacted_count += 1;
                if !msg.content.is_empty() {
                    let preview = if msg.content.len() > 100 {
                        format!("{}...", &msg.content[..100])
                    } else {
                        msg.content.clone()
                    };
                    summary_text.push_str(&preview);
                    summary_text.push('\n');
                    has_content = true;
                }
            }

            if has_content {
                new_messages.push(Message::system(&summary_text));
            }
        }

        // Add recent messages
        let start_idx = messages.len().saturating_sub(keep_recent);
        for msg in messages.iter().skip(start_idx) {
            new_messages.push(msg.clone());
        }

        CompactionResult {
            estimated_tokens: estimate_messages_tokens(&new_messages),
            compacted_count,
            messages: new_messages,
        }
    }
}

impl Default for SimpleCompaction {
    fn default() -> Self {
        Self::new()
    }
}

/// Message importance for importance-based compaction
#[derive(Debug, Clone)]
struct MessageImportance {
    index: usize,
    score: f32,
}

/// Importance-based compaction
pub struct ImportanceCompaction {
    config: CompactionConfig,
}

impl ImportanceCompaction {
    pub fn new() -> Self {
        Self {
            config: CompactionConfig::default(),
        }
    }

    pub fn should_compact(&self, messages: &[Message], system_prompt: Option<&str>) -> bool {
        let mut total = 0;
        if let Some(sp) = system_prompt {
            total += estimate_tokens(sp);
        }
        total += estimate_messages_tokens(messages);
        total > self.config.max_context_tokens
    }

    fn calculate_importance(msg: &Message, index: usize, total: usize) -> f32 {
        let mut score = 0.0;

        // Position-based importance
        if index == 0 {
            score += 10.0;
        }
        if index == 1 {
            score += 8.0;
        }
        if index == total.saturating_sub(1) {
            score += 9.0;
        }
        if index == total.saturating_sub(2) {
            score += 7.0;
        }

        // Role-based importance
        use llm::MessageRole;
        match msg.role {
            MessageRole::System => score += 5.0,
            MessageRole::Assistant => {
                if msg.tool_calls.is_some() {
                    score += 4.0;
                }
            }
            MessageRole::Tool => score += 3.0,
            _ => {}
        }

        // Content-based importance
        if msg.content.len() > 1000 {
            score += 2.0;
        } else if msg.content.len() > 100 {
            score += 1.0;
        }

        // Tool call ID presence
        if msg.tool_call_id.is_some() {
            score += 2.0;
        }

        score
    }

    pub fn compact(&self, messages: &[Message]) -> CompactionResult {
        if messages.len() <= self.config.min_messages_to_keep {
            return CompactionResult {
                messages: messages.to_vec(),
                estimated_tokens: estimate_messages_tokens(messages),
                compacted_count: 0,
            };
        }

        // Calculate importance for each message
        let mut importances: Vec<MessageImportance> = messages
            .iter()
            .enumerate()
            .map(|(i, msg)| MessageImportance {
                index: i,
                score: Self::calculate_importance(msg, i, messages.len()),
            })
            .collect();

        // Sort by importance (highest first)
        importances.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap());

        // Select messages up to target token count
        let mut selected_indices: Vec<usize> = Vec::new();
        let mut current_tokens: usize = 0;

        // Always keep minimum messages
        for imp in importances.iter().take(self.config.min_messages_to_keep) {
            selected_indices.push(imp.index);
            current_tokens += estimate_message_tokens(&messages[imp.index]);
        }

        // Add more if under target
        for imp in importances.iter().skip(self.config.min_messages_to_keep) {
            if current_tokens >= self.config.target_context_tokens {
                break;
            }
            selected_indices.push(imp.index);
            current_tokens += estimate_message_tokens(&messages[imp.index]);
        }

        // Sort by original index to maintain order
        selected_indices.sort();

        let result_messages: Vec<Message> = selected_indices
            .iter()
            .map(|&i| messages[i].clone())
            .collect();

        CompactionResult {
            estimated_tokens: estimate_messages_tokens(&result_messages),
            compacted_count: messages.len() - result_messages.len(),
            messages: result_messages,
        }
    }
}

impl Default for ImportanceCompaction {
    fn default() -> Self {
        Self::new()
    }
}

/// Sliding window compaction
pub struct SlidingWindowCompaction {
    config: CompactionConfig,
    window_size: usize,
}

impl SlidingWindowCompaction {
    pub fn new(window_size: usize) -> Self {
        Self {
            config: CompactionConfig::default(),
            window_size,
        }
    }

    pub fn should_compact(&self, messages: &[Message], _system_prompt: Option<&str>) -> bool {
        messages.len() > self.window_size
    }

    pub fn compact(&self, messages: &[Message]) -> CompactionResult {
        if messages.len() <= self.window_size {
            return CompactionResult {
                messages: messages.to_vec(),
                estimated_tokens: estimate_messages_tokens(messages),
                compacted_count: 0,
            };
        }

        // Check if first message is system
        let keep_first: usize = if !messages.is_empty() {
            use llm::MessageRole;
            if matches!(messages[0].role, MessageRole::System) {
                1
            } else {
                0
            }
        } else {
            0
        };

        let recent_start = messages.len().saturating_sub(self.window_size);
        let total_to_keep = keep_first + (messages.len() - recent_start);

        let mut result_messages: Vec<Message> = Vec::with_capacity(total_to_keep);

        if keep_first > 0 {
            result_messages.push(messages[0].clone());
        }

        for msg in messages.iter().skip(recent_start) {
            result_messages.push(msg.clone());
        }

        let compacted = messages.len() - total_to_keep;

        CompactionResult {
            estimated_tokens: estimate_messages_tokens(&result_messages),
            compacted_count: compacted,
            messages: result_messages,
        }
    }
}

/// Unified context compactor
pub struct ContextCompactor {
    config: CompactionConfig,
    stats: CompactionStats,
    strategy: CompactionStrategy,
}

impl ContextCompactor {
    pub fn new(strategy: CompactionStrategy) -> Self {
        Self {
            config: CompactionConfig::default(),
            stats: CompactionStats::default(),
            strategy,
        }
    }

    pub fn with_config(mut self, config: CompactionConfig) -> Self {
        self.config = config;
        self
    }

    pub fn should_compact(&self, messages: &[Message], system_prompt: Option<&str>) -> bool {
        let mut total = 0;
        if let Some(sp) = system_prompt {
            total += estimate_tokens(sp);
        }
        total += estimate_messages_tokens(messages);
        total > self.config.max_context_tokens
    }

    pub fn compact(&mut self, messages: &[Message]) -> CompactionResult {
        let before_tokens = estimate_messages_tokens(messages);

        let result = match self.strategy {
            CompactionStrategy::Simple => {
                let compactor = SimpleCompaction::with_config(self.config.clone());
                compactor.compact(messages)
            }
            CompactionStrategy::ImportanceBased => {
                let compactor = ImportanceCompaction::new();
                compactor.compact(messages)
            }
            CompactionStrategy::SlidingWindow => {
                let window_size = self.config.min_messages_to_keep * 2;
                let compactor = SlidingWindowCompaction::new(window_size);
                compactor.compact(messages)
            }
        };

        let after_tokens = result.estimated_tokens;
        let tokens_saved = before_tokens.saturating_sub(after_tokens);
        self.stats.record_compaction(result.compacted_count, tokens_saved);

        result
    }

    pub fn stats(&self) -> &CompactionStats {
        &self.stats
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_compaction_config_default() {
        let config = CompactionConfig::default();
        assert_eq!(config.max_context_tokens, 24000);
        assert_eq!(config.target_context_tokens, 12000);
        assert!(config.preserve_system_messages);
    }

    #[test]
    fn test_compaction_config_small() {
        let config = CompactionConfig::small_context();
        assert_eq!(config.max_context_tokens, 6000);
    }

    #[test]
    fn test_compaction_stats() {
        let mut stats = CompactionStats::default();
        stats.record_compaction(5, 1000);
        
        assert_eq!(stats.total_compactions, 1);
        assert_eq!(stats.total_messages_removed, 5);
        assert_eq!(stats.total_tokens_saved, 1000);
        assert!(stats.last_compaction_time.is_some());
    }

    #[test]
    fn test_simple_compaction() {
        let compactor = SimpleCompaction::new();
        
        // Create many messages
        let messages: Vec<Message> = (0..15)
            .map(|i| if i % 2 == 0 {
                Message::user(&format!("User message {}", i))
            } else {
                Message::assistant(&format!("Assistant message {}", i))
            })
            .collect();
        
        let result = compactor.compact(&messages);
        
        assert!(result.messages.len() < messages.len());
        assert!(result.compacted_count > 0);
        assert!(result.estimated_tokens > 0);
    }

    #[test]
    fn test_simple_compaction_preserves_first() {
        let compactor = SimpleCompaction::new();
        
        let messages: Vec<Message> = (0..12)
            .map(|i| {
                if i == 0 {
                    Message::system("System prompt")
                } else if i % 2 == 0 {
                    Message::user(&format!("User {}", i))
                } else {
                    Message::assistant(&format!("Assistant {}", i))
                }
            })
            .collect();
        
        let result = compactor.compact(&messages);
        
        // First message should be preserved
        assert_eq!(result.messages[0].content, "System prompt");
    }

    #[test]
    fn test_importance_compaction() {
        let compactor = ImportanceCompaction::new();
        
        let messages: Vec<Message> = (0..10)
            .map(|i| Message::user(&format!("Message {}", i)))
            .collect();
        
        let result = compactor.compact(&messages);
        
        assert!(result.messages.len() >= 4); // min_messages_to_keep
        assert_eq!(result.compacted_count, messages.len() - result.messages.len());
    }

    #[test]
    fn test_sliding_window_compaction() {
        let compactor = SlidingWindowCompaction::new(5);
        
        let messages: Vec<Message> = (0..10)
            .map(|i| Message::user(&format!("{}", i)))
            .collect();
        
        let result = compactor.compact(&messages);
        
        assert!(result.messages.len() <= 6); // window_size + 1 (if first is system)
        assert_eq!(result.compacted_count, 10 - result.messages.len());
    }

    #[test]
    fn test_sliding_window_preserves_system() {
        let compactor = SlidingWindowCompaction::new(3);
        
        let mut messages = vec![Message::system("System")];
        messages.extend((0..5).map(|i| Message::user(&format!("{}", i))));
        
        let result = compactor.compact(&messages);
        
        // System message should be preserved
        assert!(result.messages.iter().any(|m| m.content == "System"));
    }

    #[test]
    fn test_context_compactor() {
        let mut compactor = ContextCompactor::new(CompactionStrategy::Simple);
        
        let messages: Vec<Message> = (0..12)
            .map(|_| Message::user("Content here for testing"))
            .collect();
        
        let result = compactor.compact(&messages);
        
        assert!(!result.messages.is_empty());
        assert_eq!(compactor.stats().total_compactions, 1);
    }
}
