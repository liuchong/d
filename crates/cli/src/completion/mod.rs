//! Smart completion system for command line input
//!
//! Provides context-aware completions for:
//! - Slash commands (/help, /plan, etc.)
//! - Tool names and arguments
//! - File paths
//! - Command history

use std::collections::HashSet;

/// Completion context
#[derive(Debug, Clone)]
pub struct CompletionContext {
    /// Current input line
    pub input: String,
    /// Cursor position
    pub cursor_pos: usize,
    /// Current session context
    pub session_context: Option<String>,
    /// Available commands
    pub commands: Vec<String>,
    /// Available tools
    pub tools: Vec<String>,
    /// Command history
    pub history: Vec<String>,
}

impl CompletionContext {
    /// Create a new completion context
    pub fn new(input: impl Into<String>, cursor_pos: usize) -> Self {
        Self {
            input: input.into(),
            cursor_pos,
            session_context: None,
            commands: Vec::new(),
            tools: Vec::new(),
            history: Vec::new(),
        }
    }

    /// Add available commands
    pub fn with_commands(mut self, commands: Vec<String>) -> Self {
        self.commands = commands;
        self
    }

    /// Add available tools
    pub fn with_tools(mut self, tools: Vec<String>) -> Self {
        self.tools = tools;
        self
    }

    /// Add command history
    pub fn with_history(mut self, history: Vec<String>) -> Self {
        self.history = history;
        self
    }
}

/// Completion suggestion
#[derive(Debug, Clone, PartialEq)]
pub struct Completion {
    /// Text to insert
    pub text: String,
    /// Display text (may differ from insertion)
    pub display: String,
    /// Description of the completion
    pub description: String,
    /// Completion type
    pub kind: CompletionKind,
}

/// Completion type
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompletionKind {
    Command,
    Tool,
    File,
    Directory,
    History,
    Text,
}

impl Completion {
    /// Create a new completion
    pub fn new(text: impl Into<String>, kind: CompletionKind) -> Self {
        let text = text.into();
        Self {
            display: text.clone(),
            text,
            description: String::new(),
            kind,
        }
    }

    /// With display text
    pub fn with_display(mut self, display: impl Into<String>) -> Self {
        self.display = display.into();
        self
    }

    /// With description
    pub fn with_description(mut self, desc: impl Into<String>) -> Self {
        self.description = desc.into();
        self
    }
}

/// Fuzzy matcher for completions
pub struct FuzzyMatcher;

impl FuzzyMatcher {
    /// Calculate fuzzy match score
    pub fn score(query: &str, target: &str) -> f32 {
        let query_lower = query.to_lowercase();
        let target_lower = target.to_lowercase();
        
        // Exact match
        if target_lower == query_lower {
            return 100.0;
        }
        
        // Starts with
        if target_lower.starts_with(&query_lower) {
            return 90.0 - (target.len() - query.len()) as f32 * 0.5;
        }
        
        // Contains
        if target_lower.contains(&query_lower) {
            return 70.0 - target_lower.find(&query_lower).unwrap_or(0) as f32 * 2.0;
        }
        
        // Fuzzy match (character by character)
        let mut query_chars = query_lower.chars().peekable();
        let mut score = 0.0;
        let mut last_match_idx = 0;
        
        for (idx, target_char) in target_lower.chars().enumerate() {
            if let Some(&query_char) = query_chars.peek() {
                if target_char == query_char {
                    score += 10.0;
                    if idx == last_match_idx || idx == last_match_idx + 1 {
                        score += 5.0; // Consecutive bonus
                    }
                    last_match_idx = idx;
                    query_chars.next();
                }
            }
        }
        
        // Bonus for shorter matches
        if query_chars.peek().is_none() {
            score -= target.len() as f32 * 0.1;
            score
        } else {
            0.0 // Didn't match all characters
        }
    }
    
    /// Check if matches threshold
    pub fn matches(query: &str, target: &str, threshold: f32) -> bool {
        Self::score(query, target) >= threshold
    }
}

/// Smart completer
pub struct SmartCompleter {
    /// Slash commands
    slash_commands: Vec<(String, String)>,
    /// Tool completions cache
    tool_completions: Vec<Completion>,
    /// File completer
    file_completer: FileCompleter,
}

