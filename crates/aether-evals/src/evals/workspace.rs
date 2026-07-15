use super::diff::{DiffStats, GitDiff};
use crate::WorkspaceError;
use crate::git_repo::{CloneMode, GitRepo};
use schemars::JsonSchema;
use serde::Serialize;
use std::{
    fs::{create_dir_all, write},
    path::{Path, PathBuf},
};
use tempfile::TempDir;

pub struct Workspace {
    path: PathBuf,
    root_path: PathBuf,
    relative_cwd: Option<PathBuf>,
    source: WorkspaceSource,
    temp_dir: TempDir,
}

#[derive(Debug, Clone)]
pub enum WorkspaceSource {
    Local,
    GitRepo { url: String, start_commit: String, gold_commit: String },
    Bundle { start_commit: String, gold_commit: String },
}

#[derive(Debug, Clone, serde::Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct GitRepoSpec {
    pub url: String,
    pub start_commit: String,
    pub gold_commit: String,
    #[serde(default)]
    pub subdir: Option<PathBuf>,
}

#[derive(Debug, Clone, serde::Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct GitBundleSpec {
    pub bundle_path: PathBuf,
    pub start_commit: String,
    pub gold_commit: String,
    #[serde(default)]
    pub subdir: Option<PathBuf>,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RetainedWorkspaceInfo {
    pub root_path: PathBuf,
    pub path: PathBuf,
}

impl Workspace {
    pub fn empty() -> Result<Self, WorkspaceError> {
        let temp_dir = new_temp_dir()?;
        let path = temp_dir.path().to_path_buf();
        Ok(Self { path: path.clone(), root_path: path, relative_cwd: None, source: WorkspaceSource::Local, temp_dir })
    }

    pub fn from_dir(src_path: impl Into<PathBuf>) -> Result<Self, WorkspaceError> {
        let src_path = src_path.into();
        let temp_dir = new_temp_dir()?;
        let path = temp_dir.path().to_path_buf();

        copy_dir_contents(&src_path, &path).map_err(|source| WorkspaceError::CopyFixture {
            from: src_path.clone(),
            to: path.clone(),
            source,
        })?;

        Ok(Self { path: path.clone(), root_path: path, relative_cwd: None, source: WorkspaceSource::Local, temp_dir })
    }

    pub fn from_files<T: AsRef<Path>, U: AsRef<str>>(
        files: impl IntoIterator<Item = (T, U)>,
    ) -> Result<Self, WorkspaceError> {
        let workspace = Self::empty()?;
        for (relative_path, contents) in files {
            let path = workspace.path().join(relative_path.as_ref());
            if let Some(parent) = path.parent() {
                create_dir_all(parent)
                    .map_err(|source| WorkspaceError::WriteFile { path: parent.to_path_buf(), source })?;
            }
            write(&path, contents.as_ref()).map_err(|source| WorkspaceError::WriteFile { path, source })?;
        }
        Ok(workspace)
    }

    pub fn from_git_repo(spec: GitRepoSpec) -> Result<Self, WorkspaceError> {
        let GitRepoSpec { url, start_commit, gold_commit, subdir } = spec;
        let temp_dir = new_temp_dir()?;

        tracing::debug!("Cloning git repo {} at commit {}", url, start_commit);
        let repo = GitRepo::clone(&url, temp_dir.path(), CloneMode::Blobless)?;

        let source = WorkspaceSource::GitRepo { url, start_commit: start_commit.clone(), gold_commit };
        Self::from_cloned(temp_dir, &repo, &start_commit, subdir, source)
    }

    /// Instantiate a workspace from a local git bundle file
    pub fn from_git_bundle(spec: GitBundleSpec) -> Result<Self, WorkspaceError> {
        let GitBundleSpec { bundle_path, start_commit, gold_commit, subdir } = spec;
        if !bundle_path.exists() {
            return Err(WorkspaceError::MissingBundle { path: bundle_path });
        }

        tracing::debug!("Cloning git bundle {} at commit {}", bundle_path.display(), start_commit);
        let temp_dir = new_temp_dir()?;
        let repo = GitRepo::clone(&bundle_path, temp_dir.path(), CloneMode::Full)?;
        let source = WorkspaceSource::Bundle { start_commit: start_commit.clone(), gold_commit };
        Self::from_cloned(temp_dir, &repo, &start_commit, subdir, source)
    }

