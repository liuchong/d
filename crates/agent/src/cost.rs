//! Cost tracking for LLM API usage
//!
//! Tracks token usage and estimated costs for different models.

use llm::TokenUsage;
use std::collections::HashMap;

/// Model pricing per 1000 tokens
#[derive(Debug, Clone, Copy)]
pub struct ModelPricing {
    pub input_price: f64,
    pub output_price: f64,
}

impl ModelPricing {
    pub fn new(input_price: f64, output_price: f64) -> Self {
        Self {
            input_price,
            output_price,
        }
    }

    /// Calculate cost for given token usage
    pub fn calculate_cost(&self, usage: &TokenUsage) -> f64 {
        let input_cost = (usage.prompt_tokens as f64 / 1000.0) * self.input_price;
        let output_cost = (usage.completion_tokens as f64 / 1000.0) * self.output_price;
        input_cost + output_cost
    }
}

/// Known model pricing (USD per 1K tokens)
pub fn get_model_pricing(model: &str) -> ModelPricing {
    let model_lower = model.to_lowercase();
    
    match model_lower.as_str() {
        // Moonshot models
        m if m.contains("kimi-for-coding") => ModelPricing::new(0.0005, 0.002),
        m if m.contains("kimi-k2-5") => ModelPricing::new(0.0005, 0.002),
        m if m.contains("kimi-latest") => ModelPricing::new(0.0005, 0.002),
        
        // OpenAI models
        m if m.contains("gpt-4") && m.contains("turbo") => ModelPricing::new(0.01, 0.03),
        m if m.contains("gpt-4") => ModelPricing::new(0.03, 0.06),
        m if m.contains("gpt-3.5-turbo") => ModelPricing::new(0.0005, 0.0015),
        
        // Claude models
        m if m.contains("claude-3-opus") => ModelPricing::new(0.015, 0.075),
        m if m.contains("claude-3-sonnet") => ModelPricing::new(0.003, 0.015),
        m if m.contains("claude-3-haiku") => ModelPricing::new(0.00025, 0.00125),
        
        // Default pricing
        _ => ModelPricing::new(0.001, 0.002),
    }
}

/// Single cost entry
#[derive(Debug, Clone)]
pub struct CostEntry {
    pub timestamp: chrono::DateTime<chrono::Utc>,
    pub model: String,
    pub usage: TokenUsage,
    pub cost: f64,
    pub description: String,
}

impl CostEntry {
    pub fn new(model: impl Into<String>, usage: TokenUsage, description: impl Into<String>) -> Self {
        let model_str = model.into();
        let pricing = get_model_pricing(&model_str);
        let cost = pricing.calculate_cost(&usage);
        
        Self {
            timestamp: chrono::Utc::now(),
            model: model_str,
            usage,
            cost,
            description: description.into(),
        }
    }
}

/// Cost tracker for session
#[derive(Debug, Default)]
pub struct CostTracker {
    entries: Vec<CostEntry>,
    model_totals: HashMap<String, (u32, u32, f64)>, // (input_tokens, output_tokens, cost)
}

impl CostTracker {
    pub fn new() -> Self {
        Self::default()
    }

    /// Record a new API call
    pub fn record(&mut self, model: impl Into<String>, usage: TokenUsage, description: impl Into<String>) {
        let entry = CostEntry::new(model, usage, description);
        let model_key = entry.model.clone();
        
        // Update totals
        let totals = self.model_totals.entry(model_key).or_insert((0, 0, 0.0));
        totals.0 += entry.usage.prompt_tokens;
        totals.1 += entry.usage.completion_tokens;
        totals.2 += entry.cost;
        
        self.entries.push(entry);
    }

    /// Get total cost across all models
    pub fn total_cost(&self) -> f64 {
        self.model_totals.values().map(|(_, _, cost)| cost).sum()
    }

    /// Get total tokens across all models
    pub fn total_tokens(&self) -> (u32, u32, u32) {
        let (input, output, _) = self.model_totals.values().fold(
            (0u32, 0u32, 0.0f64),
            |(i, o, _), (input, output, _)| (i + input, o + output, 0.0)
        );
        (input, output, input + output)
    }

    /// Get number of API calls
    pub fn call_count(&self) -> usize {
        self.entries.len()
    }

    /// Get breakdown by model
    pub fn by_model(&self) -> &HashMap<String, (u32, u32, f64)> {
        &self.model_totals
    }

    /// Generate summary report
    pub fn summary(&self) -> String {
        let (input, output, total) = self.total_tokens();
        let cost = self.total_cost();
        
        format!(
            "Session Usage:\n  Calls: {}\n  Input tokens: {}\n  Output tokens: {}\n  Total tokens: {}\n  Est. cost: ${:.6}",
            self.call_count(),
            input,
            output,
            total,
            cost
        )
    }

    /// Generate detailed report
    pub fn detailed_report(&self) -> String {
        let mut lines = vec![
            "═══════════════════════════════════════".to_string(),
            "           Cost Report                 ".to_string(),
            "═══════════════════════════════════════".to_string(),
        ];

        let (input, output, total) = self.total_tokens();
        lines.push(format!("Total Calls: {}", self.call_count()));
        lines.push(format!("Total Cost: ${:.6}", self.total_cost()));
        lines.push("".to_string());
        lines.push("Token Usage:".to_string());
        lines.push(format!("  Input:  {} tokens", input));
        lines.push(format!("  Output: {} tokens", output));
        lines.push(format!("  Total:  {} tokens", total));
        lines.push("".to_string());

        if !self.model_totals.is_empty() {
            lines.push("By Model:".to_string());
            for (model, (input_tokens, output_tokens, cost)) in &self.model_totals {
                lines.push(format!(
                    "  {}: {} in / {} out (${:.6})",
                    model, input_tokens, output_tokens, cost
                ));
            }
        }

        lines.push("═══════════════════════════════════════".to_string());
        lines.join("\n")
    }

    /// Clear all entries
    pub fn clear(&mut self) {
        self.entries.clear();
        self.model_totals.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_model_pricing() {
        let pricing = get_model_pricing("kimi-for-coding");
        assert!(pricing.input_price > 0.0);
        assert!(pricing.output_price > 0.0);
    }

    #[test]
    fn test_cost_calculation() {
        let pricing = ModelPricing::new(0.001, 0.002);
        let usage = TokenUsage::new(1000, 500);
        let cost = pricing.calculate_cost(&usage);
        
        // 1000/1000 * 0.001 + 500/1000 * 0.002 = 0.001 + 0.001 = 0.002
        assert!((cost - 0.002).abs() < 0.0001);
    }

    #[test]
    fn test_cost_tracker() {
        let mut tracker = CostTracker::new();
        
        tracker.record("test-model", TokenUsage::new(1000, 500), "Test call");
        tracker.record("test-model", TokenUsage::new(2000, 1000), "Another call");
        
        assert_eq!(tracker.call_count(), 2);
        
        let (input, output, total) = tracker.total_tokens();
        assert_eq!(input, 3000);
        assert_eq!(output, 1500);
        assert_eq!(total, 4500);
        
        assert!(tracker.total_cost() > 0.0);
    }

    #[test]
    fn test_summary() {
        let mut tracker = CostTracker::new();
        tracker.record("test", TokenUsage::new(1000, 500), "test");
        
        let summary = tracker.summary();
        assert!(summary.contains("Calls:"));
        assert!(summary.contains("tokens"));
        assert!(summary.contains("$"));
    }
}
