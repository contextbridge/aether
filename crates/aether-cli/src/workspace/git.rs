use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum GitError {
    #[error("failed to spawn `git {args}`: {source}")]
    Spawn { args: String, source: std::io::Error },
    #[error("`git {args}` failed: {stderr}")]
    Failed { args: String, stderr: String },
    #[error("filesystem error: {0}")]
    Io(#[from] std::io::Error),
}

/// Resolves the root of the git working tree containing `path`.
pub fn repo_root(path: &Path) -> Result<PathBuf, GitError> {
    let output = git(path, &["rev-parse", "--show-toplevel"])?;
    Ok(PathBuf::from(String::from_utf8_lossy(&output.stdout).trim()))
}

/// Returns the hash of the repository's first root commit, a stable identity
/// shared by every clone and copy of the same repository.
pub fn root_commit_hash(repo: &Path) -> Result<String, GitError> {
    let output = git(repo, &["rev-list", "--max-parents=0", "HEAD"])?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    Ok(stdout.lines().next().unwrap_or_default().trim().to_string())
}

pub fn head_commit_hash(repo: &Path) -> Result<String, GitError> {
    let output = git(repo, &["rev-parse", "HEAD"])?;
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

pub fn is_clean(repo: &Path) -> Result<bool, GitError> {
    Ok(git(repo, &["status", "--porcelain=v1", "-z"])?.stdout.is_empty())
}

pub fn untracked_files(repo: &Path) -> Result<Vec<PathBuf>, GitError> {
    use std::os::unix::ffi::OsStrExt;

    Ok(git(repo, &["ls-files", "--others", "--exclude-standard", "-z"])?
        .stdout
        .split(|b| *b == 0)
        .filter(|chunk| !chunk.is_empty())
        .map(|chunk| PathBuf::from(std::ffi::OsStr::from_bytes(chunk)))
        .collect())
}

pub fn diff_head(repo: &Path) -> Result<Vec<u8>, GitError> {
    Ok(git(repo, &["diff", "HEAD", "--binary", "--no-color"])?.stdout)
}

pub fn apply_patch(repo: &Path, patch: &[u8]) -> Result<(), GitError> {
    if patch.is_empty() {
        return Ok(());
    }

    git_with_stdin(repo, &["apply", "--binary", "--whitespace=nowarn"], patch)?;
    Ok(())
}

pub fn copy_untracked_files(src: &Path, dst: &Path, files: &[PathBuf]) -> Result<(), GitError> {
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
    git(repo, &["reset", "--hard", "HEAD"])?;
    git(repo, &["clean", "-fd"])?;
    Ok(())
}

fn git(repo: &Path, args: &[&str]) -> Result<std::process::Output, GitError> {
    let output = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .output()
        .map_err(|e| GitError::Spawn { args: args.join(" "), source: e })?;

    git_output(args, output)
}

fn git_with_stdin(repo: &Path, args: &[&str], stdin: &[u8]) -> Result<Output, GitError> {
    let args_string = args.join(" ");
    let mut child = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| GitError::Spawn { args: args_string.clone(), source: e })?;

    child
        .stdin
        .take()
        .expect("stdin was piped")
        .write_all(stdin)
        .map_err(|e| GitError::Spawn { args: args_string.clone(), source: e })?;

    let output = child.wait_with_output().map_err(|e| GitError::Spawn { args: args_string, source: e })?;
    git_output(args, output)
}

fn git_output(args: &[&str], output: std::process::Output) -> Result<Output, GitError> {
    if output.status.success() {
        Ok(output)
    } else {
        Err(GitError::Failed {
            args: args.join(" "),
            stderr: String::from_utf8_lossy(&output.stderr).trim().to_string(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workspace::testing::{clone_repo, git, init_repo};
    use std::fs;
    use tempfile::TempDir;

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
        let clone = clone_repo(&repo, tmp.path().join("clone"));
        fs::write(clone.join("extra.txt"), "diverged\n").unwrap();
        git(&clone, &["add", "."]);
        git(&clone, &["commit", "-m", "diverge"]);

        let original = root_commit_hash(&repo).unwrap();
        assert!(!original.is_empty());
        assert_eq!(root_commit_hash(&clone).unwrap(), original);
    }

    #[test]
    fn root_commit_hash_differs_between_unrelated_repos() {
        let tmp = TempDir::new().unwrap();
        let a = init_repo(tmp.path(), "a");
        let b = init_repo(tmp.path(), "b");
        assert_ne!(root_commit_hash(&a).unwrap(), root_commit_hash(&b).unwrap());
    }
}
