use std::path::{Path, PathBuf};

/// Returns the primary (first) workspace root, falling back to `.` when none
/// are configured.
pub fn primary_root(roots: &[PathBuf]) -> PathBuf {
    roots.first().cloned().unwrap_or_else(|| PathBuf::from("."))
}

/// Resolves `path` against `root`, leaving absolute paths untouched.
pub fn resolve_path(root: &Path, path: PathBuf) -> PathBuf {
    if path.is_absolute() { path } else { root.join(path) }
}

/// Resolves a required file path against `root`. Whitespace-only input is
/// rejected; otherwise the text (including any surrounding spaces) is preserved
/// so unusual-but-valid file names round-trip unchanged.
pub fn resolve_file(root: &Path, raw: &str) -> Result<PathBuf, String> {
    if raw.trim().is_empty() {
        return Err("file path is required and cannot be empty".to_string());
    }
    Ok(resolve_path(root, PathBuf::from(raw)))
}

/// Resolves an optional directory against `root`. Missing or whitespace-only
/// input resolves to `root` itself.
pub fn resolve_dir(root: &Path, raw: Option<&str>) -> PathBuf {
    match raw {
        Some(raw) if !raw.trim().is_empty() => resolve_path(root, PathBuf::from(raw)),
        _ => root.to_path_buf(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn primary_root_falls_back_to_dot_when_empty() {
        assert_eq!(primary_root(&[]), PathBuf::from("."));
        assert_eq!(primary_root(&[PathBuf::from("/a"), PathBuf::from("/b")]), PathBuf::from("/a"));
    }

    #[test]
    fn resolves_relative_path_against_root() {
        assert_eq!(
            resolve_path(Path::new("/workspace"), PathBuf::from("src/main.rs")),
            PathBuf::from("/workspace/src/main.rs")
        );
    }

    #[test]
    fn leaves_absolute_path_untouched() {
        assert_eq!(resolve_path(Path::new("/workspace"), PathBuf::from("/etc/passwd")), PathBuf::from("/etc/passwd"));
    }

    #[test]
    fn preserves_non_empty_file_path_text() {
        let resolved = resolve_file(Path::new("/workspace"), " leading space.txt ").unwrap();
        assert_eq!(resolved, PathBuf::from("/workspace/ leading space.txt "));
    }

    #[test]
    fn rejects_whitespace_only_required_file_path() {
        let err = resolve_file(Path::new("/workspace"), "  \t ").unwrap_err();
        assert!(err.contains("file path is required"));
    }

    #[test]
    fn optional_empty_dir_resolves_to_root() {
        assert_eq!(resolve_dir(Path::new("/workspace"), Some("  ")), PathBuf::from("/workspace"));
        assert_eq!(resolve_dir(Path::new("/workspace"), None), PathBuf::from("/workspace"));
    }
}
