//! Personality analyzer for adapting to user behavior

use super::{CommunicationStyle, DetailLevel, PersonalityProfile};
use chrono::Timelike;
use tracing::debug;

/// Personality analyzer
pub struct PersonalityAnalyzer {
    min_interactions_for_adaptation: u32,
}

impl PersonalityAnalyzer {
    /// Create a new analyzer
    pub fn new() -> Self {
        Self {
            min_interactions_for_adaptation: 5,
        }
    }

    /// Check if enough data to adapt
    pub fn should_adapt(&self, profile: &PersonalityProfile) -> bool {
        let total_interactions: u32 = profile.tool_preferences.tool_frequency.values().sum();
        total_interactions >= self.min_interactions_for_adaptation
    }

    /// Analyze profile and suggest adaptations
    pub fn analyze_and_adapt(&self, profile: &mut PersonalityProfile) {
        debug!("Analyzing personality profile for user: {}", profile.user_id);

        // Analyze communication style from message lengths
        self.analyze_message_patterns(profile);

        // Analyze tool usage patterns
        self.analyze_tool_patterns(profile);

        // Analyze feedback patterns
        self.analyze_feedback_patterns(profile);

        // Analyze temporal patterns
        self.analyze_temporal_patterns(profile);
    }

    /// Analyze message length patterns
    fn analyze_message_patterns(&self, profile: &mut PersonalityProfile) {
        let avg_length = profile.interaction_patterns.avg_message_length;

        // Infer detail level preference from message length
        if avg_length < 50.0 {
            // User typically sends short messages - they prefer conciseness
            profile.preferred_detail_level = DetailLevel::Low;
        } else if avg_length > 200.0 {
            // User sends long, detailed messages
            profile.preferred_detail_level = DetailLevel::High;
        }

        // Check clarification frequency
        let clarification_rate = profile.interaction_patterns.clarification_frequency;
        if clarification_rate > 0.3 {
            // User asks for clarification often - prefer more detail
            profile.preferred_detail_level = DetailLevel::High;
        }
    }

    /// Analyze tool usage patterns
    fn analyze_tool_patterns(&self, profile: &mut PersonalityProfile) {
        let patterns = &profile.tool_preferences;
        
        // Find tools with high success rates
        for (tool, success_rate) in &patterns.tool_success_rate {
            if *success_rate < 0.3 {
                // Low success rate - user might be misusing this tool
                debug!("Tool {} has low success rate for user {}", tool, profile.user_id);
            }
        }

        // Identify task-tool mappings from history
        // This would require more sophisticated analysis in production
    }

    /// Analyze feedback patterns
    fn analyze_feedback_patterns(&self, profile: &mut PersonalityProfile) {
        let recent_feedback: Vec<_> = profile.feedback_history.iter().rev().take(10).collect();
        
        if recent_feedback.is_empty() {
            return;
        }

        let avg_rating: f64 = recent_feedback.iter()
            .map(|f| f.rating as f64)
            .sum::<f64>() / recent_feedback.len() as f64;

        if avg_rating < 0.0 {
            // Recent negative feedback - might need to adjust style
            debug!("Recent negative feedback detected for user {}", profile.user_id);
            
            // If formal and getting negative feedback, try more casual
            if profile.communication_style == CommunicationStyle::Formal {
                profile.communication_style = CommunicationStyle::Casual;
            }
        }

        // Analyze by tool
        let mut tool_ratings: std::collections::HashMap<String, Vec<i32>> = std::collections::HashMap::new();
        for feedback in &recent_feedback {
            if let Some(ref tool) = feedback.context.tool_used {
                tool_ratings.entry(tool.clone())
                    .or_default()
                    .push(feedback.rating);
            }
        }

        for (tool, ratings) in tool_ratings {
            let avg: f64 = ratings.iter().sum::<i32>() as f64 / ratings.len() as f64;
            profile.tool_preferences.tool_success_rate.insert(tool, avg / 2.0 + 0.5); // Normalize to 0-1
        }
    }

