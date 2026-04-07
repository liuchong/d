//! Pattern recognition for learning user behavior
//!
//! Analyzes user interactions to:
//! - Recognize recurring patterns
//! - Suggest next actions
//! - Predict user needs
//! - Automate repetitive workflows

use chrono::{Datelike, Timelike};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::time::Instant;

/// Recognized pattern
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Pattern {
    pub id: String,
    pub name: String,
    pub description: String,
    pub pattern_type: PatternType,
    pub frequency: u32,
    pub confidence: f64,
    pub last_seen: String,
    pub created_at: String,
}

/// Pattern types
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PatternType {
    /// Sequence of commands
    CommandSequence(Vec<String>),
    /// Time-based pattern
    TimeBased { hour: u8, day_of_week: u8 },
    /// Tool usage pattern
    ToolUsage(Vec<String>),
    /// Topic pattern
    TopicPattern(Vec<String>),
    /// File operation pattern
    FileOperation { extensions: Vec<String>, directories: Vec<String> },
}

/// Pattern match result
#[derive(Debug, Clone)]
pub struct PatternMatch {
    pub pattern: Pattern,
    pub match_score: f64,
    pub suggested_action: String,
}

/// Pattern recognizer
pub struct PatternRecognizer {
    patterns: Vec<Pattern>,
    recent_commands: Vec<CommandEntry>,
    max_history: usize,
}

/// Command entry
#[derive(Debug, Clone)]
struct CommandEntry {
    command: String,
    #[allow(dead_code)]
    timestamp: Instant,
    context: HashMap<String, String>,
}

impl PatternRecognizer {
    /// Create a new pattern recognizer
    pub fn new() -> Self {
        Self {
            patterns: Vec::new(),
            recent_commands: Vec::new(),
            max_history: 100,
        }
    }

    /// Record a command
    pub fn record_command(&mut self, command: String, context: HashMap<String, String>) {
        let entry = CommandEntry {
            command,
            timestamp: Instant::now(),
            context,
        };

        self.recent_commands.push(entry);

        // Keep only recent history
        if self.recent_commands.len() > self.max_history {
            self.recent_commands.remove(0);
        }

        // Analyze for new patterns
        self.analyze_patterns();
    }

    /// Analyze recent commands for patterns
    fn analyze_patterns(&mut self) {
        // Look for command sequences
        self.detect_command_sequences();
        
        // Look for time patterns
        self.detect_time_patterns();
        
        // Look for tool patterns
        self.detect_tool_patterns();
    }

    /// Detect command sequence patterns
    fn detect_command_sequences(&mut self) {
        if self.recent_commands.len() < 3 {
            return;
        }

        // Look for repeating pairs of commands
        let recent: Vec<_> = self.recent_commands.iter()
            .rev()
            .take(10)
            .map(|e| e.command.clone())
            .collect();

        for window in recent.windows(2) {
            if window[0] == window[1] {
                continue; // Skip identical commands
            }

            let sequence = vec![window[0].clone(), window[1].clone()];
            
            // Check if pattern already exists
            let exists = self.patterns.iter().any(|p| {
                matches!(&p.pattern_type, PatternType::CommandSequence(seq) if seq == &sequence)
            });

            if !exists {
                let now = chrono::Utc::now().to_rfc3339();
                let pattern = Pattern {
                    id: format!("seq_{}", self.patterns.len()),
                    name: format!("Sequence: {} → {}", sequence[0], sequence[1]),
                    description: format!("User often runs '{}' after '{}'", sequence[1], sequence[0]),
                    pattern_type: PatternType::CommandSequence(sequence),
                    frequency: 1,
                    confidence: 0.5,
                    last_seen: now.clone(),
                    created_at: now,
                };
                self.patterns.push(pattern);
            }
        }
    }

    /// Detect time-based patterns
    fn detect_time_patterns(&mut self) {
        // Simple implementation - would need more data for real patterns
        let now = chrono::Local::now();
        let hour = now.hour() as u8;
        let day = now.weekday().num_days_from_monday() as u8;

        // Check if this time pattern exists
        let exists = self.patterns.iter().any(|p| {
            matches!(&p.pattern_type, PatternType::TimeBased { hour: h, day_of_week: d } if *h == hour && *d == day)
        });

        if !exists && self.recent_commands.len() >= 5 {
            let now_str = chrono::Utc::now().to_rfc3339();
            let pattern = Pattern {
                id: format!("time_{}_{}", day, hour),
                name: "Time-based pattern".to_string(),
                description: format!("Active on day {} at hour {}", day, hour),
                pattern_type: PatternType::TimeBased { hour, day_of_week: day },
                frequency: 1,
                confidence: 0.3,
                last_seen: now_str.clone(),
                created_at: now_str,
            };
            self.patterns.push(pattern);
        }
    }

