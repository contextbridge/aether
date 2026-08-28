//! URI↔path conversion utilities for LSP file URIs.

use lsp_types::Uri;
use std::path::{Path, PathBuf};
use url::Url;

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

    let url = Url::from_file_path(&absolute)
        .map_err(|()| UriError { path: absolute.clone(), reason: "not an absolute file path".to_string() })?;

    url.as_str().parse::<Uri>().map_err(|e| UriError { path: absolute, reason: e.to_string() })
}

/// Convert an LSP `file://` URI to a file path string.
///
/// URIs that do not address a local file are returned unchanged.
pub fn uri_to_path(uri: &Uri) -> String {
    let uri_str = uri.as_str();
    Url::parse(uri_str)
        .ok()
        .and_then(|url| url.to_file_path().ok())
        .map_or_else(|| uri_str.to_string(), |path| path.to_string_lossy().into_owned())
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

    #[test]
    fn path_to_uri_percent_encodes_reserved_characters() {
        let cases = [
            ("/src/my file.rs", "/src/my%20file.rs"),
            ("/src/a#b.rs", "/src/a%23b.rs"),
            ("/src/a?b.rs", "/src/a%3Fb.rs"),
            ("/src/100%.rs", "/src/100%25.rs"),
            ("/src/café.rs", "/src/caf%C3%A9.rs"),
        ];

        for (path, expected_uri_path) in cases {
            let uri = path_to_uri(Path::new(path)).unwrap();
            assert_eq!(uri.as_str(), format!("file://{expected_uri_path}"), "path: {path}");
        }
    }

    #[test]
    fn uri_to_path_decodes_percent_encoding() {
        let cases = [
            ("file:///src/my%20file.rs", "/src/my file.rs"),
            ("file:///src/a%23b.rs", "/src/a#b.rs"),
            ("file:///src/caf%C3%A9.rs", "/src/café.rs"),
        ];

        for (uri_str, expected_path) in cases {
            let uri: Uri = uri_str.parse().unwrap();
            assert_eq!(uri_to_path(&uri), expected_path, "uri: {uri_str}");
        }
    }

    #[test]
    fn roundtrips_paths_containing_reserved_characters() {
        let path = "/home/user/my project/a#b.rs";
        let uri = path_to_uri(Path::new(path)).unwrap();
        assert!(uri.as_str().contains('%'), "expected percent-encoding in {}", uri.as_str());
        assert_eq!(uri_to_path(&uri), path);
    }

    #[test]
    fn uri_to_path_passes_non_file_uris_through_unchanged() {
        let uri: Uri = "https://example.com/file.rs".parse().unwrap();
        assert_eq!(uri_to_path(&uri), "https://example.com/file.rs");
    }
}
