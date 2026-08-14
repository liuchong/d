//! Request path sanitization and safe resolution against the server root.

use std::path::{Component, Path, PathBuf};

/// Result of resolving a requested URL path against the server root.
pub(crate) enum PathResolution {
    /// Path is safe to serve (canonical, inside root).
    Resolved(PathBuf),
    /// Path is syntactically invalid or attempts traversal (respond 403).
    Invalid,
    /// Path does not exist or escapes the root via symlink (respond 404).
    NotFound,
}

/// Sanitize a requested path purely lexically: reject Windows prefixes and
/// clamp `..` components at the root. Does not touch the filesystem.
pub(crate) fn sanitize_path(base: &Path, requested: &str) -> Option<PathBuf> {
    let mut result = base.to_path_buf();

    for component in Path::new(requested).components() {
        match component {
            Component::Normal(c) => result.push(c),
            Component::ParentDir => {
                if result != *base {
                    result.pop();
                }
            }
            Component::RootDir | Component::CurDir => {}
            Component::Prefix(_) => return None,
        }
    }

    if result.starts_with(base) {
        Some(result)
    } else {
        None
    }
}

/// Resolve a requested path against `root` (which must already be
/// canonical). The final candidate is canonicalized so symlinks pointing
/// outside the root are rejected with [`PathResolution::NotFound`]
/// (a missing file also yields `NotFound`, avoiding existence leaks).
pub(crate) async fn resolve_path(
    root: &Path,
    requested: &str,
) -> PathResolution {
    let candidate = match sanitize_path(root, requested) {
        Some(p) => p,
        None => return PathResolution::Invalid,
    };

    match tokio::fs::canonicalize(&candidate).await {
        Ok(canonical) if canonical.starts_with(root) => {
            PathResolution::Resolved(canonical)
        }
        Ok(canonical) => {
            tracing::warn!(
                "Symlink escape blocked: {} -> {}",
                candidate.display(),
                canonical.display()
            );
            PathResolution::NotFound
        }
        Err(_) => PathResolution::NotFound,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_simple_path() {
        let base = Path::new("/srv/root");
        assert_eq!(
            sanitize_path(base, "a/b/c.txt"),
            Some(PathBuf::from("/srv/root/a/b/c.txt"))
        );
    }

    #[test]
    fn sanitize_empty_path() {
        let base = Path::new("/srv/root");
        assert_eq!(sanitize_path(base, ""), Some(PathBuf::from("/srv/root")));
        assert_eq!(sanitize_path(base, "/"), Some(PathBuf::from("/srv/root")));
    }

    #[test]
    fn sanitize_parent_dir_within_root() {
        let base = Path::new("/srv/root");
        assert_eq!(
            sanitize_path(base, "a/../b"),
            Some(PathBuf::from("/srv/root/b"))
        );
    }

    #[test]
    fn sanitize_parent_dir_clamped_at_root() {
        let base = Path::new("/srv/root");
        // `..` at the root is clamped, so the result never escapes.
        assert_eq!(
            sanitize_path(base, "../../etc/passwd"),
            Some(PathBuf::from("/srv/root/etc/passwd"))
        );
    }

    #[test]
    fn sanitize_dot_components_ignored() {
        let base = Path::new("/srv/root");
        assert_eq!(
            sanitize_path(base, "./a/./b"),
            Some(PathBuf::from("/srv/root/a/b"))
        );
    }

    #[tokio::test]
    async fn resolve_existing_file() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        std::fs::write(root.join("hello.txt"), "hi").unwrap();

        match resolve_path(&root, "hello.txt").await {
            PathResolution::Resolved(p) => {
                assert_eq!(p, root.join("hello.txt"))
            }
            _ => panic!("expected Resolved"),
        }
    }

    #[tokio::test]
    async fn resolve_missing_file_is_not_found() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();

        assert!(matches!(
            resolve_path(&root, "nope.txt").await,
            PathResolution::NotFound
        ));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn resolve_symlink_escape_is_not_found() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        // A symlink inside the root pointing outside of it.
        std::os::unix::fs::symlink("/etc/hosts", root.join("escape")).unwrap();

        assert!(matches!(
            resolve_path(&root, "escape").await,
            PathResolution::NotFound
        ));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn resolve_symlink_within_root_ok() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        std::fs::write(root.join("real.txt"), "hi").unwrap();
        std::os::unix::fs::symlink(root.join("real.txt"), root.join("link"))
            .unwrap();

        match resolve_path(&root, "link").await {
            PathResolution::Resolved(p) => {
                assert_eq!(p, root.join("real.txt"))
            }
            _ => panic!("expected Resolved"),
        }
    }
}