    /// Detect tool usage patterns
    fn detect_tool_patterns(&mut self) {
        // Extract tool names from recent commands
        let tools: Vec<_> = self.recent_commands.iter()
            .rev()
            .take(5)
            .filter_map(|e| e.context.get("tool").cloned())
            .collect();

        if tools.len() >= 2 {
            let exists = self.patterns.iter().any(|p| {
                matches!(&p.pattern_type, PatternType::ToolUsage(t) if t == &tools)
            });

            if !exists {
                let now = chrono::Utc::now().to_rfc3339();
                let pattern = Pattern {
                    id: format!("tools_{}", self.patterns.len()),
                    name: format!("Tool pattern: {}", tools.join(", ")),
                    description: format!("Frequently uses tools: {}", tools.join(", ")),
                    pattern_type: PatternType::ToolUsage(tools),
                    frequency: 1,
                    confidence: 0.4,
                    last_seen: now.clone(),
                    created_at: now,
                };
                self.patterns.push(pattern);
            }
        }
    }

    /// Find matching patterns for current context
    pub fn find_matches(&self, context: &HashMap<String, String>) -> Vec<PatternMatch> {
        let mut matches = Vec::new();

        for pattern in &self.patterns {
            if let Some(score) = self.calculate_match_score(pattern, context) {
                if score > 0.5 {
                    let action = self.generate_suggestion(pattern);
                    matches.push(PatternMatch {
                        pattern: pattern.clone(),
                        match_score: score,
                        suggested_action: action,
                    });
                }
            }
        }

        // Sort by match score
        matches.sort_by(|a, b| b.match_score.partial_cmp(&a.match_score).unwrap());
        matches
    }

    /// Calculate match score for a pattern
    fn calculate_match_score(&self, pattern: &Pattern, context: &HashMap<String, String>) -> Option<f64> {
        match &pattern.pattern_type {
            PatternType::CommandSequence(seq) => {
                // Check if recent commands match the start of this sequence
                if seq.is_empty() || self.recent_commands.is_empty() {
                    return None;
                }

                let recent_cmd = self.recent_commands.last()?.command.clone();
                if recent_cmd == seq[0] {
                    Some(pattern.confidence * 0.8)
                } else {
                    None
                }
            }
            PatternType::TimeBased { .. } => {
                // Time patterns have lower confidence
                Some(pattern.confidence * 0.5)
            }
            PatternType::ToolUsage(tools) => {
                // Check if current tools match
                if let Some(current_tool) = context.get("tool") {
                    if tools.contains(current_tool) {
                        Some(pattern.confidence * 0.9)
                    } else {
                        None
                    }
                } else {
                    None
                }
            }
            PatternType::TopicPattern(topics) => {
                if let Some(current_topic) = context.get("topic") {
                    if topics.contains(current_topic) {
                        Some(pattern.confidence * 0.85)
                    } else {
                        None
                    }
                } else {
                    None
                }
            }
            PatternType::FileOperation { extensions, directories } => {
                let mut score = 0.0;
                
                if let Some(ext) = context.get("file_extension") {
                    if extensions.contains(ext) {
                        score += 0.5;
                    }
                }
                
                if let Some(dir) = context.get("directory") {
                    if directories.iter().any(|d| dir.contains(d)) {
                        score += 0.5;
                    }
                }
                
                if score > 0.0 {
                    Some(score * pattern.confidence)
                } else {
                    None
                }
            }
        }
    }

    /// Generate a suggestion based on a pattern
    fn generate_suggestion(&self, pattern: &Pattern) -> String {
        match &pattern.pattern_type {
            PatternType::CommandSequence(seq) if seq.len() >= 2 => {
                format!("Next: try '{}'", seq[1])
            }
            PatternType::ToolUsage(tools) => {
                format!("Frequently used: {}", tools.join(", "))
            }
            PatternType::TopicPattern(topics) => {
                format!("Related topics: {}", topics.join(", "))
            }
            _ => pattern.description.clone(),
        }
    }

    /// Get all patterns
    pub fn patterns(&self) -> &[Pattern] {
        &self.patterns
    }

    /// Get pattern by ID
    pub fn get_pattern(&self, id: &str) -> Option<&Pattern> {
        self.patterns.iter().find(|p| p.id == id)
    }

    /// Remove a pattern
    pub fn remove_pattern(&mut self, id: &str) -> bool {
        if let Some(pos) = self.patterns.iter().position(|p| p.id == id) {
            self.patterns.remove(pos);
            true
        } else {
            false
        }
    }

    /// Clear all patterns
    pub fn clear_patterns(&mut self) {
        self.patterns.clear();
    }

