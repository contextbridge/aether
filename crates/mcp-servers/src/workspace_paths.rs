use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspacePaths {
    root: PathBuf,
}

impl WorkspacePaths {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn current() -> Self {
        Self::new(current_dir())
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn resolve_path(&self, path: PathBuf) -> PathBuf {
        resolve_path(&self.root, path)
    }

    pub fn resolve_file(&self, raw: &str) -> Result<PathBuf, EmptyFilePathError> {
        resolve_file(&self.root, raw)
    }

    pub fn resolve_dir(&self, raw: Option<&str>) -> PathBuf {
        resolve_dir(&self.root, raw)
    }

    pub fn make_relative(&self, path: &Path) -> Option<PathBuf> {
        path.strip_prefix(&self.root).ok().map(Path::to_path_buf)
    }
}

pub fn current_dir() -> PathBuf {
    std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
}

/// Resolves `path` against `root`, leaving absolute paths untouched.
pub fn resolve_path(root: &Path, path: PathBuf) -> PathBuf {
    if path.is_absolute() { path } else { root.join(path) }
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
    fn workspace_paths_resolve_against_root() {
        let workspace = WorkspacePaths::new("/workspace");
        assert_eq!(workspace.root(), Path::new("/workspace"));
        assert_eq!(workspace.resolve_path(PathBuf::from("src/main.rs")), PathBuf::from("/workspace/src/main.rs"));
        assert_eq!(workspace.resolve_dir(None), PathBuf::from("/workspace"));
        assert_eq!(workspace.make_relative(Path::new("/workspace/src/main.rs")), Some(PathBuf::from("src/main.rs")));
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
        assert!(err.to_string().contains("file path is required"));
    }

    #[test]
    fn optional_empty_dir_resolves_to_root() {
        assert_eq!(resolve_dir(Path::new("/workspace"), Some("  ")), PathBuf::from("/workspace"));
        assert_eq!(resolve_dir(Path::new("/workspace"), None), PathBuf::from("/workspace"));
    }
}
