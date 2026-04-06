//! Skills system for organizing tool capabilities
//!
//! Provides a hierarchical organization of tools and capabilities,
//! allowing the agent to discover and use related functionality.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// A skill represents a category of related capabilities
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Skill {
    pub id: String,
    pub name: String,
    pub description: String,
    pub category: SkillCategory,
    pub level: SkillLevel,
    pub capabilities: Vec<Capability>,
    pub related_tools: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent: Option<String>,
    pub children: Vec<String>,
    pub metadata: SkillMetadata,
}

/// Skill category
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SkillCategory {
    /// File system operations
    Filesystem,
    /// Code manipulation
    Code,
    /// Web operations
    Web,
    /// Data processing
    Data,
    /// System operations
    System,
    /// Communication
    Communication,
    /// Analysis
    Analysis,
    /// Custom category
    Custom,
}

/// Skill proficiency level
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum SkillLevel {
    Novice,
    Intermediate,
    Advanced,
    Expert,
}

impl SkillLevel {
    pub fn as_str(&self) -> &'static str {
        match self {
            SkillLevel::Novice => "novice",
            SkillLevel::Intermediate => "intermediate",
            SkillLevel::Advanced => "advanced",
            SkillLevel::Expert => "expert",
        }
    }
}

/// Individual capability within a skill
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Capability {
    pub id: String,
    pub name: String,
    pub description: String,
    pub action: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub example: Option<String>,
}

/// Skill metadata
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SkillMetadata {
    pub usage_count: u32,
    pub success_rate: f32,
    pub avg_execution_time_ms: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_used: Option<String>,
}

impl Skill {
    /// Create a new skill
    pub fn new(id: impl Into<String>, name: impl Into<String>, description: impl Into<String>, category: SkillCategory) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            description: description.into(),
            category,
            level: SkillLevel::Novice,
            capabilities: Vec::new(),
            related_tools: Vec::new(),
            parent: None,
            children: Vec::new(),
            metadata: SkillMetadata::default(),
        }
    }

    /// Set skill level
    pub fn with_level(mut self, level: SkillLevel) -> Self {
        self.level = level;
        self
    }

    /// Add a capability
    pub fn with_capability(mut self, capability: Capability) -> Self {
        self.capabilities.push(capability);
        self
    }

    /// Add related tool
    pub fn with_tool(mut self, tool: impl Into<String>) -> Self {
        self.related_tools.push(tool.into());
        self
    }

    /// Set parent skill
    pub fn with_parent(mut self, parent: impl Into<String>) -> Self {
        self.parent = Some(parent.into());
        self
    }

    /// Add child skill
    pub fn add_child(&mut self, child: impl Into<String>) {
        self.children.push(child.into());
    }

    /// Record usage
    pub fn record_usage(&mut self, success: bool, execution_time_ms: u64) {
        self.metadata.usage_count += 1;
        self.metadata.last_used = Some(chrono::Utc::now().to_rfc3339());
        
        // Update success rate
        let total = self.metadata.usage_count as f32;
        let current_success = self.metadata.success_rate * (total - 1.0);
        let new_success = current_success + if success { 1.0 } else { 0.0 };
        self.metadata.success_rate = new_success / total;

        // Update average execution time
        let current_total = self.metadata.avg_execution_time_ms * (self.metadata.usage_count - 1) as u64;
        self.metadata.avg_execution_time_ms = (current_total + execution_time_ms) / self.metadata.usage_count as u64;
    }

    /// Check if skill matches search query
    pub fn matches(&self, query: &str) -> bool {
        let query_lower = query.to_lowercase();
        self.id.to_lowercase().contains(&query_lower)
            || self.name.to_lowercase().contains(&query_lower)
            || self.description.to_lowercase().contains(&query_lower)
            || self.capabilities.iter().any(|c| {
                c.name.to_lowercase().contains(&query_lower)
                    || c.description.to_lowercase().contains(&query_lower)
            })
    }
}

/// Skills registry
#[derive(Debug, Default)]
pub struct SkillsRegistry {
    skills: HashMap<String, Skill>,
}

impl SkillsRegistry {
    /// Create a new registry with default skills
    pub fn new() -> Self {
        let mut registry = Self::default();
        registry.register_defaults();
        registry
    }

    /// Register a skill
    pub fn register(&mut self, skill: Skill) {
        self.skills.insert(skill.id.clone(), skill);
    }

    /// Get a skill by ID
    pub fn get(&self, id: &str) -> Option<&Skill> {
        self.skills.get(id)
    }

