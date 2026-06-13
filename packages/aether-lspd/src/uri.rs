//! URI↔path conversion utilities for LSP file URIs.

use lsp_types::Uri;
use std::path::{Path, PathBuf};

#[derive(Debug, thiserror::Error)]
#[error("Invalid path '{}': {reason}", path.display())]
pub struct UriError {
    path: PathBuf,
    reason: String,
}

/// Convert a file path to an LSP `file://` URI.
///
/// Relative paths are resolved against the current working directory.
pub fn path_to_uri(path: &Path) -> Result<Uri, UriError> {
    let absolute =
        if path.is_absolute() { path.to_path_buf() } else { std::env::current_dir().unwrap_or_default().join(path) };
    // Canonicalize to resolve symlinks (e.g. /var → /private/var on macOS).
    // This ensures URIs match what language servers like rust-analyzer produce.
    let absolute = absolute.canonicalize().unwrap_or(absolute);

    #[cfg(windows)]
    let uri_str = {
        let path_str = absolute.to_string_lossy().replace('\\', "/");
        format!("file:///{}", path_str)
    };

    #[cfg(not(windows))]
    let uri_str = format!("file://{}", absolute.display());

    uri_str.parse::<Uri>().map_err(|e| UriError { path: absolute, reason: e.to_string() })
}

/// Convert an LSP `file://` URI to a file path string.
pub fn uri_to_path(uri: &Uri) -> String {
    let uri_str = uri.as_str();
    if let Some(path) = uri_str.strip_prefix("file://") {
        // Handle Windows paths (file:///C:/...)
        if path.starts_with('/') && path.len() > 2 && path.chars().nth(2) == Some(':') {
            path[1..].to_string()
        } else {
            path.to_string()
        }
    } else {
        uri_str.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn test_path_to_uri_absolute() {
        let uri = path_to_uri(Path::new("/src/main.rs")).unwrap();
        assert_eq!(uri.as_str(), "file:///src/main.rs");
    }

    #[test]
    fn test_uri_to_path_basic() {
        let uri: Uri = "file:///src/main.rs".parse().unwrap();
        assert_eq!(uri_to_path(&uri), "/src/main.rs");
    }

    #[test]
    fn test_roundtrip() {
        let path = Path::new("/home/user/project/src/lib.rs");
        let uri = path_to_uri(path).unwrap();
        let back = uri_to_path(&uri);
        assert_eq!(back, path.to_str().unwrap());
    }
}
