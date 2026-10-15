use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use utils::git::{GitError, git};

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
    pub fn init(dest: &Path) -> Result<Self, GitError> {
        git(None, &[OsStr::new("init"), dest.as_os_str()])?;
        Ok(GitRepo { path: dest.to_path_buf() })
    }

    /// Clone `source` (a URL or a local bundle/path) into `dest` with `--no-checkout`.
    ///
    /// [`CloneMode::Blobless`] defers downloading file blobs until they're needed --
    /// cheaper for large repos. [`CloneMode::Full`] downloads the complete object set so
    /// the clone is self-contained (e.g. so it can be bundled, or because `source` is a
    /// bundle and not a promisor remote).
    #[tracing::instrument(skip(source, dest))]
    pub fn clone(source: impl AsRef<OsStr>, dest: &Path, mode: CloneMode) -> Result<Self, GitError> {
        let mut args = vec![OsStr::new("clone"), OsStr::new("--no-checkout")];
        if mode == CloneMode::Blobless {
            args.push(OsStr::new("--filter=blob:none"));
        }
        args.push(source.as_ref());
        args.push(dest.as_os_str());
        git(None, &args)?;
        Ok(GitRepo { path: dest.to_path_buf() })
    }

    /// Checkout a specific commit, branch, or tag
    #[tracing::instrument(skip(self))]
    pub fn checkout(&self, reference: &str) -> Result<(), GitError> {
        self.git(&["checkout", reference])?;
        Ok(())
    }

    /// Fetch `revs` from `remote` (a named remote or a URL) into this repository.
    #[tracing::instrument(skip(self))]
    pub fn fetch(&self, remote: &str, revs: &[&str]) -> Result<(), GitError> {
        let mut args = vec!["fetch", remote];
        args.extend_from_slice(revs);
        self.git(&args)?;
        Ok(())
    }

    /// Point a ref at a commit (e.g. to create a branch tip to bundle).
    #[tracing::instrument(skip(self))]
    pub fn update_ref(&self, name: &str, commit: &str) -> Result<(), GitError> {
        self.git(&["update-ref", name, commit])?;
        Ok(())
    }

    /// Create a self-contained git bundle at `out` containing the given refs and their
    /// reachable objects.
    #[tracing::instrument(skip(self, out))]
    pub fn bundle(&self, revs: &[&str], out: &Path) -> Result<(), GitError> {
        let mut args = vec![OsStr::new("bundle"), OsStr::new("create"), out.as_os_str()];
        args.extend(revs.iter().map(OsStr::new));
        self.git(&args)?;
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
    pub fn diff_range(&self, from_commit: &str, to_commit: Option<&str>) -> Result<String, GitError> {
        let range = match to_commit {
            Some(to) => format!("{from_commit}..{to}"),
            None => from_commit.to_string(),
        };
        let stdout = self.git(&["diff", range.as_str()])?;
        Ok(String::from_utf8_lossy(&stdout).into_owned())
    }

    /// Get the diff between two commits
    ///
    /// Returns the output of `git diff from_commit..to_commit`
    pub fn diff(&self, from_commit: &str, to_commit: &str) -> Result<String, GitError> {
        self.diff_range(from_commit, Some(to_commit))
    }

    fn git<S: AsRef<OsStr>>(&self, args: &[S]) -> Result<Vec<u8>, GitError> {
        git(Some(&self.path), args)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use utils::testing::TestWorkspace;

    #[test]
    fn diff_range_covers_commit_ranges_and_the_working_tree() {
        let repo = TestWorkspace::new().file("test.txt", "initial content\n").git_init();
        let first_commit = repo.commit("Initial commit");
        repo.write("test.txt", "modified content\n");
        repo.write("new.txt", "new file\n");
        let second_commit = repo.commit("Second commit");
        repo.write("test.txt", "unstaged content\n");
        let git_repo = GitRepo::from_path(repo.root());

        let committed = git_repo.diff(&first_commit, &second_commit).unwrap();
        assert!(committed.contains("+modified content"), "{committed}");
        assert!(committed.contains("+new file"), "{committed}");
        assert!(!committed.contains("unstaged content"), "{committed}");
        assert!(git_repo.diff_range("HEAD", None).unwrap().contains("+unstaged content"));
        assert!(git_repo.diff_range(&first_commit, None).unwrap().contains("+unstaged content"));
    }

    #[test]
    fn blobless_clone_checks_out_and_diffs_lazily_fetched_commits() {
        let source = TestWorkspace::new().file("test.txt", "initial content\n").git_init();
        let first_commit = source.commit("Initial commit");
        source.write("test.txt", "modified content\n");
        let second_commit = source.commit("Second commit");
        let clone = TestWorkspace::new();

        let repo = GitRepo::clone(source.root(), clone.root(), CloneMode::Blobless).unwrap();
        assert!(
            fs::read_dir(clone.root()).unwrap().all(|entry| entry.unwrap().file_name() == ".git"),
            "working directory should be empty after a --no-checkout clone"
        );

        repo.checkout(&first_commit).unwrap();
        assert_eq!(fs::read_to_string(clone.path("test.txt")).unwrap(), "initial content\n");
        assert!(repo.diff(&first_commit, &second_commit).unwrap().contains("+modified content"));
    }
}
