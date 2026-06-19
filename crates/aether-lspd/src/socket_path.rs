use crate::language_catalog::LanguageId;
use crate::language_catalog::socket_identity_for_language;
use std::path::{Path, PathBuf};

#[doc = include_str!("docs/socket_path.md")]
pub fn socket_path(workspace_root: &Path, language: LanguageId) -> PathBuf {
    let socket_dir = get_socket_dir();
    let socket_name = generate_socket_name(workspace_root, language);
    socket_dir.join(socket_name)
}

/// Ensure the socket directory exists and return the socket path
pub fn ensure_socket_dir(workspace_root: &Path, language: LanguageId) -> std::io::Result<PathBuf> {
    let path = socket_path(workspace_root, language);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    Ok(path)
}

/// Get the lockfile path corresponding to a socket path
pub fn lockfile_path(socket_path: &Path) -> PathBuf {
    socket_path.with_extension("lock")
}

/// Get the log file path corresponding to a socket path.
///
/// The log file lives alongside the socket with a `.log` extension.
pub fn log_file_path(socket_path: &Path) -> PathBuf {
    socket_path.with_extension("log")
}

/// Get the directory where sockets are stored
///
/// Uses `XDG_RUNTIME_DIR` if available, otherwise falls back to /tmp/aether-lspd-{uid}
fn get_socket_dir() -> PathBuf {
    if let Ok(runtime_dir) = std::env::var("XDG_RUNTIME_DIR") {
        return PathBuf::from(runtime_dir).join("aether-lspd");
    }

    let uid = get_uid();
    PathBuf::from(format!("/tmp/aether-lspd-{uid}"))
}

/// Generate the socket filename from workspace and language
fn generate_socket_name(workspace_root: &Path, language: LanguageId) -> String {
    use sha2::{Digest, Sha256};

    let canonical = workspace_root.canonicalize().unwrap_or_else(|_| workspace_root.to_path_buf());

    let path_bytes = canonical.as_os_str().as_encoded_bytes();
    let hash = Sha256::digest(path_bytes);
    // Use first 8 bytes of SHA256 for a 16-char hex string
    let short_hash = u64::from_be_bytes(hash[..8].try_into().unwrap());

    format!("lsp-{}-{:016x}.sock", socket_identity_for_language(language), short_hash)
}

/// Get the current user's UID
#[cfg(unix)]
fn get_uid() -> u32 {
    unsafe { libc::getuid() }
}

#[cfg(not(unix))]
fn get_uid() -> u32 {
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_socket_path_deterministic() {
        let workspace = Path::new("/tmp/test-workspace");
        let path1 = socket_path(workspace, LanguageId::Rust);
        let path2 = socket_path(workspace, LanguageId::Rust);
        assert_eq!(path1, path2);
    }

    #[test]
    fn test_socket_path_different_languages() {
        let workspace = Path::new("/tmp/test-workspace");
        let rust_path = socket_path(workspace, LanguageId::Rust);
        let python_path = socket_path(workspace, LanguageId::Python);
        assert_ne!(rust_path, python_path);
    }

    #[test]
    fn test_socket_path_different_workspaces() {
        let workspace1 = Path::new("/tmp/workspace1");
        let workspace2 = Path::new("/tmp/workspace2");
        let path1 = socket_path(workspace1, LanguageId::Rust);
        let path2 = socket_path(workspace2, LanguageId::Rust);
        assert_ne!(path1, path2);
    }

    #[test]
    fn test_socket_path_contains_language() {
        let workspace = Path::new("/tmp/test-workspace");
        let path = socket_path(workspace, LanguageId::Rust);
        let filename = path.file_name().unwrap().to_str().unwrap();
        assert!(filename.contains("rust"));
        assert!(std::path::Path::new(filename).extension().is_some_and(|ext| ext.eq_ignore_ascii_case("sock")));
    }

    #[test]
    fn test_lockfile_path() {
        let socket = PathBuf::from("/tmp/aether-lspd-1000/lsp-rust-abc123.sock");
        let lockfile = lockfile_path(&socket);
        assert_eq!(lockfile, PathBuf::from("/tmp/aether-lspd-1000/lsp-rust-abc123.lock"));
    }

    #[test]
    fn socket_path_uses_shared_server_identity() {
        let workspace = Path::new("/tmp/test-workspace");
        assert_eq!(socket_path(workspace, LanguageId::TypeScript), socket_path(workspace, LanguageId::TypeScriptReact));
        assert_eq!(socket_path(workspace, LanguageId::C), socket_path(workspace, LanguageId::Cpp));
    }
}