    /// Get mutable skill
    pub fn get_mut(&mut self, id: &str) -> Option<&mut Skill> {
        self.skills.get_mut(id)
    }

    /// Find skills matching query
    pub fn find(&self, query: &str) -> Vec<&Skill> {
        self.skills
            .values()
            .filter(|s| s.matches(query))
            .collect()
    }

    /// Get skills by category
    pub fn by_category(&self, category: SkillCategory) -> Vec<&Skill> {
        self.skills
            .values()
            .filter(|s| s.category == category)
            .collect()
    }

    /// Get all skills
    pub fn all(&self) -> Vec<&Skill> {
        self.skills.values().collect()
    }

    /// Get root skills (no parent)
    pub fn roots(&self) -> Vec<&Skill> {
        self.skills
            .values()
            .filter(|s| s.parent.is_none())
            .collect()
    }

    /// Get children of a skill
    pub fn children(&self, parent_id: &str) -> Vec<&Skill> {
        self.skills
            .values()
            .filter(|s| s.parent.as_deref() == Some(parent_id))
            .collect()
    }

    /// List all registered skill IDs
    pub fn list(&self) -> Vec<String> {
        self.skills.keys().cloned().collect()
    }

    /// Get skills for a specific tool
    pub fn for_tool(&self, tool: &str) -> Vec<&Skill> {
        self.skills
            .values()
            .filter(|s| s.related_tools.contains(&tool.to_string()))
            .collect()
    }

    /// Build skill tree representation
    pub fn build_tree(&self) -> SkillTree {
        let roots = self.roots();
        SkillTree {
            nodes: roots.iter().map(|s| self.build_tree_node(s.id.clone())).collect(),
        }
    }

    fn build_tree_node(&self, id: String) -> SkillTreeNode {
        let skill = self.skills.get(&id).cloned().unwrap();
        let children: Vec<_> = skill
            .children
            .iter()
            .map(|child_id| self.build_tree_node(child_id.clone()))
            .collect();

        SkillTreeNode {
            skill,
            children,
        }
    }

    /// Register default skills
    fn register_defaults(&mut self) {
        // File Operations
        let file_ops = Skill::new(
            "file_ops",
            "File Operations",
            "Reading, writing, and manipulating files",
            SkillCategory::Filesystem,
        )
        .with_level(SkillLevel::Expert)
        .with_tool("read_file")
        .with_tool("write_file")
        .with_tool("str_replace")
        .with_capability(Capability {
            id: "read".to_string(),
            name: "Read File".to_string(),
            description: "Read contents of a file".to_string(),
            action: "read_file".to_string(),
            example: Some("Read source code for analysis".to_string()),
        })
        .with_capability(Capability {
            id: "write".to_string(),
            name: "Write File".to_string(),
            description: "Create or overwrite a file".to_string(),
            action: "write_file".to_string(),
            example: Some("Create new configuration file".to_string()),
        });
        self.register(file_ops);

        // Directory Navigation
        let dir_nav = Skill::new(
            "dir_nav",
            "Directory Navigation",
            "Exploring and listing directory structures",
            SkillCategory::Filesystem,
        )
        .with_level(SkillLevel::Advanced)
        .with_tool("list_directory")
        .with_tool("glob")
        .with_tool("grep")
        .with_parent("file_ops")
        .with_capability(Capability {
            id: "list".to_string(),
            name: "List Directory".to_string(),
            description: "List contents of a directory".to_string(),
            action: "list_directory".to_string(),
            example: Some("List files in project root".to_string()),
        });
        self.register(dir_nav);

        // Code Analysis
        let code_analysis = Skill::new(
            "code_analysis",
            "Code Analysis",
            "Analyzing and understanding code structure",
            SkillCategory::Code,
        )
        .with_level(SkillLevel::Advanced)
        .with_tool("read_file")
        .with_tool("grep")
        .with_tool("git")
        .with_capability(Capability {
            id: "search_code".to_string(),
            name: "Search Code".to_string(),
            description: "Search for patterns in codebase".to_string(),
            action: "grep".to_string(),
            example: Some("Find all function definitions".to_string()),
        })
        .with_capability(Capability {
            id: "git_history".to_string(),
            name: "Git History".to_string(),
            description: "View git commit history".to_string(),
            action: "git log".to_string(),
            example: Some("See recent changes to a file".to_string()),
        });
        self.register(code_analysis);

        // Web Operations
        let web_ops = Skill::new(
            "web_ops",
            "Web Operations",
            "Fetching content from the web",
            SkillCategory::Web,
        )
        .with_level(SkillLevel::Intermediate)
        .with_tool("web_search")
        .with_tool("fetch_url")
        .with_capability(Capability {
            id: "search".to_string(),
            name: "Web Search".to_string(),
            description: "Search the web for information".to_string(),
            action: "web_search".to_string(),
            example: Some("Find latest documentation".to_string()),
        })
        .with_capability(Capability {
            id: "fetch".to_string(),
            name: "Fetch URL".to_string(),
            description: "Fetch content from a URL".to_string(),
            action: "fetch_url".to_string(),
            example: Some("Read API documentation".to_string()),
        });
        self.register(web_ops);

        // System Operations
        let sys_ops = Skill::new(
            "sys_ops",
            "System Operations",
            "Executing shell commands and system operations",
            SkillCategory::System,
        )
        .with_level(SkillLevel::Intermediate)
        .with_tool("shell")
        .with_capability(Capability {
            id: "exec".to_string(),
            name: "Execute Command".to_string(),
            description: "Execute a shell command".to_string(),
            action: "shell".to_string(),
            example: Some("Run build script".to_string()),
        });
        self.register(sys_ops);

        // Update parent-child relationships
        if let Some(parent) = self.get_mut("file_ops") {
            parent.add_child("dir_nav");
        }
    }
}