    fn from_cloned(
        temp_dir: TempDir,
        repo: &GitRepo,
        start_commit: &str,
        subdir: Option<PathBuf>,
        source: WorkspaceSource,
    ) -> Result<Self, WorkspaceError> {
        repo.checkout(start_commit)?;
        let root_path = temp_dir.path().to_path_buf();
        let (path, relative_cwd) = resolve_subdir(&root_path, subdir)?;
        Ok(Self { path, root_path, relative_cwd, source, temp_dir })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn join(&self, relative_path: impl AsRef<Path>) -> PathBuf {
        self.path.join(relative_path)
    }

    pub fn root_path(&self) -> &Path {
        &self.root_path
    }

    pub fn relative_cwd(&self) -> Option<&Path> {
        self.relative_cwd.as_deref()
    }

    pub fn source(&self) -> &WorkspaceSource {
        &self.source
    }

    /// Prevents the workspace from getting automatically removed and returns its retained root and effective cwd. The caller is responsible for cleanup.
    pub fn persist(self) -> RetainedWorkspaceInfo {
        let root_path = self.temp_dir.keep();
        let path = self.relative_cwd.map_or_else(|| root_path.clone(), |relative_cwd| root_path.join(relative_cwd));
        RetainedWorkspaceInfo { root_path, path }
    }

    pub fn capture_git_diffs(&self) -> (Option<GitDiff>, Option<GitDiff>) {
        let Some((start_commit, gold_commit)) = self.diff_commits() else {
            return (None, None);
        };

        let repo = GitRepo::from_path(self.path());
        let agent_diff = repo.diff_unstaged().ok().map(|diff| GitDiff { stats: DiffStats::from_diff(&diff), diff });
        let reference_diff =
            repo.diff(start_commit, gold_commit).ok().map(|diff| GitDiff { stats: DiffStats::from_diff(&diff), diff });

        (agent_diff, reference_diff)
    }

    fn diff_commits(&self) -> Option<(&str, &str)> {
        match &self.source {
            WorkspaceSource::Local => None,
            WorkspaceSource::GitRepo { start_commit, gold_commit, .. }
            | WorkspaceSource::Bundle { start_commit, gold_commit } => Some((start_commit, gold_commit)),
        }
    }
}

const EVAL_START_REF: &str = "eval-start";
const EVAL_GOLD_REF: &str = "eval-gold";

/// Create a self-contained git bundle at `out` containing `spec`'s start and gold commits.
pub fn create_git_bundle(spec: &GitRepoSpec, out: &Path) -> Result<(), WorkspaceError> {
    let temp_dir = new_temp_dir()?;
    let repo = GitRepo::init(temp_dir.path())?;
    repo.fetch(&spec.url, &[&spec.start_commit, &spec.gold_commit])?;
    repo.update_ref(&format!("refs/heads/{EVAL_START_REF}"), &spec.start_commit)?;
    repo.update_ref(&format!("refs/heads/{EVAL_GOLD_REF}"), &spec.gold_commit)?;
    repo.bundle(&[EVAL_START_REF, EVAL_GOLD_REF], out)?;
    Ok(())
}

fn new_temp_dir() -> Result<tempfile::TempDir, WorkspaceError> {
    tempfile::tempdir().map_err(WorkspaceError::CreateTempDir)
}

fn resolve_subdir(root_path: &Path, subdir: Option<PathBuf>) -> Result<(PathBuf, Option<PathBuf>), WorkspaceError> {
    match subdir {
        None => Ok((root_path.to_path_buf(), None)),
        Some(relative_cwd) => {
            let working_path = root_path.join(&relative_cwd);
            if !working_path.exists() {
                return Err(WorkspaceError::MissingSubdir { path: working_path });
            }
            Ok((working_path, Some(relative_cwd)))
        }
    }
}

fn copy_dir_contents(src: &Path, dst: &Path) -> std::io::Result<()> {
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let source_path = entry.path();
        let dest_path = dst.join(entry.file_name());
        let file_type = entry.file_type()?;

        if file_type.is_dir() {
            std::fs::create_dir_all(&dest_path)?;
            copy_dir_contents(&source_path, &dest_path)?;
        } else if file_type.is_file() {
            std::fs::copy(&source_path, &dest_path)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::{read_to_string, remove_dir_all};
    use std::process::Command;
    use tempfile::TempDir;

    #[test]
    fn persist_reports_root_and_path_for_local_workspace() {
        let workspace = Workspace::from_files([("notes.txt", "hi\n")]).unwrap();

        let retained = workspace.persist();

        assert_eq!(retained.root_path, retained.path);
        assert_eq!(read_to_string(retained.path.join("notes.txt")).unwrap(), "hi\n");
        remove_dir_all(retained.root_path).unwrap();
    }

    #[test]
    fn from_git_bundle_round_trips_checkout_subdir_and_diffs() {
        let (repo, start, gold) = init_repo();
        let bundle_dir = tempfile::tempdir().unwrap();
        let bundle_path = bundle_dir.path().join("repo.bundle");
        create_git_bundle(
            &GitRepoSpec {
                url: format!("file://{}", repo.path().display()),
                start_commit: start.clone(),
                gold_commit: gold.clone(),
                subdir: None,
            },
            &bundle_path,
        )
        .unwrap();

        let workspace = Workspace::from_git_bundle(GitBundleSpec {
            bundle_path,
            start_commit: start,
            gold_commit: gold,
            subdir: Some(PathBuf::from("pkg")),
        })
        .unwrap();

        let (agent_diff, reference_diff) = workspace.capture_git_diffs();

        assert_eq!(read_to_string(workspace.root_path().join("root.txt")).unwrap(), "root v1\n");
        assert_eq!(workspace.relative_cwd(), Some(Path::new("pkg")));
        assert_eq!(workspace.path(), workspace.root_path().join("pkg"));
        assert!(reference_diff.unwrap().diff.contains("root v2"), "reference diff should span start..gold");
        assert!(agent_diff.unwrap().diff.is_empty(), "no agent edits yet");

        write(workspace.root_path().join("root.txt"), "root edited\n").unwrap();
        let (agent_diff, _) = workspace.capture_git_diffs();

        assert!(agent_diff.unwrap().diff.contains("root edited"), "agent diff should capture the edit");
    }

    #[test]
    fn from_git_bundle_missing_file_errors() {
        let result = Workspace::from_git_bundle(GitBundleSpec {
            bundle_path: PathBuf::from("/nonexistent/repo.bundle"),
            start_commit: "abc".into(),
            gold_commit: "def".into(),
            subdir: None,
        });

        assert!(matches!(result, Err(WorkspaceError::MissingBundle { .. })));
    }

    fn init_repo() -> (TempDir, String, String) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path();
        git(path, &["init", "--initial-branch", "main"]);
        git(path, &["config", "user.email", "eval@example.com"]);
        git(path, &["config", "user.name", "Eval"]);

        write(path.join("root.txt"), "root v1\n").unwrap();
        create_dir_all(path.join("pkg")).unwrap();
        write(path.join("pkg").join("inner.txt"), "inner v1\n").unwrap();
        git(path, &["add", "."]);
        git(path, &["commit", "-m", "start"]);
        let start = git(path, &["rev-parse", "HEAD"]);

        write(path.join("root.txt"), "root v2\n").unwrap();
        git(path, &["add", "."]);
        git(path, &["commit", "-m", "gold"]);
        let gold = git(path, &["rev-parse", "HEAD"]);

        (dir, start, gold)
    }

    fn git(repo: &Path, args: &[&str]) -> String {
        let output = Command::new("git").arg("-C").arg(repo).args(args).output().unwrap();
        assert!(output.status.success(), "git {args:?} failed: {}", String::from_utf8_lossy(&output.stderr));
        String::from_utf8(output.stdout).unwrap().trim().to_string()
    }
}
