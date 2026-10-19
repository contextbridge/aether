use crate::WorkspaceError;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use utils::git::{GitError, git, git_with_env};

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

    #[tracing::instrument(skip(self))]
    pub(crate) fn diff(&self, from_commit: &str, to_commit: &str, exclude: &[String]) -> Result<String, GitError> {
        let stdout = self.git(&diff_args(&["diff", from_commit, to_commit], exclude))?;
        Ok(String::from_utf8_lossy(&stdout).into_owned())
    }

    #[tracing::instrument(skip(self))]
    pub(crate) fn diff_working_tree(&self, from_commit: &str, exclude: &[String]) -> Result<String, WorkspaceError> {
        let scratch = tempfile::tempdir().map_err(WorkspaceError::CreateTempDir)?;
        let index = scratch.path().join("index");
        let real_index = self.git(&["rev-parse", "--path-format=absolute", "--git-path", "index"])?;
        std::fs::copy(String::from_utf8_lossy(&real_index).trim(), &index).ok();
        let env = [("GIT_INDEX_FILE", index.as_os_str())];
        self.git_with_env(&["add", "--all"], &env)?;
        let stdout = self.git_with_env(&diff_args(&["diff", "--cached", from_commit], exclude), &env)?;
        Ok(String::from_utf8_lossy(&stdout).into_owned())
    }

    fn git<S: AsRef<OsStr>>(&self, args: &[S]) -> Result<Vec<u8>, GitError> {
        self.git_with_env(args, &[])
    }

    fn git_with_env<S: AsRef<OsStr>>(&self, args: &[S], env: &[(&str, &OsStr)]) -> Result<Vec<u8>, GitError> {
        git_with_env(Some(&self.path), args, env)
    }
}

fn diff_args(args: &[&str], exclude: &[String]) -> Vec<String> {
    let separator = (!exclude.is_empty()).then(|| "--".to_string());
    let pathspecs = exclude.iter().map(|pathspec| format!(":(exclude){pathspec}"));
    args.iter().map(ToString::to_string).chain(separator).chain(pathspecs).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use utils::testing::TestWorkspace;

    #[test]
    fn diff_covers_commit_ranges() {
        let repo = TestWorkspace::new().file("test.txt", "initial content\n").git_init();
        let first_commit = repo.commit("Initial commit");
        repo.write("test.txt", "modified content\n");
        repo.write("new.txt", "new file\n");
        let second_commit = repo.commit("Second commit");
        repo.write("test.txt", "unstaged content\n");

        let committed = GitRepo::from_path(repo.root()).diff(&first_commit, &second_commit, &[]).unwrap();

        assert!(committed.contains("+modified content"), "{committed}");
        assert!(committed.contains("+new file"), "{committed}");
        assert!(!committed.contains("unstaged content"), "{committed}");
    }

    #[test]
    fn diff_working_tree_includes_untracked_files_without_touching_the_index() {
        let repo = TestWorkspace::new().file("test.txt", "initial content\n").git_init();
        let commit = repo.commit("Initial commit");
        repo.write("test.txt", "unstaged content\n");
        repo.write("untracked.txt", "untracked\n");

        let diff = GitRepo::from_path(repo.root()).diff_working_tree(&commit, &[]).unwrap();

        assert!(diff.contains("+unstaged content"), "{diff}");
        assert!(diff.contains("+untracked"), "{diff}");
        assert!(repo.git(&["status", "--porcelain"]).contains("?? untracked.txt"));
    }
}
