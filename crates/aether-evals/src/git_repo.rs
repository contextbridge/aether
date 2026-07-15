use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::Command;
use thiserror::Error;

/// Controls how much of a repository's object set is downloaded during [`GitRepo::clone`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CloneMode {
    /// Download the full object set so the clone is self-contained
    /// (e.g. so it can be bundled, or because `source` is a bundle
    /// and not a promisor remote).
    Full,
    /// Defer downloading file blobs until they are needed via a partial
    /// clone (`--filter=blob:none`) -- cheaper for large repos.
    Blobless,
}

/// Represents a git repository used for evaluation purposes
pub struct GitRepo {
    path: PathBuf,
}

impl GitRepo {
    /// Create a `GitRepo` instance from an existing repository path
    pub fn from_path(path: &Path) -> Self {
        GitRepo { path: path.to_path_buf() }
    }

    /// Initialize an empty repository at `dest`.
    #[tracing::instrument(skip(dest))]
    pub fn init(dest: &Path) -> Result<Self, GitRepoError> {
        run_git(None, [OsStr::new("init"), dest.as_os_str()], GitRepoError::InitFailed)?;
        Ok(GitRepo { path: dest.to_path_buf() })
    }

    /// Clone `source` (a URL or a local bundle/path) into `dest` with `--no-checkout`.
    ///
    /// [`CloneMode::Blobless`] defers downloading file blobs until they're needed --
    /// cheaper for large repos. [`CloneMode::Full`] downloads the complete object set so
    /// the clone is self-contained (e.g. so it can be bundled, or because `source` is a
    /// bundle and not a promisor remote).
    #[tracing::instrument(skip(source, dest))]
    pub fn clone(source: impl AsRef<OsStr>, dest: &Path, mode: CloneMode) -> Result<Self, GitRepoError> {
        let mut args = vec![OsStr::new("clone"), OsStr::new("--no-checkout")];
        if mode == CloneMode::Blobless {
            args.push(OsStr::new("--filter=blob:none"));
        }
        args.push(source.as_ref());
        args.push(dest.as_os_str());
        run_git(None, args, GitRepoError::CloneFailed)?;
        Ok(GitRepo { path: dest.to_path_buf() })
    }

    /// Checkout a specific commit, branch, or tag
    #[tracing::instrument(skip(self))]
    pub fn checkout(&self, reference: &str) -> Result<(), GitRepoError> {
        run_git(Some(&self.path), ["checkout", reference], |reason| GitRepoError::CheckoutFailed {
            reference: reference.to_string(),
            reason,
        })?;
        Ok(())
    }

    /// Fetch `revs` from `remote` (a named remote or a URL) into this repository.
    #[tracing::instrument(skip(self))]
    pub fn fetch(&self, remote: &str, revs: &[&str]) -> Result<(), GitRepoError> {
        let mut args = vec!["fetch", remote];
        args.extend_from_slice(revs);
        run_git(Some(&self.path), args, GitRepoError::FetchFailed)?;
        Ok(())
    }

    /// Point a ref at a commit (e.g. to create a branch tip to bundle).
    #[tracing::instrument(skip(self))]
    pub fn update_ref(&self, name: &str, commit: &str) -> Result<(), GitRepoError> {
        run_git(Some(&self.path), ["update-ref", name, commit], GitRepoError::UpdateRefFailed)?;
        Ok(())
    }

    /// Create a self-contained git bundle at `out` containing the given refs and their
    /// reachable objects.
    #[tracing::instrument(skip(self, out))]
    pub fn bundle(&self, revs: &[&str], out: &Path) -> Result<(), GitRepoError> {
        let mut args = vec![OsStr::new("bundle"), OsStr::new("create"), out.as_os_str()];
        args.extend(revs.iter().map(OsStr::new));
        run_git(Some(&self.path), args, GitRepoError::BundleFailed)?;
        Ok(())
    }

    /// Get the diff from a commit to another commit or working directory
    ///
    /// # Arguments
    /// * `from_commit` - Starting commit
    /// * `to_commit` - Ending commit (None means working directory/unstaged changes)
    ///
    /// # Examples
    /// * `diff_range("abc123", Some("def456"))` - diff between two commits
    /// * `diff_range("abc123", None)` - changes from commit to working directory
    /// * `diff_range("HEAD", None)` - unstaged changes (equivalent to `git diff`)
    #[tracing::instrument(skip(self))]
    pub fn diff_range(&self, from_commit: &str, to_commit: Option<&str>) -> Result<String, GitRepoError> {
        let range = match to_commit {
            Some(to) => format!("{from_commit}..{to}"),
            None => from_commit.to_string(),
        };
        run_git(Some(&self.path), ["diff", range.as_str()], |reason| GitRepoError::DiffFailed {
            from: from_commit.to_string(),
            to: to_commit.unwrap_or("working directory").to_string(),
            reason,
        })
    }

