use std::path::{Path, PathBuf};
use std::process::Command;
use thiserror::Error;

/// Represents a git repository used for evaluation purposes
pub struct GitRepo {
    path: PathBuf,
}

impl GitRepo {
    /// Create a `GitRepo` instance from an existing repository path
    pub fn from_path(path: &Path) -> Self {
        GitRepo { path: path.to_path_buf() }
    }

    /// Clone a repository to the specified destination
    ///
    /// Uses a blobless partial clone (--filter=blob:none) with --no-checkout for efficiency.
    /// This downloads commit history and tree structures but defers downloading file blobs
    /// until they're needed (e.g., during checkout or diff operations).
    ///
    /// This significantly reduces clone time and disk space usage, especially for large repos,
    /// while still allowing full git operations. Blobs are automatically fetched on-demand.
    #[tracing::instrument(skip(dest))]
    pub fn clone(url: &str, dest: &Path) -> Result<Self, GitRepoError> {
        tracing::debug!("Cloning repository from {} (blobless clone)", url);
        let output = Command::new("git")
            .arg("clone")
            .arg("--no-checkout")
            .arg("--filter=blob:none")
            .arg(url)
            .arg(dest)
            .output()
            .map_err(|e| GitRepoError::CommandFailed(format!("Failed to execute git clone: {e}")))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(GitRepoError::CloneFailed(stderr.to_string()));
        }

        Ok(GitRepo { path: dest.to_path_buf() })
    }

    /// Checkout a specific commit, branch, or tag
    #[tracing::instrument(skip(self))]
    pub fn checkout(&self, reference: &str) -> Result<(), GitRepoError> {
        tracing::debug!("Checking out ref: {}", reference);
        let output = Command::new("git")
            .arg("-C")
            .arg(&self.path)
            .arg("checkout")
            .arg(reference)
            .output()
            .map_err(|e| GitRepoError::CommandFailed(format!("Failed to execute git checkout: {e}")))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(GitRepoError::CheckoutFailed { reference: reference.to_string(), reason: stderr.to_string() });
        }

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
        let mut cmd = Command::new("git");
        cmd.arg("-C").arg(&self.path).arg("diff");

        if let Some(to) = to_commit {
            tracing::debug!("Getting diff from {} to {}", from_commit, to);
            cmd.arg(format!("{from_commit}..{to}"));
        } else {
            tracing::debug!("Getting diff from {} to working directory", from_commit);
            cmd.arg(from_commit);
        }

        let output =
            cmd.output().map_err(|e| GitRepoError::CommandFailed(format!("Failed to execute git diff: {e}")))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(GitRepoError::DiffFailed {
                from: from_commit.to_string(),
                to: to_commit.unwrap_or("working directory").to_string(),
                reason: stderr.to_string(),
            });
        }

        let diff = String::from_utf8_lossy(&output.stdout).to_string();
        Ok(diff)
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

    /// Get the path to the repository
    pub fn path(&self) -> &Path {
        &self.path
    }
}

#[derive(Debug, Error)]
pub enum GitRepoError {
    #[error("Git command failed: {0}")]
    CommandFailed(String),

    #[error("Failed to clone repository: {0}")]
    CloneFailed(String),

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
        // Create a temporary directory for the test
        let temp_dir = tempfile::tempdir().unwrap();
        let repo_path = temp_dir.path();

        // Initialize a git repo
        Command::new("git").args(["init"]).current_dir(repo_path).output().unwrap();

        // Configure git user for commits
        Command::new("git").args(["config", "user.email", "test@example.com"]).current_dir(repo_path).output().unwrap();
        Command::new("git").args(["config", "user.name", "Test User"]).current_dir(repo_path).output().unwrap();

        // Create initial file and commit
        fs::write(repo_path.join("test.txt"), "initial content\n").unwrap();
        Command::new("git").args(["add", "test.txt"]).current_dir(repo_path).output().unwrap();
        Command::new("git").args(["commit", "-m", "Initial commit"]).current_dir(repo_path).output().unwrap();

        // Get the first commit hash
        let first_commit = String::from_utf8(
            Command::new("git").args(["rev-parse", "HEAD"]).current_dir(repo_path).output().unwrap().stdout,
        )
        .unwrap()
        .trim()
        .to_string();

        // Make changes and create second commit
        fs::write(repo_path.join("test.txt"), "modified content\n").unwrap();
        fs::write(repo_path.join("new.txt"), "new file\n").unwrap();
        Command::new("git").args(["add", "."]).current_dir(repo_path).output().unwrap();
        Command::new("git").args(["commit", "-m", "Second commit"]).current_dir(repo_path).output().unwrap();

        // Get the second commit hash
        let second_commit = String::from_utf8(
            Command::new("git").args(["rev-parse", "HEAD"]).current_dir(repo_path).output().unwrap().stdout,
        )
        .unwrap()
        .trim()
        .to_string();

        // Test the diff function
        let git_repo = GitRepo::from_path(repo_path);
        let diff = git_repo.diff(&first_commit, &second_commit).unwrap();

