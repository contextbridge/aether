use rmcp::model::Root;
use std::path::{Path, PathBuf};

/// Convert a `PathBuf` to a file:// URI string.
///
/// This function handles platform-specific path formats:
/// - Unix: /home/user/project -> <file:///home/user/project>
/// - Windows: C:\Users\user\project -> <file:///C:/Users/user/project>
pub fn path_to_file_uri(path: &Path) -> String {
    #[cfg(unix)]
    {
        format!("file://{}", path.display())
    }

    #[cfg(windows)]
    {
        // Convert Windows paths to URI format
        let path_str = path.display().to_string().replace('\\', "/");
        format!("file:///{}", path_str)
    }

    #[cfg(not(any(unix, windows)))]
    {
        // Fallback for other platforms
        format!("file://{}", path.display())
    }
}

/// Convert a Root's file:// URI back to a `PathBuf`.
///
/// Returns `None` if the URI is not a valid file URI or cannot be converted
/// to a path on the current platform.
pub fn root_to_path(root: &Root) -> Option<PathBuf> {
    let uri = root.uri.as_str();
    if !uri.starts_with("file://") {
        return None;
    }

    let path_str = uri.strip_prefix("file://")?;

    #[cfg(unix)]
    {
        Some(PathBuf::from(path_str))
    }

    #[cfg(windows)]
    {
        // Windows URIs have format file:///C:/path, so strip leading /
        let path_str = path_str.strip_prefix('/').unwrap_or(path_str);
        // Convert forward slashes back to backslashes
        Some(PathBuf::from(path_str.replace('/', "\\")))
    }

    #[cfg(not(any(unix, windows)))]
    {
        Some(PathBuf::from(path_str))
    }
}

pub fn primary_root_path(roots: &[Root]) -> Option<PathBuf> {
    roots.iter().find_map(root_to_path)
}

/// Create a Root from a `PathBuf`.
///
/// The path is converted to an absolute file:// URI.
pub fn root_from_path(path: &Path, name: Option<String>) -> Root {
    let uri = path_to_file_uri(path);
    let root = Root::new(uri);
    match name {
        Some(n) => root.with_name(n),
        None => root,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_path_to_file_uri() {
        let path = PathBuf::from("/home/user/project");
        let uri = path_to_file_uri(&path);
        assert_eq!(uri, "file:///home/user/project");
    }

    #[test]
    fn test_primary_root_path_uses_first_valid_file_root() {
        let invalid = Root::new("https://example.com".to_string());
        let root = root_from_path(&PathBuf::from("/tmp/test"), None);
        assert_eq!(primary_root_path(&[invalid, root]), Some(PathBuf::from("/tmp/test")));
    }

    #[test]
    fn test_root_from_path() {
        let path = PathBuf::from("/home/user/project");
        let root = root_from_path(&path, Some("Test Project".to_string()));

        assert_eq!(root.uri.as_str(), "file:///home/user/project");
        assert_eq!(root.name, Some("Test Project".to_string()));
    }

    #[test]
    fn test_root_from_path_no_name() {
        let path = PathBuf::from("/tmp/test");
        let root = root_from_path(&path, None);

        assert_eq!(root.uri.as_str(), "file:///tmp/test");
        assert_eq!(root.name, None);
    }

    #[test]
    fn test_path_with_spaces() {
        let path = PathBuf::from("/home/user/my project");
        let root = root_from_path(&path, None);

        // The URI should preserve spaces (not percent-encoded in this simple implementation)
        assert_eq!(root.uri.as_str(), "file:///home/user/my project");
    }

    #[test]
    fn test_root_to_path_unix() {
        let root = Root::new("file:///home/user/project");
        let path = root_to_path(&root).expect("Should convert to path");
        assert_eq!(path, PathBuf::from("/home/user/project"));
    }

    #[test]
    fn test_root_to_path_roundtrip() {
        let original = PathBuf::from("/home/user/project");
        let root = root_from_path(&original, None);
        let converted = root_to_path(&root).expect("Should convert back to path");
        assert_eq!(converted, original);
    }

    #[test]
    fn test_root_to_path_invalid_uri() {
        let root = Root::new("http://example.com");
        assert!(root_to_path(&root).is_none());
    }
}
