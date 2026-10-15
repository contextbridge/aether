use std::path::{Path, PathBuf};

pub fn current_dir() -> PathBuf {
    std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
}

/// Resolves `path` against `root`, leaving absolute paths untouched.
pub fn resolve_path(root: &Path, path: PathBuf) -> PathBuf {
    if path.is_absolute() { path } else { root.join(path) }
}

pub fn resolve_root(base: &Path, root: Option<PathBuf>) -> PathBuf {
    root.map_or_else(|| base.to_path_buf(), |path| resolve_path(base, path))
}

pub fn relative_path(root: &Path, path: &str) -> String {
    Path::new(path)
        .strip_prefix(root)
        .map_or_else(|_| path.to_string(), |relative| relative.to_string_lossy().to_string())
}

#[derive(Debug, thiserror::Error)]
#[error("file path is required and cannot be empty")]
pub struct EmptyFilePathError;

/// Resolves a required file path against `root`. Whitespace-only input is
/// rejected; otherwise the text (including any surrounding spaces) is preserved
/// so unusual-but-valid file names round-trip unchanged.
pub fn resolve_file(root: &Path, raw: &str) -> Result<PathBuf, EmptyFilePathError> {
    if raw.trim().is_empty() {
        return Err(EmptyFilePathError);
    }
    Ok(resolve_path(root, PathBuf::from(raw)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relative_path_strips_root_prefix() {
        assert_eq!(relative_path(Path::new("/workspace"), "/workspace/src/main.rs"), "src/main.rs");
        assert_eq!(relative_path(Path::new("/workspace"), "/elsewhere/lib.rs"), "/elsewhere/lib.rs");
    }

    #[test]
    fn resolves_relative_path_against_root() {
        assert_eq!(
            resolve_path(Path::new("/workspace"), PathBuf::from("src/main.rs")),
            PathBuf::from("/workspace/src/main.rs")
        );
    }

    #[test]
    fn root_override_resolves_against_base() {
        assert_eq!(resolve_root(Path::new("/workspace"), None), PathBuf::from("/workspace"));
        assert_eq!(resolve_root(Path::new("/workspace"), Some("sub".into())), PathBuf::from("/workspace/sub"));
        assert_eq!(resolve_root(Path::new("/workspace"), Some("/abs".into())), PathBuf::from("/abs"));
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
        assert!(err.to_string().contains("file path is required"));
    }
}