    /// Get suggestion for next action
    pub fn suggest_next_action(&self) -> Option<String> {
        if self.patterns.is_empty() {
            return None;
        }

        // Find highest confidence pattern
        let best = self.patterns.iter()
            .max_by(|a, b| a.confidence.partial_cmp(&b.confidence).unwrap())?;

        if best.confidence > 0.6 {
            Some(self.generate_suggestion(best))
        } else {
            None
        }
    }

    /// Export patterns to JSON
    pub fn export_json(&self) -> anyhow::Result<String> {
        Ok(serde_json::to_string_pretty(&self.patterns)?)
    }

    /// Import patterns from JSON
    pub fn import_json(&mut self, json: &str) -> anyhow::Result<()> {
        let patterns: Vec<Pattern> = serde_json::from_str(json)?;
        self.patterns = patterns;
        Ok(())
    }
}

impl Default for PatternRecognizer {
    fn default() -> Self {
        Self::new()
    }
}

/// Suggestion engine
pub struct SuggestionEngine {
    recognizer: PatternRecognizer,
}

impl SuggestionEngine {
    /// Create a new suggestion engine
    pub fn new() -> Self {
        Self {
            recognizer: PatternRecognizer::new(),
        }
    }

    /// Record user action
    pub fn record_action(&mut self, command: String, context: HashMap<String, String>) {
        self.recognizer.record_command(command, context);
    }

    /// Get suggestions for current context
    pub fn get_suggestions(&self, context: &HashMap<String, String>) -> Vec<String> {
        let matches = self.recognizer.find_matches(context);
        matches.into_iter()
            .take(3)
            .map(|m| m.suggested_action)
            .collect()
    }

    /// Get next action suggestion
    pub fn suggest_next(&self) -> Option<String> {
        self.recognizer.suggest_next_action()
    }

    /// Get pattern statistics
    pub fn stats(&self) -> PatternStats {
        PatternStats {
            total_patterns: self.recognizer.patterns().len(),
            command_patterns: self.recognizer.patterns().iter()
                .filter(|p| matches!(p.pattern_type, PatternType::CommandSequence(_)))
                .count(),
            time_patterns: self.recognizer.patterns().iter()
                .filter(|p| matches!(p.pattern_type, PatternType::TimeBased { .. }))
                .count(),
            tool_patterns: self.recognizer.patterns().iter()
                .filter(|p| matches!(p.pattern_type, PatternType::ToolUsage(_)))
                .count(),
        }
    }
}

impl Default for SuggestionEngine {
    fn default() -> Self {
        Self::new()
    }
}

/// Pattern statistics
#[derive(Debug, Clone)]
pub struct PatternStats {
    pub total_patterns: usize,
    pub command_patterns: usize,
    pub time_patterns: usize,
    pub tool_patterns: usize,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pattern_recognizer() {
        let mut recognizer = PatternRecognizer::new();
        
        // Record some commands
        for _ in 0..5 {
            let mut ctx = HashMap::new();
            ctx.insert("tool".to_string(), "read_file".to_string());
            recognizer.record_command("read src/main.rs".to_string(), ctx);
        }

        // Should have detected patterns
        assert!(!recognizer.patterns().is_empty());
    }

    #[test]
    fn test_suggestion_engine() {
        let mut engine = SuggestionEngine::new();
        
        let mut ctx = HashMap::new();
        ctx.insert("tool".to_string(), "read_file".to_string());
        
        // Add multiple actions to trigger pattern detection
        for i in 0..5 {
            engine.record_action(format!("read src/file{}.rs", i), ctx.clone());
        }
        
        let suggestions = engine.get_suggestions(&ctx);
        // May or may not have suggestions depending on pattern detection
        // Just verify it doesn't panic
        let _ = suggestions.len();
    }

    #[test]
    fn test_pattern_export_import() {
        let mut recognizer = PatternRecognizer::new();
        
        // Add a pattern manually
        let pattern = Pattern {
            id: "test".to_string(),
            name: "Test".to_string(),
            description: "Test pattern".to_string(),
            pattern_type: PatternType::ToolUsage(vec!["read_file".to_string()]),
            frequency: 5,
            confidence: 0.8,
            last_seen: chrono::Utc::now().to_rfc3339(),
            created_at: chrono::Utc::now().to_rfc3339(),
        };
        recognizer.patterns.push(pattern);
        
        // Export
        let json = recognizer.export_json().unwrap();
        
        // Import
        let mut new_recognizer = PatternRecognizer::new();
        new_recognizer.import_json(&json).unwrap();
        
        assert_eq!(new_recognizer.patterns().len(), 1);
    }

    #[test]
    fn test_pattern_stats() {
        let engine = SuggestionEngine::new();
        let stats = engine.stats();
        
        assert_eq!(stats.total_patterns, 0);
        assert_eq!(stats.command_patterns, 0);
    }
}
