//! Personality engine for learning and adapting to user preferences
//!
//! Analyzes interaction patterns to customize:
//! - Response style (concise vs detailed)
//! - Code preferences (functional vs imperative)
//! - Tool usage patterns
//! - Communication tone

use chrono::{Datelike, Timelike};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

mod analyzer;
mod storage;

pub use analyzer::PersonalityAnalyzer;
pub use storage::PersonalityStorage;

/// User personality profile
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct PersonalityProfile {
    pub user_id: String,
    pub created_at: String,
    pub updated_at: String,
    
    // Communication preferences
    pub communication_style: CommunicationStyle,
    pub preferred_detail_level: DetailLevel,
    pub code_style_preference: CodeStylePreference,
    
    // Interaction patterns
    pub interaction_patterns: InteractionPatterns,
    
    // Tool usage
    pub tool_preferences: ToolPreferences,
    
    // Topic interests
    pub topic_interests: Vec<TopicInterest>,
    
    // Feedback history
    pub feedback_history: Vec<FeedbackRecord>,
    
    // Adaptation settings
    pub adaptation: AdaptationSettings,
}

/// Communication style preference
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CommunicationStyle {
    /// Formal, structured responses
    Formal,
    /// Casual, conversational
    Casual,
    /// Technical, precise
    Technical,
    /// Teaching-oriented with explanations
    Educational,
    /// Direct, minimal fluff
    Concise,
}

impl Default for CommunicationStyle {
    fn default() -> Self {
        CommunicationStyle::Technical
    }
}

/// Detail level preference
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum DetailLevel {
    Minimal,
    Low,
    Medium,
    High,
    Exhaustive,
}

impl Default for DetailLevel {
    fn default() -> Self {
        DetailLevel::Medium
    }
}

/// Code style preference
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CodeStylePreference {
    /// Prefer functional programming patterns
    Functional,
    /// Prefer object-oriented patterns
    ObjectOriented,
    /// Prefer procedural/imperative
    Procedural,
    /// Mixed, context-dependent
    Mixed,
}

impl Default for CodeStylePreference {
    fn default() -> Self {
        CodeStylePreference::Mixed
    }
}

/// Interaction patterns
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct InteractionPatterns {
    /// Average message length
    pub avg_message_length: f64,
    /// Frequency of asking for clarifications
    pub clarification_frequency: f64,
    /// How often user prefers examples
    pub example_preference: f64,
    /// Typical session duration in minutes
    pub avg_session_duration: f64,
    /// Time of day patterns (hour -> frequency)
    pub time_of_day: HashMap<u8, u32>,
    /// Day of week patterns (0=Monday -> frequency)
    pub day_of_week: HashMap<u8, u32>,
}

/// Tool preferences
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ToolPreferences {
    /// Most frequently used tools
    pub tool_frequency: HashMap<String, u32>,
    /// Success rate per tool
    pub tool_success_rate: HashMap<String, f64>,
    /// Preferred tools for specific tasks
    pub task_tool_mapping: HashMap<String, Vec<String>>,
    /// Tools user has explicitly liked/disliked
    pub tool_ratings: HashMap<String, i32>,
}

/// Topic interest
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TopicInterest {
    pub topic: String,
    pub interest_score: f64,  // 0.0 to 1.0
    pub frequency: u32,
    pub last_mentioned: String,
}

/// Feedback record
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FeedbackRecord {
    pub timestamp: String,
    pub context: FeedbackContext,
    pub rating: i32,  // -2 to +2 (strong dislike to strong like)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub comment: Option<String>,
}

/// Feedback context
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FeedbackContext {
    pub message_id: String,
    pub conversation_topic: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_used: Option<String>,
}

/// Adaptation settings
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AdaptationSettings {
    /// How quickly to adapt (0.0 = slow, 1.0 = fast)
    pub adaptation_rate: f64,
    /// Whether to actively suggest based on patterns
    pub enable_proactive_suggestions: bool,
    /// Minimum confidence for adaptations
    pub confidence_threshold: f64,
}

impl Default for AdaptationSettings {
    fn default() -> Self {
        Self {
            adaptation_rate: 0.3,
            enable_proactive_suggestions: true,
            confidence_threshold: 0.6,
        }
    }
}

