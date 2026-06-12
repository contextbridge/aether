use acp_utils::notifications::{WorkspaceEntry, WorkspaceMoveTarget};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use thiserror::Error;

mod git;
mod registry;
#[doc(hidden)]
pub mod testing;

use git::GitError;
use registry::{RegistryError, WorkspaceRegistry};

/// Manages the workspaces of a repository: listing the known clones and
/// moving uncommitted changes between them.
pub struct WorkspaceManager {
    registry: WorkspaceRegistry,
    cloner: Arc<dyn DirectoryCloner>,
}

/// Clones a directory into a new destination path.
pub trait DirectoryCloner: Send + Sync {
    fn clone_dir(&self, src: &Path, dst: &Path) -> Result<(), CloneError>;
}

/// Production directory cloner selected for the current platform.
pub struct PlatformDirectoryCloner;

/// Errors returned by [`WorkspaceManager`] operations.
#[derive(Debug, Error)]
pub enum WorkspaceError {
    #[error("{} is not a git repository: {source}", .path.display())]
    NotARepository { path: PathBuf, source: GitError },
    #[error("failed to resolve repository identity: {0}")]
    RepositoryIdentity(GitError),
    #[error("target workspace does not exist: {}", .0.display())]
    TargetMissing(PathBuf),
    #[error("workspace belongs to a different repository: {}", .0.display())]
    DifferentRepository(PathBuf),
    #[error("target workspace is on a different commit: {}", .0.display())]
    DifferentHead(PathBuf),
    #[error("session is already in that workspace")]
    AlreadyInWorkspace,
    #[error("workspace name must not be empty")]
    EmptyName,
    #[error("workspace name must not contain path separators: {0}")]
    NameSeparator(String),
    #[error("workspace name is reserved: {0}")]
    ReservedName(String),
    #[error("source repository root has no parent directory: {}", .0.display())]
    NoParent(PathBuf),
    #[error("target path already exists: {}", .0.display())]
    TargetPathExists(PathBuf),
    #[error("target workspace has uncommitted changes: {}", .0.display())]
    TargetDirty(PathBuf),
    #[error(transparent)]
    Registry(#[from] RegistryError),
    #[error(transparent)]
    Clone(#[from] CloneError),
    #[error(transparent)]
    Git(#[from] GitError),
    #[error("filesystem error: {0}")]
    Io(#[from] io::Error),
}

/// Errors returned when cloning a workspace directory.
#[derive(Debug, Error)]
pub enum CloneError {
    #[error("path contains an interior NUL byte: {}", .0.display())]
    InvalidPath(PathBuf),
    #[error("failed to clone {} to {}: {source}", .src.display(), .dst.display())]
    Clone { src: PathBuf, dst: PathBuf, source: io::Error },
}

impl WorkspaceManager {
    pub fn new() -> io::Result<Self> {
        Ok(Self::from_registry_and_cloner(WorkspaceRegistry::new()?, Arc::new(PlatformDirectoryCloner)))
    }

    pub fn from_registry_path(path: PathBuf) -> Self {
        Self::from_registry_and_cloner(WorkspaceRegistry::from_path(path), Arc::new(PlatformDirectoryCloner))
    }

    #[doc(hidden)]
    pub fn from_registry_path_with_cloner(path: PathBuf, cloner: Arc<dyn DirectoryCloner>) -> Self {
        Self::from_registry_and_cloner(WorkspaceRegistry::from_path(path), cloner)
    }

    fn from_registry_and_cloner(registry: WorkspaceRegistry, cloner: Arc<dyn DirectoryCloner>) -> Self {
        Self { registry, cloner }
    }

    /// Lists every managed workspace sharing `cwd`'s repository, registering
    /// `cwd`'s own repo root as a side effect so it becomes a known move
    /// target for other workspaces.
    pub fn list(&self, cwd: &Path) -> Result<Vec<WorkspaceEntry>, WorkspaceError> {
        let (src_root, repo_key) = resolve_repo(cwd)?;
        self.registry.register(&repo_key, &src_root)?;
        Ok(self
            .registry
            .workspaces_for(&repo_key)?
            .into_iter()
            .map(|record| WorkspaceEntry { is_current: record.path == src_root, path: record.path })
            .collect())
    }

    /// Moves the uncommitted changes from `cwd`'s workspace to `target`
    /// (cloning a new sibling directory when the target is new) and
    /// returns the directory within the target corresponding to `cwd`.
    pub fn move_to(&self, cwd: &Path, target: &WorkspaceMoveTarget) -> Result<PathBuf, WorkspaceError> {
        let (src_root, repo_key) = resolve_repo(cwd)?;

        let dst_root = match target {
            WorkspaceMoveTarget::Existing { path } => {
                if !path.exists() {
                    return Err(WorkspaceError::TargetMissing(path.clone()));
                }
                let (dst_root, dst_key) = resolve_repo(path)?;
                if dst_key != repo_key {
                    return Err(WorkspaceError::DifferentRepository(path.clone()));
                }
                if dst_root == src_root {
                    return Err(WorkspaceError::AlreadyInWorkspace);
                }
                if git::head_commit_hash(&src_root)? != git::head_commit_hash(&dst_root)? {
                    return Err(WorkspaceError::DifferentHead(path.clone()));
                }
                dst_root
            }
            WorkspaceMoveTarget::New { name } => sibling_workspace_path(&src_root, name)?,
        };

        move_changes(&src_root, &dst_root, self.cloner.as_ref())?;
        self.registry.register(&repo_key, &src_root)?;
        self.registry.register(&repo_key, &dst_root)?;
        let canonical_cwd = cwd.canonicalize().unwrap_or_else(|_| cwd.to_path_buf());
        let relocated = canonical_cwd
            .strip_prefix(&src_root)
            .ok()
            .filter(|relative| !relative.as_os_str().is_empty())
            .map(|relative| dst_root.join(relative));
        Ok(relocated.filter(|candidate| candidate.exists()).unwrap_or(dst_root))
    }
}

impl WorkspaceError {
    pub fn is_invalid_input(&self) -> bool {
        match self {
            Self::NotARepository { .. }
            | Self::RepositoryIdentity(_)
            | Self::TargetMissing(_)
            | Self::DifferentRepository(_)
            | Self::DifferentHead(_)
            | Self::AlreadyInWorkspace
            | Self::EmptyName
            | Self::NameSeparator(_)
            | Self::ReservedName(_)
            | Self::NoParent(_)
            | Self::TargetPathExists(_)
            | Self::TargetDirty(_) => true,
            Self::Registry(_) | Self::Clone(_) | Self::Git(_) | Self::Io(_) => false,
        }
    }
}

fn resolve_repo(cwd: &Path) -> Result<(PathBuf, String), WorkspaceError> {
    let root =
        git::repo_root(cwd).map_err(|source| WorkspaceError::NotARepository { path: cwd.to_path_buf(), source })?;
    let root = root.canonicalize().unwrap_or(root);
    let key = git::root_commit_hash(&root).map_err(WorkspaceError::RepositoryIdentity)?;
    Ok((root, key))
}

fn sibling_workspace_path(src_root: &Path, name: &str) -> Result<PathBuf, WorkspaceError> {
    if name.is_empty() {
        return Err(WorkspaceError::EmptyName);
    }
    if name.chars().any(std::path::is_separator) {
        return Err(WorkspaceError::NameSeparator(name.to_string()));
    }
    if name == "." || name == ".." {
        return Err(WorkspaceError::ReservedName(name.to_string()));
    }

    let parent = src_root.parent().ok_or_else(|| WorkspaceError::NoParent(src_root.to_path_buf()))?;
    let dst = parent.join(name);
    if dst.exists() {
        return Err(WorkspaceError::TargetPathExists(dst));
    }
    Ok(dst)
}

fn move_changes(src: &Path, dst: &Path, cloner: &dyn DirectoryCloner) -> Result<(), WorkspaceError> {
    if dst.exists() {
        if !git::is_clean(dst)? {
            return Err(WorkspaceError::TargetDirty(dst.to_path_buf()));
        }

        let untracked = git::untracked_files(src)?;
        for rel in &untracked {
            let dst_file = dst.join(rel);
            if dst_file.exists() {
                return Err(WorkspaceError::TargetPathExists(dst_file));
            }
        }

        let patch = git::diff_head(src)?;
        git::apply_patch(dst, &patch)?;
        git::copy_untracked_files(src, dst, &untracked)?;
    } else {
        cloner.clone_dir(src, dst)?;
    }

    git::reset_clean(src)?;
    Ok(())
}

impl DirectoryCloner for PlatformDirectoryCloner {
    fn clone_dir(&self, src: &Path, dst: &Path) -> Result<(), CloneError> {
        platform_clone_dir(src, dst)
    }
}

#[cfg(target_os = "macos")]
fn platform_clone_dir(src: &Path, dst: &Path) -> Result<(), CloneError> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;

    let src_c = CString::new(src.as_os_str().as_bytes()).map_err(|_| CloneError::InvalidPath(src.to_path_buf()))?;

    let dst_c = CString::new(dst.as_os_str().as_bytes()).map_err(|_| CloneError::InvalidPath(dst.to_path_buf()))?;

    let ret = unsafe { libc::clonefile(src_c.as_ptr(), dst_c.as_ptr(), 0) };
    if ret != 0 {
        return Err(CloneError::Clone {
            src: src.to_path_buf(),
            dst: dst.to_path_buf(),
            source: io::Error::last_os_error(),
        });
    }

    Ok(())
}

#[cfg(not(target_os = "macos"))]
fn platform_clone_dir(src: &Path, dst: &Path) -> Result<(), CloneError> {
    use std::process::Command;

    let output = Command::new("cp")
        .arg("-a")
        .arg("--reflink=auto")
        .arg(src)
        .arg(dst)
        .output()
        .map_err(|e| CloneError::Clone { src: src.to_path_buf(), dst: dst.to_path_buf(), source: e })?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let detail = stderr.trim();
        let message = if detail.is_empty() {
            "failed to clone workspace directory".to_string()
        } else {
            format!("failed to clone workspace directory: {detail}")
        };
        return Err(CloneError::Clone {
            src: src.to_path_buf(),
            dst: dst.to_path_buf(),
            source: io::Error::other(message),
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workspace::testing::{StdCopyCloner, init_repo};
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn move_to_rejects_invalid_new_names() {
        let tmp = TempDir::new().unwrap();
        let repo = init_repo(tmp.path(), "repo");
        fs::create_dir_all(tmp.path().join("taken")).unwrap();
        let manager = WorkspaceManager::from_registry_path_with_cloner(
            tmp.path().join("workspaces.json"),
            Arc::new(StdCopyCloner),
        );

        let move_to = |name: &str| manager.move_to(&repo, &WorkspaceMoveTarget::New { name: name.to_string() });

        assert!(matches!(move_to(""), Err(WorkspaceError::EmptyName)));
        assert!(matches!(move_to("a/b"), Err(WorkspaceError::NameSeparator(_))));
        assert!(matches!(move_to("."), Err(WorkspaceError::ReservedName(_))));
        assert!(matches!(move_to(".."), Err(WorkspaceError::ReservedName(_))));
        assert!(matches!(move_to("taken"), Err(WorkspaceError::TargetPathExists(_))));
    }

    #[test]
    fn list_registers_repo_root_and_marks_it_current() {
        let tmp = TempDir::new().unwrap();
        let repo = init_repo(tmp.path(), "repo");
        let manager = WorkspaceManager::from_registry_path_with_cloner(
            tmp.path().join("workspaces.json"),
            Arc::new(StdCopyCloner),
        );

        let workspaces = manager.list(&repo).unwrap();

        assert_eq!(workspaces.len(), 1);
        assert!(workspaces[0].is_current);
        assert_eq!(workspaces[0].path, repo.canonicalize().unwrap());
    }
}