    /// Analyze temporal patterns
    fn analyze_temporal_patterns(&self, profile: &mut PersonalityProfile) {
        let patterns = &profile.interaction_patterns;

        // Find peak hours
        let mut peak_hours: Vec<_> = patterns.time_of_day.iter().collect();
        peak_hours.sort_by(|a, b| b.1.cmp(a.1));

        if let Some((hour, count)) = peak_hours.first() {
            debug!("User {} most active at hour {} ({} interactions)", 
                profile.user_id, hour, count);
        }

        // Weekend vs weekday patterns
        let weekend_count: u32 = patterns.day_of_week
            .iter()
            .filter(|(d, _)| **d >= 5) // Saturday = 5, Sunday = 6
            .map(|(_, c)| c)
            .sum();
        
        let weekday_count: u32 = patterns.day_of_week
            .iter()
            .filter(|(d, _)| **d < 5)
            .map(|(_, c)| c)
            .sum();

        if weekend_count > weekday_count * 2 {
            debug!("User {} primarily uses system on weekends", profile.user_id);
        }
    }

    /// Suggest proactive actions based on patterns
    pub fn suggest_actions(&self, profile: &PersonalityProfile) -> Vec<SuggestedAction> {
        let mut suggestions = Vec::new();

        // Suggest frequently used tools
        let favorites = profile.favorite_tools(3);
        if !favorites.is_empty() {
            suggestions.push(SuggestedAction::ToolShortcuts(favorites));
        }

        // Suggest topics of interest
        let top_interests: Vec<_> = profile.topic_interests.iter()
            .filter(|t| t.interest_score > 0.7)
            .take(3)
            .map(|t| t.topic.clone())
            .collect();
        
        if !top_interests.is_empty() {
            suggestions.push(SuggestedAction::TopicSuggestions(top_interests));
        }

        // Suggest based on time patterns
        let now = chrono::Local::now();
        let current_hour = now.hour() as u8;
        
        if let Some(&count) = profile.interaction_patterns.time_of_day.get(&current_hour) {
            if count > 10 {
                suggestions.push(SuggestedAction::ActiveHours);
            }
        }

        suggestions
    }
}

impl Default for PersonalityAnalyzer {
    fn default() -> Self {
        Self::new()
    }
}

/// Suggested action based on patterns
#[derive(Debug, Clone)]
pub enum SuggestedAction {
    /// Suggest frequently used tools
    ToolShortcuts(Vec<String>),
    /// Suggest topics of interest
    TopicSuggestions(Vec<String>),
    /// User typically active during current hours
    ActiveHours,
    /// Consider different communication style
    CommunicationAdjustment(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_should_adapt() {
        let analyzer = PersonalityAnalyzer::new();
        let profile = PersonalityProfile::new("test");
        
        // Not enough interactions
        assert!(!analyzer.should_adapt(&profile));
        
        // Add interactions
        let mut profile = PersonalityProfile::new("test");
        for i in 0..10 {
            profile.tool_preferences.tool_frequency.insert(format!("tool{}", i), 1);
        }
        assert!(analyzer.should_adapt(&profile));
    }

    #[test]
    fn test_analyze_message_patterns() {
        let analyzer = PersonalityAnalyzer::new();
        let mut profile = PersonalityProfile::new("test");
        
        profile.interaction_patterns.avg_message_length = 30.0;
        analyzer.analyze_message_patterns(&mut profile);
        
        assert_eq!(profile.preferred_detail_level, DetailLevel::Low);
    }

    #[test]
    fn test_suggest_actions() {
        let analyzer = PersonalityAnalyzer::new();
        let mut profile = PersonalityProfile::new("test");
        
        profile.tool_preferences.tool_frequency.insert("read_file".to_string(), 10);
        profile.topic_interests.push(crate::personality::TopicInterest {
            topic: "rust".to_string(),
            interest_score: 0.8,
            frequency: 5,
            last_mentioned: chrono::Utc::now().to_rfc3339(),
        });
        
        let suggestions = analyzer.suggest_actions(&profile);
        
        assert!(!suggestions.is_empty());
        assert!(matches!(suggestions[0], SuggestedAction::ToolShortcuts(_)));
    }
}