impl PersonalityProfile {
    /// Create a new profile for a user
    pub fn new(user_id: impl Into<String>) -> Self {
        let now = chrono::Utc::now().to_rfc3339();
        Self {
            user_id: user_id.into(),
            created_at: now.clone(),
            updated_at: now,
            communication_style: CommunicationStyle::default(),
            preferred_detail_level: DetailLevel::default(),
            code_style_preference: CodeStylePreference::default(),
            interaction_patterns: InteractionPatterns::default(),
            tool_preferences: ToolPreferences::default(),
            topic_interests: Vec::new(),
            feedback_history: Vec::new(),
            adaptation: AdaptationSettings::default(),
        }
    }

    /// Update from interaction
    pub fn record_interaction(&mut self, interaction: InteractionRecord) {
        self.updated_at = chrono::Utc::now().to_rfc3339();

        // Update message length average
        let patterns = &mut self.interaction_patterns;
        let count = self.tool_preferences.tool_frequency.values().sum::<u32>() as f64 + 1.0;
        patterns.avg_message_length = 
            (patterns.avg_message_length * (count - 1.0) + interaction.message_length as f64) / count;

        // Record time of interaction
        let hour = interaction.timestamp.hour() as u8;
        let day = interaction.timestamp.weekday().num_days_from_monday() as u8;
        
        *patterns.time_of_day.entry(hour).or_insert(0) += 1;
        *patterns.day_of_week.entry(day).or_insert(0) += 1;

        // Record tool usage
        if let Some(tool) = interaction.tool_used {
            *self.tool_preferences.tool_frequency.entry(tool).or_insert(0) += 1;
        }

        // Record topics
        for topic in interaction.topics {
            self.update_topic_interest(&topic);
        }
    }

    /// Update topic interest
    fn update_topic_interest(&mut self, topic: &str) {
        let now = chrono::Utc::now().to_rfc3339();
        
        if let Some(interest) = self.topic_interests.iter_mut().find(|t| t.topic == topic) {
            interest.frequency += 1;
            interest.last_mentioned = now;
            // Decay old interest and boost current
            interest.interest_score = (interest.interest_score * 0.9 + 0.1).min(1.0);
        } else {
            self.topic_interests.push(TopicInterest {
                topic: topic.to_string(),
                interest_score: 0.5,
                frequency: 1,
                last_mentioned: now,
            });
        }
    }

    /// Record feedback
    pub fn record_feedback(&mut self, feedback: FeedbackRecord) {
        self.updated_at = chrono::Utc::now().to_rfc3339();
        
        // Adjust style based on feedback
        match feedback.rating {
            -2 | -1 => {
                // Negative feedback - adjust away from current style
                self.adjust_style_negative(feedback.context.clone());
            }
            1 | 2 => {
                // Positive feedback - reinforce current style
                self.adjust_style_positive(feedback.context.clone());
            }
            _ => {}
        }

        self.feedback_history.push(feedback);
        
        // Keep only recent feedback
        if self.feedback_history.len() > 100 {
            self.feedback_history.remove(0);
        }
    }

    fn adjust_style_negative(&mut self, context: FeedbackContext) {
        // If tool was used and got negative feedback, decrease its rating
        if let Some(tool) = context.tool_used {
            let rating = self.tool_preferences.tool_ratings.entry(tool).or_insert(0);
            *rating = (*rating - 1).max(-5);
        }
    }

    fn adjust_style_positive(&mut self, context: FeedbackContext) {
        // If tool was used and got positive feedback, increase its rating
        if let Some(tool) = context.tool_used {
            let rating = self.tool_preferences.tool_ratings.entry(tool).or_insert(0);
            *rating = (*rating + 1).min(5);
        }
    }

    /// Get system prompt customization based on personality
    pub fn get_system_prompt_addon(&self) -> String {
        let mut addons = Vec::new();

        // Communication style
        let style_prompt = match self.communication_style {
            CommunicationStyle::Formal => "Use formal, professional language.",
            CommunicationStyle::Casual => "Keep responses conversational and approachable.",
            CommunicationStyle::Technical => "Be precise and technical in explanations.",
            CommunicationStyle::Educational => "Explain concepts thoroughly, as if teaching.",
            CommunicationStyle::Concise => "Be brief and direct. Avoid unnecessary elaboration.",
        };
        addons.push(style_prompt.to_string());

        // Detail level
        let detail_prompt = match self.preferred_detail_level {
            DetailLevel::Minimal => "Provide minimal necessary information.",
            DetailLevel::Low => "Keep responses brief but complete.",
            DetailLevel::Medium => "Balance detail with brevity.",
            DetailLevel::High => "Provide detailed, comprehensive responses.",
            DetailLevel::Exhaustive => "Be thorough and cover edge cases.",
        };
        addons.push(detail_prompt.to_string());

        // Code style
        let code_prompt = match self.code_style_preference {
            CodeStylePreference::Functional => "Prefer functional programming patterns.",
            CodeStylePreference::ObjectOriented => "Use object-oriented design principles.",
            CodeStylePreference::Procedural => "Write clear, procedural code.",
            CodeStylePreference::Mixed => "Choose the most appropriate style for each situation.",
        };
        addons.push(code_prompt.to_string());

        addons.join(" ")
    }

