//! Worktree management for multiple workspace directories
//!
//! Provides:
//! - Multiple workspace tracking
//! - Worktree switching
//! - Path resolution across worktrees

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use tracing::{info, warn};

/// A worktree represents a workspace directory
#[derive(Debug, Clone)]
pub struct Worktree {
    /// Unique identifier
    pub id: String,
    /// Human-readable name
    pub name: String,
    /// Absolute path to worktree root
    pub path: PathBuf,
    /// Whether this is the active worktree
    pub active: bool,
    /// Associated metadata
    pub metadata: HashMap<String, String>,
}

impl Worktree {
    /// Create a new worktree
    pub fn new(id: impl Into<String>, name: impl Into<String>, path: impl Into<PathBuf>) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            path: path.into(),
            active: false,
            metadata: HashMap::new(),
        }
    }

    /// Set as active
    pub fn set_active(&mut self, active: bool) {
        self.active = active;
    }

    /// Resolve a relative path within this worktree
    pub fn resolve_path(&self, relative: impl AsRef<Path>) -> PathBuf {
        self.path.join(relative)
    }

    /// Check if a path is within this worktree
    pub fn contains(&self, path: impl AsRef<Path>) -> bool {
        let path = path.as_ref();
        path.starts_with(&self.path)
    }
}

/// Worktree manager
pub struct WorktreeManager {
    worktrees: HashMap<String, Worktree>,
    active_id: Option<String>,
    default_path: PathBuf,
}

impl WorktreeManager {
    /// Create a new manager
    pub fn new(default_path: impl Into<PathBuf>) -> Self {
        Self {
            worktrees: HashMap::new(),
            active_id: None,
            default_path: default_path.into(),
        }
    }

    /// Add a worktree
    pub fn add(&mut self, worktree: Worktree) {
        info!("Adding worktree: {} at {:?}", worktree.name, worktree.path);
        self.worktrees.insert(worktree.id.clone(), worktree);
    }

    /// Remove a worktree
    pub fn remove(&mut self, id: &str) -> Option<Worktree> {
        if self.active_id.as_deref() == Some(id) {
            self.active_id = None;
        }
        self.worktrees.remove(id)
    }

    /// Get a worktree by ID
    pub fn get(&self, id: &str) -> Option<&Worktree> {
        self.worktrees.get(id)
    }

    /// Get mutable worktree
    pub fn get_mut(&mut self, id: &str) -> Option<&mut Worktree> {
        self.worktrees.get_mut(id)
    }

    /// Set active worktree
    pub fn set_active(&mut self, id: impl Into<String>) -> anyhow::Result<()> {
        let id = id.into();
        
        if !self.worktrees.contains_key(&id) {
            anyhow::bail!("Worktree not found: {}", id);
        }

        // Deactivate current
        if let Some(active_id) = &self.active_id {
            if let Some(wt) = self.worktrees.get_mut(active_id) {
                wt.active = false;
            }
        }

        // Activate new
        if let Some(wt) = self.worktrees.get_mut(&id) {
            wt.active = true;
            info!("Activated worktree: {}", wt.name);
        }

        self.active_id = Some(id);
        Ok(())
    }

    /// Get active worktree
    pub fn active(&self) -> Option<&Worktree> {
        self.active_id.as_ref().and_then(|id| self.worktrees.get(id))
    }

    /// List all worktrees
    pub fn list(&self) -> Vec<&Worktree> {
        self.worktrees.values().collect()
    }

    /// Resolve path in active worktree or default
    pub fn resolve_path(&self, relative: impl AsRef<Path>) -> PathBuf {
        if let Some(active) = self.active() {
            active.resolve_path(relative)
        } else {
            self.default_path.join(relative)
        }
    }

    /// Auto-detect worktrees from git repositories
    pub fn auto_detect(&mut self, root: impl AsRef<Path>) -> anyhow::Result<()> {
        let root = root.as_ref();
        
        if !root.exists() {
            return Ok(());
        }

        // Look for .git directories
        for entry in walkdir::WalkDir::new(root)
            .max_depth(3)
            .into_iter()
            .filter_map(|e| e.ok())
        {
            if entry.file_name() == ".git" && entry.file_type().is_dir() {
                let repo_root = entry.path().parent().unwrap_or(root);
                let name = repo_root
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("unknown");
                
                let id = name.to_lowercase().replace(" ", "-");
                let worktree = Worktree::new(&id, name, repo_root);
                
                if !self.worktrees.contains_key(&id) {
                    self.add(worktree);
                }
            }
        }

        Ok(())
    }
}

impl Default for WorktreeManager {
    fn default() -> Self {
        Self::new(std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_worktree() {
        let wt = Worktree::new("test", "Test Project", "/home/user/project");
        assert_eq!(wt.id, "test");
        assert_eq!(wt.name, "Test Project");
        assert!(!wt.active);
    }

    #[test]
    fn test_worktree_manager() {
        let mut manager = WorktreeManager::default();
        
        let wt = Worktree::new("wt1", "Worktree 1", "/path/to/wt1");
        manager.add(wt);
        
        assert_eq!(manager.list().len(), 1);
        assert!(manager.get("wt1").is_some());
    }

    #[test]
    fn test_active_worktree() {
        let mut manager = WorktreeManager::default();
        
        let wt = Worktree::new("wt1", "Worktree 1", "/path/to/wt1");
        manager.add(wt);
        
        manager.set_active("wt1").unwrap();
        
        let active = manager.active().unwrap();
        assert_eq!(active.id, "wt1");
        assert!(active.active);
    }
}
