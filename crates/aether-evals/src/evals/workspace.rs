use super::diff::GitDiff;
use crate::WorkspaceError;
use crate::git_repo::GitRepo;
use schemars::JsonSchema;
use serde::Serialize;
use std::{
    ffi::OsStr,
    fs::{create_dir_all, write},
    path::{Path, PathBuf},
};
use tempfile::TempDir;

pub struct Workspace {
    path: PathBuf,
    root_path: PathBuf,
    relative_cwd: Option<PathBuf>,
    git: Option<GitBaseline>,
    temp_dir: TempDir,
}

#[derive(Debug, Clone, serde::Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct GitSpec {
    pub source: GitSource,
    pub start_commit: String,
    pub gold_commit: String,
    #[serde(default)]
    pub subdir: Option<PathBuf>,
    #[serde(default)]
    pub branch: Option<String>,
}

#[derive(Debug, Clone, serde::Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum GitSource {
    Url(String),
    Bundle(PathBuf),
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RetainedWorkspaceInfo {
    pub root_path: PathBuf,
    pub path: PathBuf,
}

impl Workspace {
    pub fn empty() -> Result<Self, WorkspaceError> {
        Ok(Self::local(new_temp_dir()?))
    }

    pub fn from_dir(src_path: impl Into<PathBuf>) -> Result<Self, WorkspaceError> {
        let src_path = src_path.into();
        let temp_dir = new_temp_dir()?;

        copy_dir_contents(&src_path, temp_dir.path()).map_err(|source| WorkspaceError::CopyFixture {
            from: src_path.clone(),
            to: temp_dir.path().to_path_buf(),
            source,
        })?;

        Ok(Self::local(temp_dir))
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

    pub fn from_git(spec: GitSpec) -> Result<Self, WorkspaceError> {
        let GitSpec { source, start_commit, gold_commit, subdir, branch } = spec;
        if let GitSource::Bundle(path) = &source
            && !path.exists()
        {
            return Err(WorkspaceError::MissingBundle { path: path.clone() });
        }
        tracing::debug!("Fetching {:?} at commit {}", source, start_commit);

        let scratch_dir = new_temp_dir()?;
        let scratch = GitRepo::init(scratch_dir.path())?;
        scratch.fetch(source.remote(), &[&start_commit, &gold_commit])?;
        let reference_diff = GitDiff::new(scratch.diff(&start_commit, &gold_commit)?);

        let temp_dir = new_temp_dir()?;
        let repo = GitRepo::init(temp_dir.path())?;
        repo.fetch(scratch_dir.path(), &[&start_commit])?;
        repo.checkout_branch(branch.as_deref().unwrap_or(DEFAULT_BRANCH), &start_commit)?;

        let root_path = temp_dir.path().to_path_buf();
        let (path, relative_cwd) = resolve_subdir(&root_path, subdir)?;
        let git = GitBaseline { start_commit, reference_diff };
        Ok(Self { path, root_path, relative_cwd, git: Some(git), temp_dir })
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

    /// Prevents the workspace from getting automatically removed and returns its retained root and effective cwd. The caller is responsible for cleanup.
    pub fn persist(self) -> RetainedWorkspaceInfo {
        let root_path = self.temp_dir.keep();
        let path = self.relative_cwd.map_or_else(|| root_path.clone(), |relative_cwd| root_path.join(relative_cwd));
        RetainedWorkspaceInfo { root_path, path }
    }

    pub fn reference_diff(&self) -> Option<&GitDiff> {
        self.git.as_ref().map(|git| &git.reference_diff)
    }

    pub fn capture_agent_diff(&self) -> Result<Option<GitDiff>, WorkspaceError> {
        let Some(git) = &self.git else {
            return Ok(None);
        };
        let diff = GitRepo::from_path(&self.root_path).diff_working_tree(&git.start_commit)?;
        Ok(Some(GitDiff::new(diff)))
    }

    fn local(temp_dir: TempDir) -> Self {
        let path = temp_dir.path().to_path_buf();
        Self { path: path.clone(), root_path: path, relative_cwd: None, git: None, temp_dir }
    }
}

const EVAL_START_REF: &str = "eval-start";
const EVAL_GOLD_REF: &str = "eval-gold";
const DEFAULT_BRANCH: &str = "main";

/// Create a self-contained git bundle at `out` containing `spec`'s start and gold commits.
pub fn create_git_bundle(spec: &GitSpec, out: &Path) -> Result<(), WorkspaceError> {
    let temp_dir = new_temp_dir()?;
    let repo = GitRepo::init(temp_dir.path())?;
    repo.fetch(spec.source.remote(), &[&spec.start_commit, &spec.gold_commit])?;
    repo.update_ref(&format!("refs/heads/{EVAL_START_REF}"), &spec.start_commit)?;
    repo.update_ref(&format!("refs/heads/{EVAL_GOLD_REF}"), &spec.gold_commit)?;
    repo.bundle(&[EVAL_START_REF, EVAL_GOLD_REF], out)?;
    Ok(())
}

struct GitBaseline {
    start_commit: String,
    reference_diff: GitDiff,
}

impl GitSource {
    fn remote(&self) -> &OsStr {
        match self {
            Self::Url(url) => url.as_ref(),
            Self::Bundle(path) => path.as_os_str(),
        }
    }
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
    use utils::testing::{TestWorkspace, git};

    #[test]
    fn persist_reports_root_and_path_for_local_workspace() {
        let workspace = Workspace::from_files([("notes.txt", "hi\n")]).unwrap();

        let retained = workspace.persist();

        assert_eq!(retained.root_path, retained.path);
        assert_eq!(read_to_string(retained.path.join("notes.txt")).unwrap(), "hi\n");
        remove_dir_all(retained.root_path).unwrap();
    }

    #[test]
    fn local_workspaces_have_no_diffs() {
        let workspace = Workspace::empty().unwrap();

        assert!(workspace.reference_diff().is_none());
        assert!(workspace.capture_agent_diff().unwrap().is_none());
    }

    #[test]
    fn git_spec_deserializes_from_camel_case_json() {
        let spec: GitSpec = serde_json::from_value(serde_json::json!({
            "source": { "bundle": "repo.bundle" },
            "startCommit": "abc",
            "goldCommit": "def",
        }))
        .unwrap();

        assert!(matches!(spec.source, GitSource::Bundle(path) if path == Path::new("repo.bundle")));
    }

    #[test]
    fn from_git_with_a_bundle_round_trips_checkout_subdir_and_diffs() {
        let (_repo, spec) = init_repo();
        let bundle_dir = tempfile::tempdir().unwrap();
        let spec = GitSpec { subdir: Some("pkg".into()), ..bundle_spec(&spec, bundle_dir.path()) };
        let workspace = Workspace::from_git(spec).unwrap();

        assert_eq!(read_to_string(workspace.root_path().join("root.txt")).unwrap(), "root v1\n");
        assert_eq!(workspace.relative_cwd(), Some(Path::new("pkg")));
        assert_eq!(workspace.path(), workspace.root_path().join("pkg"));
        assert!(workspace.reference_diff().unwrap().diff.contains("root v2"), "reference diff should span start..gold");
        assert!(agent_diff(&workspace).diff.is_empty(), "no agent edits yet");

        write(workspace.root_path().join("root.txt"), "root edited\n").unwrap();

        assert!(agent_diff(&workspace).diff.contains("root edited"), "agent diff should capture the edit");
    }

    #[test]
    fn git_workspaces_hold_only_the_start_commit() {
        let (_repo, spec) = init_repo();
        let bundle_dir = tempfile::tempdir().unwrap();
        let workspaces = [
            Workspace::from_git(spec.clone()).unwrap(),
            Workspace::from_git(bundle_spec(&spec, bundle_dir.path())).unwrap(),
        ];

        for workspace in workspaces {
            let root = workspace.root_path();
            assert_eq!(git(root, &["rev-parse", "HEAD"]), spec.start_commit);
            assert_eq!(git(root, &["for-each-ref", "--format=%(refname)"]), "refs/heads/main");
            assert_eq!(git(root, &["remote"]), "");
            assert!(
                utils::git::git(Some(root), &["cat-file", "-e", &spec.gold_commit]).is_err(),
                "the gold commit must not be in the agent's repository"
            );
            assert!(workspace.reference_diff().unwrap().diff.contains("+root v2"));
        }
    }

    #[test]
    fn from_git_checks_out_the_requested_branch() {
        let (_repo, spec) = init_repo();

        let workspace = Workspace::from_git(GitSpec { branch: Some("trunk".into()), ..spec }).unwrap();

        assert_eq!(git(workspace.root_path(), &["branch", "--show-current"]), "trunk");
    }

    #[test]
    fn capture_agent_diff_includes_committed_changes() {
        let (_repo, spec) = init_repo();
        let workspace = Workspace::from_git(spec).unwrap();

        write(workspace.join("root.txt"), "agent committed\n").unwrap();
        git(workspace.root_path(), &["add", "."]);
        git(
            workspace.root_path(),
            &["-c", "user.email=agent@example.com", "-c", "user.name=Agent", "commit", "-m", "agent change"],
        );

        let agent_diff = agent_diff(&workspace);

        assert!(agent_diff.diff.contains("+agent committed"));
        assert_eq!(agent_diff.stats.files_changed, 1);
        assert_eq!(agent_diff.stats.lines_added, 1);
        assert_eq!(agent_diff.stats.lines_removed, 1);
    }

    #[test]
    fn capture_agent_diff_includes_staged_unstaged_added_deleted_and_untracked_changes() {
        let (_repo, spec) = init_repo();
        let workspace = Workspace::from_git(spec).unwrap();

        write(workspace.join("root.txt"), "staged change\n").unwrap();
        write(workspace.join("added.txt"), "added\n").unwrap();
        git(workspace.root_path(), &["add", "root.txt", "added.txt"]);
        write(workspace.join("pkg/inner.txt"), "unstaged change\n").unwrap();
        std::fs::remove_file(workspace.join("deleted.txt")).unwrap();
        create_dir_all(workspace.join("dir")).unwrap();
        write(workspace.join("dir/untracked.txt"), "untracked\n").unwrap();

        let agent_diff = agent_diff(&workspace);

        assert!(agent_diff.diff.contains("root.txt"));
        assert!(agent_diff.diff.contains("+staged change"));
        assert!(agent_diff.diff.contains("pkg/inner.txt"));
        assert!(agent_diff.diff.contains("+unstaged change"));
        assert!(agent_diff.diff.contains("added.txt"));
        assert!(agent_diff.diff.contains("+added"));
        assert!(agent_diff.diff.contains("deleted.txt"));
        assert!(agent_diff.diff.contains("-before deleted"));
        assert!(agent_diff.diff.contains("+untracked"));
        assert_eq!(agent_diff.stats.files_changed, 5);
        assert_eq!(agent_diff.stats.lines_added, 4);
        assert_eq!(agent_diff.stats.lines_removed, 3);
        assert!(git(workspace.root_path(), &["status", "--porcelain"]).contains("?? dir/"), "the index is untouched");
    }

    #[test]
    fn capture_agent_diff_errors_when_git_fails() {
        let (_repo, spec) = init_repo();
        let workspace = Workspace::from_git(spec).unwrap();
        write(workspace.root_path().join(".git/index"), "corrupt").unwrap();

        assert!(matches!(workspace.capture_agent_diff(), Err(WorkspaceError::Git(_))));
    }

    #[test]
    fn from_git_missing_bundle_errors() {
        let result = Workspace::from_git(GitSpec {
            source: GitSource::Bundle(PathBuf::from("/nonexistent/repo.bundle")),
            start_commit: "abc".into(),
            gold_commit: "def".into(),
            subdir: None,
            branch: None,
        });

        assert!(matches!(result, Err(WorkspaceError::MissingBundle { .. })));
    }

    fn agent_diff(workspace: &Workspace) -> GitDiff {
        workspace.capture_agent_diff().unwrap().expect("git workspaces have an agent diff")
    }

    fn bundle_spec(spec: &GitSpec, dir: &Path) -> GitSpec {
        let bundle_path = dir.join("repo.bundle");
        create_git_bundle(spec, &bundle_path).unwrap();
        GitSpec { source: GitSource::Bundle(bundle_path), ..spec.clone() }
    }

    fn init_repo() -> (TestWorkspace, GitSpec) {
        let repo = TestWorkspace::new()
            .file("root.txt", "root v1\n")
            .file("deleted.txt", "before deleted\n")
            .file("pkg/inner.txt", "inner v1\n")
            .git_init();
        let start_commit = repo.commit("start");
        repo.write("root.txt", "root v2\n");
        let gold_commit = repo.commit("gold");
        let source = GitSource::Url(format!("file://{}", repo.root().display()));
        (repo, GitSpec { source, start_commit, gold_commit, subdir: None, branch: None })
    }
}
