//! Lexical path sanitization and canonical root-sandbox verification.

use std::path::{Component, Path, PathBuf};
use tokio::fs;

/// Result of resolving a request path within the served root directory.
#[derive(Debug, PartialEq, Eq)]
pub enum PathResolution {
    Resolved(PathBuf),
    Invalid,
    NotFound,
}

/// Lexically normalize `req_path` (dropping `.`, clamping `..` at root).
pub fn sanitize_path(req_path: &str) -> Option<PathBuf> {
    let mut sanitized = PathBuf::new();
    for component in Path::new(req_path).components() {
        match component {
            Component::Normal(part) => sanitized.push(part),
            Component::ParentDir => {
                sanitized.pop();
            }
            Component::CurDir | Component::RootDir | Component::Prefix(_) => {}
        }
    }
    Some(sanitized)
}

/// Resolve `req_path` inside canonical `root`, rejecting symlink escapes as `NotFound`.
pub async fn resolve_path(root: &Path, req_path: &str) -> PathResolution {
    let Some(rel) = sanitize_path(req_path) else {
        return PathResolution::Invalid;
    };
    let candidate = root.join(rel);
    match fs::canonicalize(&candidate).await {
        Ok(canonical) if canonical.starts_with(root) => {
            PathResolution::Resolved(canonical)
        }
        Ok(_) => PathResolution::NotFound,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            PathResolution::NotFound
        }
        Err(_) => PathResolution::NotFound,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn sanitize_cases() {
        assert_eq!(sanitize_path(""), Some(PathBuf::new()));
        assert_eq!(
            sanitize_path("/a/b/c.txt"),
            Some(PathBuf::from("a/b/c.txt"))
        );
        assert_eq!(sanitize_path("/a/./b/./c"), Some(PathBuf::from("a/b/c")));
        assert_eq!(sanitize_path("/a/b/../c"), Some(PathBuf::from("a/c")));
        assert_eq!(
            sanitize_path("/../../etc/passwd"),
            Some(PathBuf::from("etc/passwd"))
        );
    }

    #[tokio::test]
    async fn resolve_sandbox_and_symlinks() {
        let dir = TempDir::new().unwrap();
        let root = dir.path().canonicalize().unwrap();
        std::fs::write(root.join("ok.txt"), "hi").unwrap();

        assert_eq!(
            resolve_path(&root, "/ok.txt").await,
            PathResolution::Resolved(root.join("ok.txt"))
        );
        assert_eq!(
            resolve_path(&root, "/nope").await,
            PathResolution::NotFound
        );

        #[cfg(unix)]
        {
            std::os::unix::fs::symlink("/etc/hosts", root.join("escape"))
                .unwrap();
            assert_eq!(
                resolve_path(&root, "/escape").await,
                PathResolution::NotFound
            );

            std::os::unix::fs::symlink(root.join("ok.txt"), root.join("link"))
                .unwrap();
            assert_eq!(
                resolve_path(&root, "/link").await,
                PathResolution::Resolved(root.join("ok.txt"))
            );
        }
    }
}