/// Skill tree node
#[derive(Debug, Clone)]
pub struct SkillTreeNode {
    pub skill: Skill,
    pub children: Vec<SkillTreeNode>,
}

/// Skill tree
#[derive(Debug, Clone)]
pub struct SkillTree {
    pub nodes: Vec<SkillTreeNode>,
}

impl SkillTree {
    /// Format as string representation
    pub fn format(&self) -> String {
        let mut lines = vec!["Skill Tree:".to_string()];
        for node in &self.nodes {
            self.format_node(node, &mut lines, 0);
        }
        lines.join("\n")
    }

    fn format_node(&self, node: &SkillTreeNode, lines: &mut Vec<String>, depth: usize) {
        let indent = "  ".repeat(depth);
        let level_icon = match node.skill.level {
            SkillLevel::Novice => "🔰",
            SkillLevel::Intermediate => "⭐",
            SkillLevel::Advanced => "⭐⭐",
            SkillLevel::Expert => "⭐⭐⭐",
        };
        lines.push(format!(
            "{}{} {} - {}",
            indent, level_icon, node.skill.name, node.skill.description
        ));

        for child in &node.children {
            self.format_node(child, lines, depth + 1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_skill_creation() {
        let skill = Skill::new("test", "Test Skill", "A test skill", SkillCategory::Custom)
            .with_level(SkillLevel::Advanced)
            .with_tool("test_tool");

        assert_eq!(skill.id, "test");
        assert_eq!(skill.level, SkillLevel::Advanced);
        assert_eq!(skill.related_tools.len(), 1);
    }

    #[test]
    fn test_skills_registry() {
        let registry = SkillsRegistry::new();
        assert!(!registry.all().is_empty());

        let file_ops = registry.get("file_ops");
        assert!(file_ops.is_some());
        assert_eq!(file_ops.unwrap().category, SkillCategory::Filesystem);
    }

    #[test]
    fn test_find_skills() {
        let registry = SkillsRegistry::new();
        let results = registry.find("file");
        assert!(!results.is_empty());
    }

    #[test]
    fn test_by_category() {
        let registry = SkillsRegistry::new();
        let fs_skills = registry.by_category(SkillCategory::Filesystem);
        assert!(!fs_skills.is_empty());
    }

    #[test]
    fn test_skill_usage_tracking() {
        let mut skill = Skill::new("test", "Test", "Desc", SkillCategory::Custom);
        
        skill.record_usage(true, 100);
        assert_eq!(skill.metadata.usage_count, 1);
        assert_eq!(skill.metadata.success_rate, 1.0);

        skill.record_usage(false, 200);
        assert_eq!(skill.metadata.usage_count, 2);
        assert_eq!(skill.metadata.success_rate, 0.5);
    }

    #[test]
    fn test_skill_tree() {
        let registry = SkillsRegistry::new();
        let tree = registry.build_tree();
        assert!(!tree.nodes.is_empty());
        
        let formatted = tree.format();
        assert!(!formatted.is_empty());
        assert!(formatted.contains("Skill Tree"));
    }
}