impl SmartCompleter {
    /// Create a new smart completer
    pub fn new() -> Self {
        Self {
            slash_commands: vec![
                ("/help".to_string(), "Show help message".to_string()),
                ("/plan".to_string(), "Toggle plan mode".to_string()),
                ("/yolo".to_string(), "Toggle yolo mode".to_string()),
                ("/sessions".to_string(), "List sessions".to_string()),
                ("/new".to_string(), "Start new session".to_string()),
                ("/save".to_string(), "Save current session".to_string()),
                ("/load".to_string(), "Load a session".to_string()),
                ("/cost".to_string(), "Show cost statistics".to_string()),
                ("/thinking".to_string(), "Configure thinking mode".to_string()),
                ("/tasks".to_string(), "Show background tasks".to_string()),
                ("/export".to_string(), "Export session".to_string()),
                ("/game".to_string(), "Start text adventure game".to_string()),
                ("/clear".to_string(), "Clear screen".to_string()),
                ("/quit".to_string(), "Exit application".to_string()),
            ],
            tool_completions: vec![
                Completion::new("read_file", CompletionKind::Tool)
                    .with_description("Read contents of a file"),
                Completion::new("write_file", CompletionKind::Tool)
                    .with_description("Write content to a file"),
                Completion::new("str_replace", CompletionKind::Tool)
                    .with_description("Replace text in a file"),
                Completion::new("list_directory", CompletionKind::Tool)
                    .with_description("List directory contents"),
                Completion::new("glob", CompletionKind::Tool)
                    .with_description("Find files matching pattern"),
                Completion::new("grep", CompletionKind::Tool)
                    .with_description("Search file contents"),
                Completion::new("shell", CompletionKind::Tool)
                    .with_description("Execute shell command"),
                Completion::new("git", CompletionKind::Tool)
                    .with_description("Execute git commands"),
                Completion::new("web_search", CompletionKind::Tool)
                    .with_description("Search the web"),
                Completion::new("fetch_url", CompletionKind::Tool)
                    .with_description("Fetch URL content"),
            ],
            file_completer: FileCompleter::new(),
        }
    }

    /// Get completions for context
    pub fn complete(&self, ctx: &CompletionContext) -> Vec<Completion> {
        let input = &ctx.input[..ctx.cursor_pos.min(ctx.input.len())];
        
        // Check what we're completing
        if input.starts_with('/') {
            // Slash command completion
            self.complete_slash_command(input, ctx)
        } else if input.starts_with("@") {
            // Tool completion
            self.complete_tool(&input[1..], ctx)
        } else if input.starts_with("./") || input.starts_with('/') || input.starts_with('~') {
            // File path completion
            self.file_completer.complete(input)
        } else {
            // General completion - check history and context
            self.complete_general(input, ctx)
        }
    }

    /// Complete slash commands with fuzzy matching
    fn complete_slash_command(&self, input: &str, _ctx: &CompletionContext) -> Vec<Completion> {
        let prefix = input.to_lowercase();
        
        let mut matches: Vec<_> = self.slash_commands
            .iter()
            .filter_map(|(cmd, desc)| {
                let score = FuzzyMatcher::score(&prefix, cmd);
                if score > 30.0 {
                    Some((score, cmd.clone(), desc.clone()))
                } else {
                    None
                }
            })
            .collect();
        
        // Sort by score descending
        matches.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap());
        
        matches.into_iter()
            .map(|(_, cmd, desc)| {
                Completion::new(cmd, CompletionKind::Command)
                    .with_description(desc)
            })
            .collect()
    }

    /// Complete tool names with fuzzy matching
    fn complete_tool(&self, input: &str, _ctx: &CompletionContext) -> Vec<Completion> {
        let prefix = input.to_lowercase();
        
        let mut matches: Vec<_> = self.tool_completions
            .iter()
            .filter_map(|c| {
                let score = FuzzyMatcher::score(&prefix, &c.text);
                if score > 30.0 {
                    Some((score, c.clone()))
                } else {
                    None
                }
            })
            .collect();
        
        matches.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap());
        matches.into_iter().map(|(_, c)| c).collect()
    }

    /// Complete general input
    fn complete_general(&self, input: &str, ctx: &CompletionContext) -> Vec<Completion> {
        let mut completions = Vec::new();
        let prefix = input.to_lowercase();

        // Add from history
        let seen: HashSet<_> = ctx.history.iter().cloned().collect();
        for item in seen {
            if item.to_lowercase().starts_with(&prefix) && item != input {
                completions.push(
                    Completion::new(item, CompletionKind::History)
                        .with_description("From history")
                );
            }
        }

        // Add available tools if input looks like a tool call
        if input.contains("tool") || input.contains("use") {
            for tool in &self.tool_completions {
                if !completions.iter().any(|c: &Completion| c.text == tool.text) {
                    completions.push(tool.clone());
                }
            }
        }

        completions
    }

    /// Add custom slash command
    pub fn add_slash_command(&mut self, command: impl Into<String>, description: impl Into<String>) {
        self.slash_commands.push((command.into(), description.into()));
    }

    /// Get all slash commands
    pub fn slash_commands(&self) -> &[(String, String)] {
        &self.slash_commands
    }
}

impl Default for SmartCompleter {
    fn default() -> Self {
        Self::new()
    }
}

/// File path completer
pub struct FileCompleter {
    // Could add caching here
}

impl FileCompleter {
    /// Create a new file completer
    pub fn new() -> Self {
        Self {}
    }

