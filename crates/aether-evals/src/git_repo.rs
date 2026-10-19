use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use utils::git::{GitError, git};

/// Represents a git repository used for evaluation purposes
pub(crate) struct GitRepo {
    path: PathBuf,
}

impl GitRepo {
    /// Create a `GitRepo` instance from an existing repository path
    pub(crate) fn from_path(path: &Path) -> Self {
        GitRepo { path: path.to_path_buf() }
    }

    /// Initialize an empty repository at `dest`.
    #[tracing::instrument(skip(dest))]
    pub(crate) fn init(dest: &Path) -> Result<Self, GitError> {
        git(None, &[OsStr::new("init"), dest.as_os_str()])?;
        Ok(GitRepo { path: dest.to_path_buf() })
    }

    #[tracing::instrument(skip(self))]
    pub(crate) fn checkout_branch(&self, branch: &str, commit: &str) -> Result<(), GitError> {
        self.git(&["checkout", "-B", branch, commit])?;
        Ok(())
    }

    /// Fetch `revs` from `remote` (a named remote or a URL) into this repository.
    #[tracing::instrument(skip(self, remote))]
    pub(crate) fn fetch(&self, remote: impl AsRef<OsStr>, revs: &[&str]) -> Result<(), GitError> {
        let mut args = vec![OsStr::new("fetch"), OsStr::new("--no-tags"), remote.as_ref()];
        args.extend(revs.iter().map(OsStr::new));
        self.git(&args)?;
        Ok(())
    }

    /// Point a ref at a commit (e.g. to create a branch tip to bundle).
    #[tracing::instrument(skip(self))]
    pub(crate) fn update_ref(&self, name: &str, commit: &str) -> Result<(), GitError> {
        self.git(&["update-ref", name, commit])?;
        Ok(())
    }

    /// Create a self-contained git bundle at `out` containing the given refs and their
    /// reachable objects.
    #[tracing::instrument(skip(self, out))]
    pub(crate) fn bundle(&self, revs: &[&str], out: &Path) -> Result<(), GitError> {
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
    pub(crate) fn diff_range(&self, from_commit: &str, to_commit: Option<&str>) -> Result<String, GitError> {
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
    pub(crate) fn diff(&self, from_commit: &str, to_commit: &str) -> Result<String, GitError> {
        self.diff_range(from_commit, Some(to_commit))
    }

    fn git<S: AsRef<OsStr>>(&self, args: &[S]) -> Result<Vec<u8>, GitError> {
        git(Some(&self.path), args)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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
}