    /// Get favorite tools
    pub fn favorite_tools(&self, limit: usize) -> Vec<String> {
        let mut tools: Vec<_> = self.tool_preferences.tool_frequency
            .iter()
            .map(|(k, v)| (k.clone(), *v))
            .collect();
        tools.sort_by(|a, b| b.1.cmp(&a.1));
        tools.into_iter().take(limit).map(|(k, _)| k).collect()
    }

    /// Get top interests
    pub fn top_interests(&self, limit: usize) -> Vec<&TopicInterest> {
        let mut interests: Vec<_> = self.topic_interests.iter().collect();
        interests.sort_by(|a, b| b.interest_score.partial_cmp(&a.interest_score).unwrap());
        interests.into_iter().take(limit).collect()
    }
}

/// Interaction record for analysis
#[derive(Debug, Clone)]
pub struct InteractionRecord {
    pub timestamp: chrono::DateTime<chrono::Utc>,
    pub message_length: usize,
    pub tool_used: Option<String>,
    pub tool_success: bool,
    pub topics: Vec<String>,
    pub asked_for_clarification: bool,
    pub requested_examples: bool,
}

/// Personality engine
pub struct PersonalityEngine {
    storage: Box<dyn PersonalityStorage>,
    analyzer: PersonalityAnalyzer,
    cache: HashMap<String, PersonalityProfile>,
}

impl PersonalityEngine {
    /// Create a new personality engine
    pub fn new(storage: Box<dyn PersonalityStorage>) -> Self {
        Self {
            storage,
            analyzer: PersonalityAnalyzer::new(),
            cache: HashMap::new(),
        }
    }

    /// Load or create profile for a user
    pub async fn get_profile(&mut self, user_id: &str) -> anyhow::Result<PersonalityProfile> {
        if let Some(profile) = self.cache.get(user_id) {
            return Ok(profile.clone());
        }

        match self.storage.load(user_id).await? {
            Some(profile) => {
                self.cache.insert(user_id.to_string(), profile.clone());
                Ok(profile)
            }
            None => {
                let profile = PersonalityProfile::new(user_id);
                self.storage.save(&profile).await?;
                self.cache.insert(user_id.to_string(), profile.clone());
                Ok(profile)
            }
        }
    }

    /// Update profile with interaction
    pub async fn record_interaction(
        &mut self,
        user_id: &str,
        interaction: InteractionRecord,
    ) -> anyhow::Result<()> {
        let mut profile = self.get_profile(user_id).await?;
        profile.record_interaction(interaction);
        
        // Analyze and potentially adapt
        if self.analyzer.should_adapt(&profile) {
            self.analyzer.analyze_and_adapt(&mut profile);
        }

        self.storage.save(&profile).await?;
        self.cache.insert(user_id.to_string(), profile);
        Ok(())
    }

    /// Record feedback
    pub async fn record_feedback(
        &mut self,
        user_id: &str,
        feedback: FeedbackRecord,
    ) -> anyhow::Result<()> {
        let mut profile = self.get_profile(user_id).await?;
        profile.record_feedback(feedback);
        self.storage.save(&profile).await?;
        self.cache.insert(user_id.to_string(), profile);
        Ok(())
    }

    /// Get personalized system prompt
    pub async fn get_personalized_prompt(&mut self, user_id: &str) -> anyhow::Result<String> {
        let profile = self.get_profile(user_id).await?;
        Ok(profile.get_system_prompt_addon())
    }

    /// Flush cache to storage
    pub async fn flush(&mut self) -> anyhow::Result<()> {
        for (_user_id, profile) in &self.cache {
            self.storage.save(profile).await?;
        }
        Ok(())
    }
}

/// File-based storage implementation
pub struct FileStorage {
    base_path: PathBuf,
}

impl FileStorage {
    pub fn new(base_path: impl AsRef<Path>) -> Self {
        Self {
            base_path: base_path.as_ref().to_path_buf(),
        }
    }
}