    /// Complete file paths
    pub fn complete(&self, input: &str) -> Vec<Completion> {
        use std::path::Path;

        let path = Path::new(input);
        let (dir_part, prefix_string): (&Path, String) = if input.ends_with('/') {
            (path, String::new())
        } else {
            match (path.parent(), path.file_name()) {
                (Some(dir), Some(name)) => {
                    let prefix = name.to_string_lossy().to_string();
                    (dir, prefix)
                }
                _ => (Path::new("."), input.to_string()),
            }
        };

        let mut completions = Vec::new();

        if let Ok(entries) = std::fs::read_dir(dir_part) {
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().to_string();
                let file_prefix = prefix_string.as_str();
                
                if name.starts_with(file_prefix) || file_prefix.is_empty() {
                    let is_dir = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
                    let full_path = dir_part.join(&name);
                    let display = if is_dir {
                        format!("{}/", name)
                    } else {
                        name.clone()
                    };

                    let text = full_path.to_string_lossy().to_string();
                    
                    completions.push(Completion {
                        text,
                        display,
                        description: if is_dir { "Directory".to_string() } else { "File".to_string() },
                        kind: if is_dir { CompletionKind::Directory } else { CompletionKind::File },
                    });
                }
            }
        }

        completions.sort_by(|a, b| {
            // Directories first
            match (a.kind, b.kind) {
                (CompletionKind::Directory, CompletionKind::File) => std::cmp::Ordering::Less,
                (CompletionKind::File, CompletionKind::Directory) => std::cmp::Ordering::Greater,
                _ => a.text.cmp(&b.text),
            }
        });

        completions
    }
}

impl Default for FileCompleter {
    fn default() -> Self {
        Self::new()
    }
}

/// Completion formatter
pub struct CompletionFormatter;

impl CompletionFormatter {
    /// Format completions for display
    pub fn format(completions: &[Completion]) -> String {
        if completions.is_empty() {
            return "No completions available.".to_string();
        }

        let mut lines = vec!["Completions:".to_string()];
        
        for (i, c) in completions.iter().take(10).enumerate() {
            let kind_icon = match c.kind {
                CompletionKind::Command => "",
                CompletionKind::Tool => "🔧",
                CompletionKind::File => "📄",
                CompletionKind::Directory => "📁",
                CompletionKind::History => "⏰",
                CompletionKind::Text => "💬",
            };
            
            lines.push(format!(
                "  {} {} {} - {}",
                i + 1,
                kind_icon,
                c.display,
                c.description
            ));
        }

        if completions.len() > 10 {
            lines.push(format!("  ... and {} more", completions.len() - 10));
        }

        lines.join("\n")
    }

    /// Format as single line for inline completion
    pub fn format_inline(completion: &Completion) -> String {
        format!("{} - {}", completion.display, completion.description)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_fuzzy_matcher_exact() {
        assert_eq!(FuzzyMatcher::score("help", "help"), 100.0);
    }

    #[test]
    fn test_fuzzy_matcher_starts_with() {
        let score = FuzzyMatcher::score("he", "help");
        assert!(score > 80.0 && score < 100.0);
    }

    #[test]
    fn test_fuzzy_matcher_contains() {
        let score = FuzzyMatcher::score("el", "help");
        assert!(score > 50.0 && score < 80.0);
    }

    #[test]
    fn test_fuzzy_matcher_fuzzy() {
        let score = FuzzyMatcher::score("hl", "help");
        assert!(score > 0.0);
    }

    #[test]
    fn test_fuzzy_matcher_no_match() {
        let score = FuzzyMatcher::score("xyz", "help");
        assert_eq!(score, 0.0);
    }

    #[test]
    fn test_fuzzy_matcher_case_insensitive() {
        let score_lower = FuzzyMatcher::score("help", "HELP");
        let score_upper = FuzzyMatcher::score("HELP", "help");
        assert!(score_lower > 0.0);
        assert!(score_upper > 0.0);
    }

    #[test]
    fn test_smart_completer_slash_commands() {
        let completer = SmartCompleter::new();
        let ctx = CompletionContext::new("/he", 3);
        
        let completions = completer.complete(&ctx);
        
        assert!(!completions.is_empty());
        assert!(completions.iter().any(|c| c.text == "/help"));
    }

    #[test]
    fn test_smart_completer_tools() {
        let completer = SmartCompleter::new();
        let ctx = CompletionContext::new("@re", 3);
        
        let completions = completer.complete(&ctx);
        
        assert!(!completions.is_empty());
        assert!(completions.iter().any(|c| c.text == "read_file"));
    }

    #[test]
    fn test_file_completer() {
        let completer = FileCompleter::new();
        let completions = completer.complete("./");
        
        // Should have at least some completions (Cargo.toml, src/, etc.)
        assert!(!completions.is_empty());
    }

    #[test]
    fn test_completion_formatter() {
        let completions = vec![
            Completion::new("/help", CompletionKind::Command)
                .with_description("Show help"),
            Completion::new("read_file", CompletionKind::Tool)
                .with_description("Read a file"),
        ];
        
        let formatted = CompletionFormatter::format(&completions);
        assert!(formatted.contains("Completions:"));
        assert!(formatted.contains("/help"));
    }

    #[test]
    fn test_completion_kinds() {
        let cmd = Completion::new("test", CompletionKind::Command);
        assert_eq!(cmd.kind, CompletionKind::Command);
        
        let tool = Completion::new("test", CompletionKind::Tool);
        assert_eq!(tool.kind, CompletionKind::Tool);
    }
}
