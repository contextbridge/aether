use rmcp::model::Root;
use std::path::PathBuf;

/// Convert a PathBuf to a file:// URI string.
///
/// This function handles platform-specific path formats:
/// - Unix: /home/user/project -> file:///home/user/project
/// - Windows: C:\Users\user\project -> file:///C:/Users/user/project
pub fn path_to_file_uri(path: &PathBuf) -> String {
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

/// Create a Root from a PathBuf.
///
/// The path is converted to an absolute file:// URI.
///
/// # Example
/// ```
/// use std::path::PathBuf;
/// use aether::mcp::root_from_path;
///
/// let root = root_from_path(
///     PathBuf::from("/home/user/project"),
///     Some("My Project".to_string())
/// );
/// assert_eq!(root.uri.as_str(), "file:///home/user/project");
/// ```
pub fn root_from_path(path: PathBuf, name: Option<String>) -> Root {
    let uri = path_to_file_uri(&path);
    Root {
        uri: uri.into(),
        name,
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
    fn test_root_from_path() {
        let path = PathBuf::from("/home/user/project");
        let root = root_from_path(path, Some("Test Project".to_string()));

        assert_eq!(root.uri.as_str(), "file:///home/user/project");
        assert_eq!(root.name, Some("Test Project".to_string()));
    }

    #[test]
    fn test_root_from_path_no_name() {
        let path = PathBuf::from("/tmp/test");
        let root = root_from_path(path, None);

        assert_eq!(root.uri.as_str(), "file:///tmp/test");
        assert_eq!(root.name, None);
    }

    #[test]
    fn test_path_with_spaces() {
        let path = PathBuf::from("/home/user/my project");
        let root = root_from_path(path, None);

        // The URI should preserve spaces (not percent-encoded in this simple implementation)
        assert_eq!(root.uri.as_str(), "file:///home/user/my project");
    }
}
