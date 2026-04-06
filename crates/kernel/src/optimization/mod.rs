//! Toolchain optimization utilities
//!
//! Provides:
//! - Performance optimization suggestions
//! - Code analysis for improvements
//! - Caching strategies
//! - Resource optimization

use std::collections::HashMap;
use std::time::Duration;
use tracing::{debug, info, warn};

/// Optimization category
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum OptimizationCategory {
    /// Performance improvements
    Performance,
    /// Memory usage
    Memory,
    /// Startup time
    Startup,
    /// Binary size
    BinarySize,
    /// Cache efficiency
    Caching,
    /// I/O operations
    Io,
    /// Network efficiency
    Network,
}

impl std::fmt::Display for OptimizationCategory {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            OptimizationCategory::Performance => write!(f, "Performance"),
            OptimizationCategory::Memory => write!(f, "Memory"),
            OptimizationCategory::Startup => write!(f, "Startup"),
            OptimizationCategory::BinarySize => write!(f, "BinarySize"),
            OptimizationCategory::Caching => write!(f, "Caching"),
            OptimizationCategory::Io => write!(f, "I/O"),
            OptimizationCategory::Network => write!(f, "Network"),
        }
    }
}

/// Priority level for optimizations
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Priority {
    Low = 1,
    Medium = 2,
    High = 3,
    Critical = 4,
}

/// Optimization suggestion
#[derive(Debug, Clone)]
pub struct OptimizationSuggestion {
    /// Unique identifier
    pub id: String,
    /// Category
    pub category: OptimizationCategory,
    /// Priority
    pub priority: Priority,
    /// Title
    pub title: String,
    /// Detailed description
    pub description: String,
    /// Expected impact (e.g., "20% faster")
    pub expected_impact: String,
    /// How to implement
    pub implementation: String,
    /// Code example (if applicable)
    pub code_example: Option<String>,
    /// Estimated effort
    pub effort: Effort,
}

/// Effort required for implementation
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Effort {
    Quick,      // Minutes
    Short,      // Hours
    Medium,     // Days
    Large,      // Weeks
}

/// Optimization analyzer
pub struct OptimizationAnalyzer {
    metrics: HashMap<String, f64>,
    suggestions: Vec<OptimizationSuggestion>,
}

impl OptimizationAnalyzer {
    /// Create a new analyzer
    pub fn new() -> Self {
        Self {
            metrics: HashMap::new(),
            suggestions: Vec::new(),
        }
    }

    /// Record a metric
    pub fn record_metric(&mut self, name: impl Into<String>, value: f64) {
        self.metrics.insert(name.into(), value);
    }

    /// Get a metric
    pub fn get_metric(&self, name: &str) -> Option<f64> {
        self.metrics.get(name).copied()
    }

    /// Add a suggestion
    pub fn add_suggestion(&mut self, suggestion: OptimizationSuggestion) {
        info!("Adding optimization suggestion: {}", suggestion.title);
        self.suggestions.push(suggestion);
    }

    /// Get all suggestions
    pub fn suggestions(&self) -> &[OptimizationSuggestion] {
        &self.suggestions
    }

    /// Get suggestions by category
    pub fn suggestions_by_category(&self, category: OptimizationCategory) -> Vec<&OptimizationSuggestion> {
        self.suggestions
            .iter()
            .filter(|s| s.category == category)
            .collect()
    }

    /// Get high priority suggestions
    pub fn high_priority_suggestions(&self) -> Vec<&OptimizationSuggestion> {
        self.suggestions
            .iter()
            .filter(|s| s.priority >= Priority::High)
            .collect()
    }

    /// Analyze cache efficiency
    pub fn analyze_cache_efficiency(&mut self, hits: u64, misses: u64) {
        let total = hits + misses;
        if total == 0 {
            return;
        }

        let hit_rate = hits as f64 / total as f64;
        self.record_metric("cache_hit_rate", hit_rate);

        if hit_rate < 0.7 {
            self.add_suggestion(OptimizationSuggestion {
                id: "cache-001".to_string(),
                category: OptimizationCategory::Caching,
                priority: Priority::High,
                title: "Low cache hit rate".to_string(),
                description: format!("Cache hit rate is {:.1}%, consider increasing cache size or improving key strategy", hit_rate * 100.0),
                expected_impact: "Reduced latency and external calls".to_string(),
                implementation: "Review cache key generation and increase TTL".to_string(),
                code_example: None,
                effort: Effort::Short,
            });
        }
    }

