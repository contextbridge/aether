//! TempDir-backed workspace fixture for exercising file-based tools.

use std::fs;
use std::os::unix::fs::symlink;
use std::path::{Path, PathBuf};
use tempfile::TempDir;

/// A temporary workspace directory on the real filesystem.
///
/// Builder methods create files and directories eagerly so each test reads as
/// a description of the workspace followed by assertions against it.
pub struct TestWorkspace {
    root: TempDir,
}

impl Default for TestWorkspace {
    fn default() -> Self {
        Self::new()
    }
}

impl TestWorkspace {
    pub fn new() -> Self {
        Self { root: TempDir::new().expect("failed to create temp workspace") }
    }

    /// Writes `contents` to `path` inside the workspace, creating parent directories.
    pub fn file(self, path: impl AsRef<Path>, contents: impl AsRef<[u8]>) -> Self {
        let path = self.join(path);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("failed to create parent directory");
        }
        fs::write(path, contents).expect("failed to write file");
        self
    }

    /// Creates a directory (and any missing parents) inside the workspace.
    pub fn dir(self, path: impl AsRef<Path>) -> Self {
        fs::create_dir_all(self.join(path)).expect("failed to create directory");
        self
    }

    /// Creates a symlink at `link` pointing to `target`; both resolved inside the workspace.
    #[cfg(unix)]
    pub fn symlink(self, target: impl AsRef<Path>, link: impl AsRef<Path>) -> Self {
        symlink(self.join(target), self.join(link)).expect("failed to create symlink");
        self
    }

    pub fn root(&self) -> &Path {
        self.root.path()
    }

    pub fn root_string(&self) -> String {
        self.root.path().to_string_lossy().into_owned()
    }

    /// Resolves `path` inside the workspace without requiring it to exist.
    pub fn path(&self, path: impl AsRef<Path>) -> PathBuf {
        self.join(path)
    }

    pub fn path_string(&self, path: impl AsRef<Path>) -> String {
        self.join(path).to_string_lossy().into_owned()
    }

    fn join(&self, path: impl AsRef<Path>) -> PathBuf {
        self.root.path().join(path)
    }
}