    /// Get the diff between two commits
    ///
    /// Returns the output of `git diff from_commit..to_commit`
    pub fn diff(&self, from_commit: &str, to_commit: &str) -> Result<String, GitRepoError> {
        self.diff_range(from_commit, Some(to_commit))
    }

    /// Get the diff of unstaged changes in the working directory
    ///
    /// Returns the output of `git diff` which shows all unstaged changes
    pub fn diff_unstaged(&self) -> Result<String, GitRepoError> {
        self.diff_range("HEAD", None)
    }
}

/// Run `git` (optionally inside `cwd`) and return its stdout.
///
/// Spawn failures are surfaced as [`GitRepoError::CommandFailed`]; a non-zero exit is mapped
/// to a caller-chosen variant via `on_failure`, which receives the captured stderr.
fn run_git<T: IntoIterator<Item = U>, U: AsRef<OsStr>>(
    cwd: Option<&Path>,
    args: T,
    on_failure: impl FnOnce(String) -> GitRepoError,
) -> Result<String, GitRepoError> {
    let mut cmd = Command::new("git");
    if let Some(dir) = cwd {
        cmd.arg("-C").arg(dir);
    }

    let output =
        cmd.args(args).output().map_err(|e| GitRepoError::CommandFailed(format!("Failed to execute git: {e}")))?;

    if !output.status.success() {
        return Err(on_failure(String::from_utf8_lossy(&output.stderr).into_owned()));
    }

    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

#[derive(Debug, Error)]
pub enum GitRepoError {
    #[error("Git command failed: {0}")]
    CommandFailed(String),

    #[error("Failed to initialize repository: {0}")]
    InitFailed(String),

    #[error("Failed to clone repository: {0}")]
    CloneFailed(String),

    #[error("Failed to fetch revisions: {0}")]
    FetchFailed(String),

    #[error("Failed to update ref: {0}")]
    UpdateRefFailed(String),

    #[error("Failed to create git bundle: {0}")]
    BundleFailed(String),

    #[error("Failed to checkout '{reference}': {reason}")]
    CheckoutFailed { reference: String, reason: String },

    #[error("Failed to diff '{from}..{to}': {reason}")]
    DiffFailed { from: String, to: String, reason: String },
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn test_git_diff_between_commits() {
        let temp_dir = tempfile::tempdir().unwrap();
        let repo_path = temp_dir.path();

        Command::new("git").args(["init"]).current_dir(repo_path).output().unwrap();
        Command::new("git").args(["config", "user.email", "test@example.com"]).current_dir(repo_path).output().unwrap();
        Command::new("git").args(["config", "user.name", "Test User"]).current_dir(repo_path).output().unwrap();

        fs::write(repo_path.join("test.txt"), "initial content\n").unwrap();
        Command::new("git").args(["add", "test.txt"]).current_dir(repo_path).output().unwrap();
        Command::new("git").args(["commit", "-m", "Initial commit"]).current_dir(repo_path).output().unwrap();

        let first_commit = String::from_utf8(
            Command::new("git").args(["rev-parse", "HEAD"]).current_dir(repo_path).output().unwrap().stdout,
        )
        .unwrap()
        .trim()
        .to_string();

        fs::write(repo_path.join("test.txt"), "modified content\n").unwrap();
        fs::write(repo_path.join("new.txt"), "new file\n").unwrap();
        Command::new("git").args(["add", "."]).current_dir(repo_path).output().unwrap();
        Command::new("git").args(["commit", "-m", "Second commit"]).current_dir(repo_path).output().unwrap();

        let second_commit = String::from_utf8(
            Command::new("git").args(["rev-parse", "HEAD"]).current_dir(repo_path).output().unwrap().stdout,
        )
        .unwrap()
        .trim()
        .to_string();

        let git_repo = GitRepo::from_path(repo_path);
        let diff = git_repo.diff(&first_commit, &second_commit).unwrap();

        assert!(diff.contains("test.txt"), "Diff should mention test.txt");
        assert!(diff.contains("new.txt"), "Diff should mention new.txt");
        assert!(
            diff.contains("modified content") || diff.contains("+modified content"),
            "Diff should show modified content"
        );
    }

    #[test]
    fn test_unified_diff_function() {
        let temp_dir = tempfile::tempdir().unwrap();
        let repo_path = temp_dir.path();

        Command::new("git").args(["init"]).current_dir(repo_path).output().unwrap();
        Command::new("git").args(["config", "user.email", "test@example.com"]).current_dir(repo_path).output().unwrap();
        Command::new("git").args(["config", "user.name", "Test User"]).current_dir(repo_path).output().unwrap();

        fs::write(repo_path.join("test.txt"), "initial content\n").unwrap();
        Command::new("git").args(["add", "test.txt"]).current_dir(repo_path).output().unwrap();
        Command::new("git").args(["commit", "-m", "Initial commit"]).current_dir(repo_path).output().unwrap();

        let first_commit = String::from_utf8(
            Command::new("git").args(["rev-parse", "HEAD"]).current_dir(repo_path).output().unwrap().stdout,
        )
        .unwrap()
        .trim()
        .to_string();

        fs::write(repo_path.join("test.txt"), "modified content\n").unwrap();
        Command::new("git").args(["add", "test.txt"]).current_dir(repo_path).output().unwrap();
        Command::new("git").args(["commit", "-m", "Second commit"]).current_dir(repo_path).output().unwrap();

        let second_commit = String::from_utf8(
            Command::new("git").args(["rev-parse", "HEAD"]).current_dir(repo_path).output().unwrap().stdout,
        )
        .unwrap()
        .trim()
        .to_string();

        fs::write(repo_path.join("test.txt"), "unstaged content\n").unwrap();

        let git_repo = GitRepo::from_path(repo_path);

        let diff = git_repo.diff_range(&first_commit, Some(&second_commit)).unwrap();
        assert!(diff.contains("modified content") || diff.contains("+modified content"));

        let unstaged_diff = git_repo.diff_range("HEAD", None).unwrap();
        assert!(unstaged_diff.contains("unstaged content") || unstaged_diff.contains("+unstaged content"));

        let from_commit_diff = git_repo.diff_range(&first_commit, None).unwrap();
        assert!(from_commit_diff.contains("unstaged content") || from_commit_diff.contains("+unstaged content"));
    }

    #[test]
    fn test_blobless_clone_and_checkout() {
        let source_dir = tempfile::tempdir().unwrap();
        let source_path = source_dir.path();

        Command::new("git").args(["init"]).current_dir(source_path).output().unwrap();
        Command::new("git")
            .args(["config", "user.email", "test@example.com"])
            .current_dir(source_path)
            .output()
            .unwrap();
        Command::new("git").args(["config", "user.name", "Test User"]).current_dir(source_path).output().unwrap();

        fs::write(source_path.join("test.txt"), "initial content\n").unwrap();
        Command::new("git").args(["add", "test.txt"]).current_dir(source_path).output().unwrap();
        Command::new("git").args(["commit", "-m", "Initial commit"]).current_dir(source_path).output().unwrap();

        let first_commit = String::from_utf8(
            Command::new("git").args(["rev-parse", "HEAD"]).current_dir(source_path).output().unwrap().stdout,
        )
        .unwrap()
        .trim()
        .to_string();

        fs::write(source_path.join("test.txt"), "modified content\n").unwrap();
        Command::new("git").args(["add", "test.txt"]).current_dir(source_path).output().unwrap();
        Command::new("git").args(["commit", "-m", "Second commit"]).current_dir(source_path).output().unwrap();

        let second_commit = String::from_utf8(
            Command::new("git").args(["rev-parse", "HEAD"]).current_dir(source_path).output().unwrap().stdout,
        )
        .unwrap()
        .trim()
        .to_string();

        let clone_dir = tempfile::tempdir().unwrap();
        let repo = GitRepo::clone(source_path.to_str().unwrap(), clone_dir.path(), CloneMode::Blobless).unwrap();

        let entries: Vec<_> = fs::read_dir(clone_dir.path())
            .unwrap()
            .filter_map(std::result::Result::ok)
            .filter(|e| e.file_name() != ".git")
            .collect();
        assert_eq!(entries.len(), 0, "Working directory should be empty after blobless clone");

        repo.checkout(&first_commit).unwrap();

        let content = fs::read_to_string(clone_dir.path().join("test.txt")).unwrap();
        assert_eq!(content, "initial content\n");

        let diff = repo.diff(&first_commit, &second_commit).unwrap();
        assert!(diff.contains("modified content") || diff.contains("+modified content"));
    }
}