    /// Analyze memory usage
    pub fn analyze_memory_usage(&mut self, current_mb: f64, peak_mb: f64) {
        self.record_metric("memory_current_mb", current_mb);
        self.record_metric("memory_peak_mb", peak_mb);

        if peak_mb > 1024.0 {
            self.add_suggestion(OptimizationSuggestion {
                id: "mem-001".to_string(),
                category: OptimizationCategory::Memory,
                priority: Priority::Medium,
                title: "High memory usage".to_string(),
                description: format!("Peak memory usage is {:.0} MB", peak_mb),
                expected_impact: "Lower memory footprint".to_string(),
                implementation: "Use streaming for large data, reduce buffer sizes".to_string(),
                code_example: Some("// Use iterator instead of collecting\nlet results: Vec<_> = items.collect();\n\n// Better:\nfor item in items {\n    process(item);\n}".to_string()),
                effort: Effort::Medium,
            });
        }
    }

    /// Generate report
    pub fn generate_report(&self) -> OptimizationReport {
        let mut by_category: HashMap<OptimizationCategory, Vec<&OptimizationSuggestion>> = HashMap::new();
        
        for suggestion in &self.suggestions {
            by_category
                .entry(suggestion.category)
                .or_default()
                .push(suggestion);
        }

        OptimizationReport {
            total_suggestions: self.suggestions.len(),
            by_category: by_category
                .into_iter()
                .map(|(k, v)| (k, v.into_iter().cloned().collect()))
                .collect(),
            high_priority: self.high_priority_suggestions()
                .into_iter()
                .cloned()
                .collect(),
            metrics: self.metrics.clone(),
        }
    }
}

impl Default for OptimizationAnalyzer {
    fn default() -> Self {
        Self::new()
    }
}

/// Optimization report
#[derive(Debug, Clone)]
pub struct OptimizationReport {
    pub total_suggestions: usize,
    pub by_category: HashMap<OptimizationCategory, Vec<OptimizationSuggestion>>,
    pub high_priority: Vec<OptimizationSuggestion>,
    pub metrics: HashMap<String, f64>,
}

impl OptimizationReport {
    /// Format as markdown
    pub fn to_markdown(&self) -> String {
        let mut output = String::new();
        
        output.push_str("# Optimization Report\n\n");
        output.push_str(&format!("**Total Suggestions:** {}\n\n", self.total_suggestions));
        
        if !self.high_priority.is_empty() {
            output.push_str("## High Priority\n\n");
            for s in &self.high_priority {
                output.push_str(&format!("### {}\n", s.title));
                output.push_str(&format!("- **Category:** {}\n", s.category));
                output.push_str(&format!("- **Impact:** {}\n", s.expected_impact));
                output.push_str(&format!("- **Effort:** {:?}\n", s.effort));
                output.push_str(&format!("\n{}\n\n", s.description));
            }
        }

        output.push_str("## Metrics\n\n");
        for (name, value) in &self.metrics {
            output.push_str(&format!("- {}: {:.2}\n", name, value));
        }

        output
    }
}

/// Profile-guided optimization data
#[derive(Debug, Clone, Default)]
pub struct ProfileData {
    pub hot_paths: Vec<HotPath>,
    pub function_frequency: HashMap<String, u64>,
    pub call_graph: HashMap<String, Vec<String>>,
}

/// Hot code path
#[derive(Debug, Clone)]
pub struct HotPath {
    pub path: String,
    pub execution_count: u64,
    pub total_time: Duration,
}

impl ProfileData {
    /// Create empty profile data
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a hot path
    pub fn add_hot_path(&mut self, path: impl Into<String>, count: u64, time: Duration) {
        self.hot_paths.push(HotPath {
            path: path.into(),
            execution_count: count,
            total_time: time,
        });
        
        // Sort by execution time
        self.hot_paths.sort_by(|a, b| b.total_time.cmp(&a.total_time));
    }

    /// Get top hot paths
    pub fn top_hot_paths(&self, n: usize) -> &[HotPath] {
        &self.hot_paths[..self.hot_paths.len().min(n)]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_optimization_analyzer() {
        let mut analyzer = OptimizationAnalyzer::new();
        analyzer.record_metric("test", 100.0);
        assert_eq!(analyzer.get_metric("test"), Some(100.0));
    }

    #[test]
    fn test_cache_analysis() {
        let mut analyzer = OptimizationAnalyzer::new();
        analyzer.analyze_cache_efficiency(50, 100); // 33% hit rate
        
        assert_eq!(analyzer.suggestions().len(), 1);
        assert_eq!(analyzer.suggestions()[0].category, OptimizationCategory::Caching);
    }

    #[test]
    fn test_report_generation() {
        let mut analyzer = OptimizationAnalyzer::new();
        analyzer.analyze_cache_efficiency(50, 100);
        
        let report = analyzer.generate_report();
        assert_eq!(report.total_suggestions, 1);
        assert!(!report.high_priority.is_empty());
    }
}