#[async_trait::async_trait]
impl PersonalityStorage for FileStorage {
    async fn load(&self, user_id: &str) -> anyhow::Result<Option<PersonalityProfile>> {
        let path = self.base_path.join(format!("{}.json", user_id));
        if !path.exists() {
            return Ok(None);
        }
        
        let content = tokio::fs::read_to_string(&path).await?;
        let profile = serde_json::from_str(&content)?;
        Ok(Some(profile))
    }

    async fn save(&self, profile: &PersonalityProfile) -> anyhow::Result<()> {
        tokio::fs::create_dir_all(&self.base_path).await?;
        let path = self.base_path.join(format!("{}.json", profile.user_id));
        let content = serde_json::to_string_pretty(profile)?;
        tokio::fs::write(&path, content).await?;
        Ok(())
    }

    async fn list_users(&self) -> anyhow::Result<Vec<String>> {
        let mut users = Vec::new();
        if self.base_path.exists() {
            let mut entries = tokio::fs::read_dir(&self.base_path).await?;
            while let Some(entry) = entries.next_entry().await? {
                if let Some(name) = entry.file_name().to_str() {
                    if name.ends_with(".json") {
                        users.push(name.trim_end_matches(".json").to_string());
                    }
                }
            }
        }
        Ok(users)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_personality_profile_creation() {
        let profile = PersonalityProfile::new("user123");
        assert_eq!(profile.user_id, "user123");
        assert_eq!(profile.communication_style, CommunicationStyle::Technical);
        assert_eq!(profile.preferred_detail_level, DetailLevel::Medium);
    }

    #[test]
    fn test_record_interaction() {
        let mut profile = PersonalityProfile::new("user123");
        
        let interaction = InteractionRecord {
            timestamp: chrono::Utc::now(),
            message_length: 100,
            tool_used: Some("read_file".to_string()),
            tool_success: true,
            topics: vec!["rust".to_string(), "async".to_string()],
            asked_for_clarification: false,
            requested_examples: true,
        };
        
        profile.record_interaction(interaction);
        
        assert!(profile.interaction_patterns.avg_message_length > 0.0);
        assert_eq!(profile.topic_interests.len(), 2);
    }

    #[test]
    fn test_record_feedback() {
        let mut profile = PersonalityProfile::new("user123");
        
        let feedback = FeedbackRecord {
            timestamp: chrono::Utc::now().to_rfc3339(),
            context: FeedbackContext {
                message_id: "msg1".to_string(),
                conversation_topic: "testing".to_string(),
                tool_used: Some("shell".to_string()),
            },
            rating: 2,
            comment: Some("Great!".to_string()),
        };
        
        profile.record_feedback(feedback);
        
        assert_eq!(profile.feedback_history.len(), 1);
        assert_eq!(profile.tool_preferences.tool_ratings.get("shell"), Some(&1));
    }

    #[test]
    fn test_system_prompt_addon() {
        let mut profile = PersonalityProfile::new("user123");
        profile.communication_style = CommunicationStyle::Concise;
        profile.preferred_detail_level = DetailLevel::Low;
        profile.code_style_preference = CodeStylePreference::Functional;
        
        let addon = profile.get_system_prompt_addon();
        
        assert!(addon.contains("brief"));
        assert!(addon.contains("functional"));
    }

    #[test]
    fn test_favorite_tools() {
        let mut profile = PersonalityProfile::new("user123");
        profile.tool_preferences.tool_frequency.insert("read_file".to_string(), 10);
        profile.tool_preferences.tool_frequency.insert("write_file".to_string(), 5);
        profile.tool_preferences.tool_frequency.insert("shell".to_string(), 20);
        
        let favorites = profile.favorite_tools(2);
        
        assert_eq!(favorites.len(), 2);
        assert_eq!(favorites[0], "shell");
    }

    #[tokio::test]
    async fn test_file_storage() {
        let temp_dir = std::env::temp_dir().join("test_personality");
        let _ = tokio::fs::remove_dir_all(&temp_dir).await;
        
        let storage = FileStorage::new(&temp_dir);
        
        // Save
        let profile = PersonalityProfile::new("test_user");
        storage.save(&profile).await.unwrap();
        
        // Load
        let loaded = storage.load("test_user").await.unwrap();
        assert!(loaded.is_some());
        assert_eq!(loaded.unwrap().user_id, "test_user");
        
        // List
        let users = storage.list_users().await.unwrap();
        assert_eq!(users.len(), 1);
        
        // Cleanup
        let _ = tokio::fs::remove_dir_all(&temp_dir).await;
    }
}
