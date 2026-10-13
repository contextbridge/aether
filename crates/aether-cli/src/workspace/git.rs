use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use utils::git::{GitError, git, git_with_stdin};

pub fn current_ref(repo: &Path) -> Option<String> {
    git_text(repo, &["branch", "--show-current"])
        .ok()
        .filter(|name| !name.is_empty())
        .or_else(|| git_text(repo, &["rev-parse", "--short", "HEAD"]).ok())
}

/// Resolves the root of the git working tree containing `path`.
pub fn repo_root(path: &Path) -> Result<PathBuf, GitError> {
    git_text(path, &["rev-parse", "--show-toplevel"]).map(PathBuf::from)
}

/// Returns the hash of the repository's first root commit, a stable identity
/// shared by every clone and copy of the same repository.
pub fn root_commit_hash(repo: &Path) -> Result<String, GitError> {
    Ok(git_text(repo, &["rev-list", "--max-parents=0", "HEAD"])?.lines().next().unwrap_or_default().to_string())
}

pub fn head_commit_hash(repo: &Path) -> Result<String, GitError> {
    git_text(repo, &["rev-parse", "HEAD"])
}

pub fn is_clean(repo: &Path) -> Result<bool, GitError> {
    Ok(git(Some(repo), &["status", "--porcelain=v1", "-z"])?.is_empty())
}

pub fn untracked_files(repo: &Path) -> Result<Vec<PathBuf>, GitError> {
    use std::os::unix::ffi::OsStrExt;

    Ok(git(Some(repo), &["ls-files", "--others", "--exclude-standard", "-z"])?
        .split(|b| *b == 0)
        .filter(|chunk| !chunk.is_empty())
        .map(|chunk| PathBuf::from(std::ffi::OsStr::from_bytes(chunk)))
        .collect())
}

pub fn diff_head(repo: &Path) -> Result<Vec<u8>, GitError> {
    git(Some(repo), &["diff", "HEAD", "--binary", "--no-color"])
}

pub fn apply_patch(repo: &Path, patch: &[u8]) -> Result<(), GitError> {
    if patch.is_empty() {
        return Ok(());
    }

    git_with_stdin(Some(repo), &["apply", "--binary", "--whitespace=nowarn"], patch)?;
    Ok(())
}

pub fn copy_untracked_files(src: &Path, dst: &Path, files: &[PathBuf]) -> io::Result<()> {
    for rel in files {
        let src_file = src.join(rel);
        let dst_file = dst.join(rel);

        if let Some(parent) = dst_file.parent() {
            fs::create_dir_all(parent)?;
        }

        fs::copy(src_file, dst_file)?;
    }

    Ok(())
}

pub fn reset_clean(repo: &Path) -> Result<(), GitError> {
    git(Some(repo), &["reset", "--hard", "HEAD"])?;
    git(Some(repo), &["clean", "-fd"])?;
    Ok(())
}

fn git_text(repo: &Path, args: &[&str]) -> Result<String, GitError> {
    git(Some(repo), args).map(|stdout| String::from_utf8_lossy(&stdout).trim().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workspace::DirectoryCloner;
    use crate::workspace::testing::{StdCopyCloner, init_repo};
    use std::fs;
    use tempfile::TempDir;
    use utils::testing::git_commit;

    #[test]
    fn repo_root_resolves_from_subdirectory() {
        let tmp = TempDir::new().unwrap();
        let repo = init_repo(tmp.path(), "repo");
        let sub = repo.join("nested/dir");
        fs::create_dir_all(&sub).unwrap();
        assert_eq!(repo_root(&sub).unwrap(), repo.canonicalize().unwrap());
    }

    #[test]
    fn root_commit_hash_is_stable_across_clones() {
        let tmp = TempDir::new().unwrap();
        let repo = init_repo(tmp.path(), "repo");
        let clone = tmp.path().join("clone");
        StdCopyCloner.clone_dir(&repo, &clone).unwrap();
        fs::write(clone.join("extra.txt"), "diverged\n").unwrap();
        git_commit(&clone, "diverge");

        let original = root_commit_hash(&repo).unwrap();
        assert_ne!(original, "");
        assert_eq!(root_commit_hash(&clone).unwrap(), original);
    }

    #[test]
    fn root_commit_hash_differs_between_unrelated_repos() {
        let tmp = TempDir::new().unwrap();
        let a = init_repo(tmp.path(), "a");
        let b = init_repo(tmp.path(), "b");
        assert_ne!(root_commit_hash(&a).unwrap(), root_commit_hash(&b).unwrap());
    }

    #[test]
    fn current_ref_reports_a_branch_and_none_outside_a_repository() {
        let tmp = TempDir::new().unwrap();
        let repo = init_repo(tmp.path(), "repo");
        assert!(current_ref(&repo).is_some());

        let outside = TempDir::new().unwrap();
        assert_eq!(current_ref(outside.path()), None);
    }
}
