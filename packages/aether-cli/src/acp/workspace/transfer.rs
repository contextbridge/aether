use super::git::{GitCommandError, repo_root, run_git, run_git_bytes, run_git_with_stdin};
use std::io;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TransferOutcome {
    Moved,
    NothingToMove,
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum WorkspaceTransferError {
    #[error("destination is not a git repository: {0}")]
    NotAGitRepo(PathBuf),
    #[error("destination has uncommitted changes: {0}")]
    DestinationDirty(PathBuf),
    #[error("patch does not apply: {0}")]
    PatchDoesNotApply(String),
    #[error("source repository has no commits; commit at least once before forking: {0}")]
    UnbornHead(PathBuf),
    #[error("git failed: {0}")]
    Git(String),
    #[error(transparent)]
    Io(#[from] io::Error),
}

/// Move tracked and untracked, non-ignored working tree changes from `src` to `dest`.
pub(crate) async fn move_working_changes(src: &Path, dest: &Path) -> Result<TransferOutcome, WorkspaceTransferError> {
    let Some(src_root) = repo_root(src).await else {
        return Ok(TransferOutcome::NothingToMove);
    };
    let Some(dest_root) = repo_root(dest).await else {
        return Err(WorkspaceTransferError::NotAGitRepo(dest.to_path_buf()));
    };

    if git(&src_root, &["status", "--porcelain", "-z"]).await?.is_empty() {
        return Ok(TransferOutcome::NothingToMove);
    }
    if run_git(&src_root, &["rev-parse", "--verify", "HEAD"]).await.is_err() {
        return Err(WorkspaceTransferError::UnbornHead(src_root));
    }
    if !git(&dest_root, &["status", "--porcelain", "-z"]).await?.is_empty() {
        return Err(WorkspaceTransferError::DestinationDirty(dest_root));
    }

    let copied = copy_untracked_files(&src_root, &dest_root).await?;
    let diff = run_git_bytes(&src_root, &["diff", "--binary", "--find-renames", "HEAD"])
        .await
        .map_err(|e| WorkspaceTransferError::Git(git_error_message(e)))?;
    if !diff.is_empty()
        && let Err(e) = run_git_with_stdin(&dest_root, &["apply", "--binary", "--whitespace=nowarn"], &diff).await
    {
        rollback_copied(&dest_root, &copied).await;
        return Err(WorkspaceTransferError::PatchDoesNotApply(git_error_message(e)));
    }

    if let Err(e) = clean_working_changes(&src_root).await {
        tracing::warn!("Failed to clean source workspace after transfer: {e}");
    }
    Ok(TransferOutcome::Moved)
}

pub(crate) async fn clean_working_changes(src: &Path) -> Result<(), WorkspaceTransferError> {
    let Some(src_root) = repo_root(src).await else {
        return Ok(());
    };
    if run_git(&src_root, &["rev-parse", "--verify", "HEAD"]).await.is_err() {
        return Ok(());
    }

    git(&src_root, &["restore", "--staged", "--worktree", "--", ":/"]).await?;
    let untracked = untracked_files(&src_root).await?;
    for rel in untracked {
        let path = src_root.join(rel);
        match tokio::fs::symlink_metadata(&path).await {
            Ok(meta) if meta.is_dir() => tokio::fs::remove_dir_all(&path).await?,
            Ok(_) => tokio::fs::remove_file(&path).await?,
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.into()),
        }
    }
    Ok(())
}

async fn copy_untracked_files(src_root: &Path, dest_root: &Path) -> Result<Vec<PathBuf>, WorkspaceTransferError> {
    let mut copied = Vec::new();
    for rel in untracked_files(src_root).await? {
        let src = src_root.join(&rel);
        let dest = dest_root.join(&rel);
        if let Some(parent) = dest.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        tokio::fs::copy(&src, &dest).await?;
        copied.push(rel);
    }
    Ok(copied)
}

async fn untracked_files(root: &Path) -> Result<Vec<PathBuf>, WorkspaceTransferError> {
    Ok(git(root, &["ls-files", "--others", "--exclude-standard", "-z"])
        .await?
        .split('\0')
        .filter(|rel| !rel.is_empty())
        .map(PathBuf::from)
        .collect())
}

async fn rollback_copied(dest_root: &Path, copied: &[PathBuf]) {
    for rel in copied.iter().rev() {
        let path = dest_root.join(rel);
        let _ = tokio::fs::remove_file(&path).await;
        remove_empty_parents(dest_root, path.parent()).await;
    }
}

async fn remove_empty_parents(dest_root: &Path, mut current: Option<&Path>) {
    while let Some(dir) = current {
        if dir == dest_root {
            break;
        }
        if tokio::fs::remove_dir(dir).await.is_err() {
            break;
        }
        current = dir.parent();
    }
}

async fn git(cwd: &Path, args: &[&str]) -> Result<String, WorkspaceTransferError> {
    run_git(cwd, args).await.map_err(|e| WorkspaceTransferError::Git(git_error_message(e)))
}

fn git_error_message(error: GitCommandError) -> String {
    match error {
        GitCommandError::Failed { stderr, .. } => stderr,
        GitCommandError::Io(e) => e.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn init_git_repo(path: &Path) {
        std::fs::create_dir_all(path).unwrap();
        std::process::Command::new("git").arg("-C").arg(path).args(["init"]).output().unwrap();
        std::process::Command::new("git")
            .arg("-C")
            .arg(path)
            .args(["config", "user.email", "test@example.com"])
            .output()
            .unwrap();
        std::process::Command::new("git")
            .arg("-C")
            .arg(path)
            .args(["config", "user.name", "Test User"])
            .output()
            .unwrap();
    }

    fn commit_all(path: &Path, message: &str) {
        std::process::Command::new("git").arg("-C").arg(path).args(["add", "."]).output().unwrap();
        std::process::Command::new("git").arg("-C").arg(path).args(["commit", "-m", message]).output().unwrap();
    }

    fn status(path: &Path) -> String {
        String::from_utf8(
            std::process::Command::new("git")
                .arg("-C")
                .arg(path)
                .args(["status", "--porcelain"])
                .output()
                .unwrap()
                .stdout,
        )
        .unwrap()
    }

    fn paired_repos() -> (TempDir, PathBuf, PathBuf) {
        let dir = TempDir::new().unwrap();
        let src = dir.path().join("src");
        let dest = dir.path().join("dest");
        init_git_repo(&src);
        std::fs::write(src.join("tracked.txt"), "base\n").unwrap();
        std::fs::write(src.join("delete-me.txt"), "delete\n").unwrap();
        commit_all(&src, "initial");
        std::process::Command::new("git").arg("clone").arg(&src).arg(&dest).output().unwrap();
        (dir, src, dest)
    }

    #[tokio::test]
    async fn moves_tracked_modification_and_untracked_file() {
        let (_dir, src, dest) = paired_repos();
        std::fs::write(src.join("tracked.txt"), "changed\n").unwrap();
        std::fs::write(src.join("new.txt"), "new\n").unwrap();
        assert_eq!(move_working_changes(&src, &dest).await.unwrap(), TransferOutcome::Moved);
        assert_eq!(std::fs::read_to_string(dest.join("tracked.txt")).unwrap(), "changed\n");
        assert_eq!(std::fs::read_to_string(dest.join("new.txt")).unwrap(), "new\n");
        assert!(!src.join("new.txt").exists());
        assert!(status(&src).is_empty());
    }

    #[tokio::test]
    async fn moves_binary_tracked_file_without_corrupting_patch() {
        let (_dir, src, dest) = paired_repos();
        let bytes = vec![0, 159, 146, 150, 255, 0, 1, 2];
        std::fs::write(src.join("tracked.txt"), &bytes).unwrap();
        assert_eq!(move_working_changes(&src, &dest).await.unwrap(), TransferOutcome::Moved);
        assert_eq!(std::fs::read(dest.join("tracked.txt")).unwrap(), bytes);
        assert!(status(&src).is_empty());
    }

    #[tokio::test]
    async fn clean_source_returns_nothing_to_move() {
        let (_dir, src, dest) = paired_repos();
        assert_eq!(move_working_changes(&src, &dest).await.unwrap(), TransferOutcome::NothingToMove);
    }

    #[tokio::test]
    async fn non_git_source_returns_nothing_to_move() {
        let dir = TempDir::new().unwrap();
        let src = dir.path().join("src");
        let dest = dir.path().join("dest");
        std::fs::create_dir_all(&src).unwrap();
        init_git_repo(&dest);
        std::fs::write(dest.join("file.txt"), "base").unwrap();
        commit_all(&dest, "initial");
        assert_eq!(move_working_changes(&src, &dest).await.unwrap(), TransferOutcome::NothingToMove);
    }

    #[tokio::test]
    async fn dirty_destination_is_rejected_without_touching_trees() {
        let (_dir, src, dest) = paired_repos();
        std::fs::write(src.join("new.txt"), "source\n").unwrap();
        std::fs::write(dest.join("dest.txt"), "dirty\n").unwrap();
        let result = move_working_changes(&src, &dest).await;
        assert!(matches!(result, Err(WorkspaceTransferError::DestinationDirty(_))));
        assert!(src.join("new.txt").exists());
        assert!(dest.join("dest.txt").exists());
    }

    #[tokio::test]
    async fn divergent_destination_rolls_back_copied_untracked_files() {
        let (_dir, src, dest) = paired_repos();
        std::fs::write(dest.join("tracked.txt"), "dest change\n").unwrap();
        commit_all(&dest, "diverge");
        std::fs::write(src.join("tracked.txt"), "src change\n").unwrap();
        std::fs::write(src.join("new.txt"), "new\n").unwrap();
        let result = move_working_changes(&src, &dest).await;
        assert!(matches!(result, Err(WorkspaceTransferError::PatchDoesNotApply(_))));
        assert!(!dest.join("new.txt").exists());
        assert!(src.join("new.txt").exists());
    }

    #[tokio::test]
    async fn unborn_head_is_rejected() {
        let dir = TempDir::new().unwrap();
        let src = dir.path().join("src");
        let dest = dir.path().join("dest");
        init_git_repo(&src);
        std::fs::write(src.join("new.txt"), "new\n").unwrap();
        init_git_repo(&dest);
        std::fs::write(dest.join("file.txt"), "base\n").unwrap();
        commit_all(&dest, "initial");
        assert!(matches!(move_working_changes(&src, &dest).await, Err(WorkspaceTransferError::UnbornHead(_))));
    }

    #[tokio::test]
    async fn tracked_deletion_transfers_and_source_is_restored() {
        let (_dir, src, dest) = paired_repos();
        std::fs::remove_file(src.join("delete-me.txt")).unwrap();
        move_working_changes(&src, &dest).await.unwrap();
        assert!(!dest.join("delete-me.txt").exists());
        assert!(src.join("delete-me.txt").exists());
        assert!(status(&src).is_empty());
    }
}
