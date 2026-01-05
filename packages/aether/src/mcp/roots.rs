use rmcp::model::Root as RmcpRoot;
use std::path::PathBuf;

/// A root directory exposed to MCP servers.
///
/// Represents a workspace root that MCP servers can access. The MCP protocol
/// uses file:// URIs to identify roots, and clients advertise these roots to
/// servers during initialization or via dynamic updates.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Root {
    /// The file:// URI for the root (e.g., "file:///home/user/project")
    pub uri: String,
    /// Human-readable display name for the root (optional)
    pub name: Option<String>,
}

impl Root {
    /// Create a Root from a PathBuf.
    ///
    /// The path is converted to an absolute file:// URI. On Unix systems, this
    /// is straightforward. On Windows, drive letters are converted to the
    /// appropriate URI format (e.g., C:\ becomes file:///C:/).
    ///
    /// # Example
    /// ```
    /// use std::path::PathBuf;
    /// use aether::mcp::Root;
    ///
    /// let root = Root::from_path(
    ///     PathBuf::from("/home/user/project"),
    ///     Some("My Project".to_string())
    /// );
    /// assert_eq!(root.uri, "file:///home/user/project");
    /// assert_eq!(root.name, Some("My Project".to_string()));
    /// ```
    pub fn from_path(path: PathBuf, name: Option<String>) -> Self {
        let uri = path_to_file_uri(&path);
        Self { uri, name }
    }

    /// Extract the file path from this root's URI.
    ///
    /// Returns None if the URI is malformed or not a file:// URI.
    pub fn to_path(&self) -> Option<PathBuf> {
        file_uri_to_path(&self.uri)
    }
}

/// Convert an rmcp Root to our Root type.
impl From<RmcpRoot> for Root {
    fn from(root: RmcpRoot) -> Self {
        Self {
            uri: root.uri.to_string(),
            name: root.name,
        }
    }
}

/// Convert our Root to rmcp Root.
impl From<Root> for RmcpRoot {
    fn from(root: Root) -> Self {
        RmcpRoot {
            uri: root.uri.into(),
            name: root.name,
        }
    }
}

/// Convert a PathBuf to a file:// URI string.
///
/// This function handles platform-specific path formats:
/// - Unix: /home/user/project -> file:///home/user/project
/// - Windows: C:\Users\user\project -> file:///C:/Users/user/project
fn path_to_file_uri(path: &PathBuf) -> String {
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

/// Convert a file:// URI to a PathBuf.
///
/// Returns None if the URI is not a valid file:// URI.
fn file_uri_to_path(uri: &str) -> Option<PathBuf> {
    let uri = uri.strip_prefix("file://")?;

    #[cfg(unix)]
    {
        Some(PathBuf::from(uri))
    }

    #[cfg(windows)]
    {
        // Strip leading / and convert back to Windows format
        let path_str = uri.strip_prefix('/')?.replace('/', "\\");
        Some(PathBuf::from(path_str))
    }

    #[cfg(not(any(unix, windows)))]
    {
        Some(PathBuf::from(uri))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_root_from_path() {
        let path = PathBuf::from("/home/user/project");
        let root = Root::from_path(path, Some("Test Project".to_string()));

        assert_eq!(root.uri, "file:///home/user/project");
        assert_eq!(root.name, Some("Test Project".to_string()));
    }

    #[test]
    fn test_root_to_path() {
        let root = Root {
            uri: "file:///home/user/project".to_string(),
            name: None,
        };

        let path = root.to_path();
        assert_eq!(path, Some(PathBuf::from("/home/user/project")));
    }

    #[test]
    fn test_root_roundtrip() {
        let original_path = PathBuf::from("/tmp/test");
        let root = Root::from_path(original_path.clone(), None);
        let recovered_path = root.to_path();

        assert_eq!(Some(original_path), recovered_path);
    }

    #[test]
    fn test_rmcp_root_conversion() {
        let our_root = Root {
            uri: "file:///home/user/project".to_string(),
            name: Some("Test".to_string()),
        };

        let rmcp_root: RmcpRoot = our_root.clone().into();
        assert_eq!(rmcp_root.uri.as_str(), "file:///home/user/project");
        assert_eq!(rmcp_root.name, Some("Test".to_string()));

        let converted_back: Root = rmcp_root.into();
        assert_eq!(converted_back, our_root);
    }

    #[test]
    fn test_path_with_spaces() {
        let path = PathBuf::from("/home/user/my project");
        let root = Root::from_path(path.clone(), None);

        // The URI should preserve spaces (not percent-encoded in this simple implementation)
        assert_eq!(root.uri, "file:///home/user/my project");

        let recovered = root.to_path();
        assert_eq!(recovered, Some(path));
    }
}