        // Verify the diff contains expected changes
        assert!(diff.contains("test.txt"), "Diff should mention test.txt");
        assert!(diff.contains("new.txt"), "Diff should mention new.txt");
        assert!(
            diff.contains("modified content") || diff.contains("+modified content"),
            "Diff should show modified content"
        );
    }

    #[test]
    fn test_unified_diff_function() {
        // Create a temporary directory for the test
        let temp_dir = tempfile::tempdir().unwrap();
        let repo_path = temp_dir.path();

        // Initialize a git repo
        Command::new("git").args(["init"]).current_dir(repo_path).output().unwrap();

        // Configure git user for commits
        Command::new("git").args(["config", "user.email", "test@example.com"]).current_dir(repo_path).output().unwrap();
        Command::new("git").args(["config", "user.name", "Test User"]).current_dir(repo_path).output().unwrap();

        // Create initial file and commit
        fs::write(repo_path.join("test.txt"), "initial content\n").unwrap();
        Command::new("git").args(["add", "test.txt"]).current_dir(repo_path).output().unwrap();
        Command::new("git").args(["commit", "-m", "Initial commit"]).current_dir(repo_path).output().unwrap();

        let first_commit = String::from_utf8(
            Command::new("git").args(["rev-parse", "HEAD"]).current_dir(repo_path).output().unwrap().stdout,
        )
        .unwrap()
        .trim()
        .to_string();

        // Make changes and create second commit
        fs::write(repo_path.join("test.txt"), "modified content\n").unwrap();
        Command::new("git").args(["add", "test.txt"]).current_dir(repo_path).output().unwrap();
        Command::new("git").args(["commit", "-m", "Second commit"]).current_dir(repo_path).output().unwrap();

        let second_commit = String::from_utf8(
            Command::new("git").args(["rev-parse", "HEAD"]).current_dir(repo_path).output().unwrap().stdout,
        )
        .unwrap()
        .trim()
        .to_string();

        // Make unstaged changes
        fs::write(repo_path.join("test.txt"), "unstaged content\n").unwrap();

        let git_repo = GitRepo::from_path(repo_path);

        // Test: diff between two specific commits
        let diff = git_repo.diff_range(&first_commit, Some(&second_commit)).unwrap();
        assert!(diff.contains("modified content") || diff.contains("+modified content"));

        // Test: diff unstaged changes (from HEAD to working directory)
        let unstaged_diff = git_repo.diff_range("HEAD", None).unwrap();
        assert!(unstaged_diff.contains("unstaged content") || unstaged_diff.contains("+unstaged content"));

        // Test: diff from specific commit to working directory
        let from_commit_diff = git_repo.diff_range(&first_commit, None).unwrap();
        assert!(from_commit_diff.contains("unstaged content") || from_commit_diff.contains("+unstaged content"));
    }

    #[test]
    fn test_blobless_clone_and_checkout() {
        // Create a test repository to clone from
        let source_dir = tempfile::tempdir().unwrap();
        let source_path = source_dir.path();

        // Initialize source repo
        Command::new("git").args(["init"]).current_dir(source_path).output().unwrap();

        // Configure git user
        Command::new("git")
            .args(["config", "user.email", "test@example.com"])
            .current_dir(source_path)
            .output()
            .unwrap();
        Command::new("git").args(["config", "user.name", "Test User"]).current_dir(source_path).output().unwrap();

        // Create a file and commit
        fs::write(source_path.join("test.txt"), "initial content\n").unwrap();
        Command::new("git").args(["add", "test.txt"]).current_dir(source_path).output().unwrap();
        Command::new("git").args(["commit", "-m", "Initial commit"]).current_dir(source_path).output().unwrap();

        let first_commit = String::from_utf8(
            Command::new("git").args(["rev-parse", "HEAD"]).current_dir(source_path).output().unwrap().stdout,
        )
        .unwrap()
        .trim()
        .to_string();

        // Create another commit
        fs::write(source_path.join("test.txt"), "modified content\n").unwrap();
        Command::new("git").args(["add", "test.txt"]).current_dir(source_path).output().unwrap();
        Command::new("git").args(["commit", "-m", "Second commit"]).current_dir(source_path).output().unwrap();

        let second_commit = String::from_utf8(
            Command::new("git").args(["rev-parse", "HEAD"]).current_dir(source_path).output().unwrap().stdout,
        )
        .unwrap()
        .trim()
        .to_string();

        // Clone the repo (blobless clone with --no-checkout)
        let clone_dir = tempfile::tempdir().unwrap();
        let repo = GitRepo::clone(source_path.to_str().unwrap(), clone_dir.path()).unwrap();

        // Verify working directory is empty (no files checked out)
        let entries: Vec<_> = fs::read_dir(clone_dir.path())
            .unwrap()
            .filter_map(std::result::Result::ok)
            .filter(|e| e.file_name() != ".git")
            .collect();
        assert_eq!(entries.len(), 0, "Working directory should be empty after blobless clone");

        // Checkout the first commit
        repo.checkout(&first_commit).unwrap();

        // Verify file is now present with correct content
        let content = fs::read_to_string(clone_dir.path().join("test.txt")).unwrap();
        assert_eq!(content, "initial content\n");

        // Verify we can get diffs between commits (tests lazy blob fetching)
        let diff = repo.diff(&first_commit, &second_commit).unwrap();
        assert!(diff.contains("modified content") || diff.contains("+modified content"));
    }
}
