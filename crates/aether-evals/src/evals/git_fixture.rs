use super::diff::GitDiff;
use super::workspace::{Workspace, new_temp_dir};
use crate::WorkspaceError;
use crate::git_repo::GitRepo;
use schemars::JsonSchema;
use std::{
    ffi::OsString,
    path::{Path, PathBuf},
};
use tempfile::TempDir;

#[derive(Debug, Clone, serde::Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct GitSpec {
    pub source: GitSource,
    pub start_commit: String,
    pub gold_commit: String,
    #[serde(default)]
    pub subdir: Option<PathBuf>,
    #[serde(default = "default_branch")]
    pub branch: String,
    #[serde(default)]
    pub diff_exclude: Vec<String>,
}

#[derive(Debug, Clone, serde::Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum GitSource {
    Url(String),
    Bundle(PathBuf),
}

pub struct GitFixture {
    repo: GitRepo,
    spec: GitSpec,
    reference_diff: GitDiff,
    _repo_dir: TempDir,
}

impl GitSpec {
    pub fn new(source: GitSource, start_commit: impl Into<String>, gold_commit: impl Into<String>) -> Self {
        Self {
            source,
            start_commit: start_commit.into(),
            gold_commit: gold_commit.into(),
            subdir: None,
            branch: default_branch(),
            diff_exclude: Vec::new(),
        }
    }
}

impl GitFixture {
    pub fn fetch(spec: GitSpec) -> Result<Self, WorkspaceError> {
        let GitSpec { source, start_commit, gold_commit, diff_exclude, .. } = &spec;
        let remote = source.remote()?;
        tracing::debug!("Fetching {:?} at commits {} and {}", remote, start_commit, gold_commit);

        let repo_dir = new_temp_dir()?;
        let repo = GitRepo::init(repo_dir.path())?;
        repo.fetch(&remote, &[start_commit, gold_commit])?;
        repo.update_ref(&format!("refs/heads/{EVAL_START_REF}"), start_commit)?;
        repo.update_ref(&format!("refs/heads/{EVAL_GOLD_REF}"), gold_commit)?;
        let reference_diff = GitDiff::new(repo.diff(start_commit, gold_commit, diff_exclude)?);

        Ok(Self { repo, spec, reference_diff, _repo_dir: repo_dir })
    }

    pub fn reference_diff(&self) -> &GitDiff {
        &self.reference_diff
    }

    pub fn workspace(&self) -> Result<Workspace, WorkspaceError> {
        let temp_dir = new_temp_dir()?;
        let repo = GitRepo::init(temp_dir.path())?;
        repo.fetch(self.repo.path(), &[&self.spec.start_commit])?;
        repo.checkout_branch(&self.spec.branch, &self.spec.start_commit)?;
        Workspace::from_temp_dir(temp_dir, self.spec.subdir.clone())
    }

    pub fn agent_diff(&self, workspace: &Workspace) -> Result<GitDiff, WorkspaceError> {
        let repo = GitRepo::from_path(workspace.root_path());
        let diff = repo.diff_working_tree(&self.spec.start_commit, &self.spec.diff_exclude)?;
        Ok(GitDiff::new(diff))
    }

    pub fn bundle(&self, out: &Path) -> Result<(), WorkspaceError> {
        self.repo.bundle(&[EVAL_START_REF, EVAL_GOLD_REF], &absolute(out)?)?;
        Ok(())
    }
}

const EVAL_START_REF: &str = "eval-start";
const EVAL_GOLD_REF: &str = "eval-gold";

impl GitSource {
    fn remote(&self) -> Result<OsString, WorkspaceError> {
        match self {
            Self::Url(url) => Ok(url.into()),
            Self::Bundle(path) => {
                let path = absolute(path)?;
                if !path.exists() {
                    return Err(WorkspaceError::MissingBundle { path });
                }
                Ok(path.into_os_string())
            }
        }
    }
}

fn default_branch() -> String {
    "main".to_string()
}

fn absolute(path: &Path) -> Result<PathBuf, WorkspaceError> {
    std::path::absolute(path).map_err(|source| WorkspaceError::ResolvePath { path: path.to_path_buf(), source })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::{create_dir_all, read_to_string, remove_file, write};
    use utils::testing::{TestWorkspace, git};

    #[test]
    fn a_bundled_fixture_round_trips_checkout_subdir_and_diffs() {
        let (_repo, spec) = init_repo();
        let bundle_dir = tempfile::tempdir().unwrap();
        let (fixture, workspace) =
            checkout(GitSpec { subdir: Some("pkg".into()), ..bundle_spec(&spec, bundle_dir.path()) });

        assert_eq!(read_to_string(workspace.root_path().join("root.txt")).unwrap(), "root v1\n");
        assert_eq!(workspace.relative_cwd(), Some(Path::new("pkg")));
        assert_eq!(workspace.path(), workspace.root_path().join("pkg"));
        assert!(fixture.reference_diff().diff.contains("root v2"), "reference diff should span start..gold");
        assert!(agent_diff(&fixture, &workspace).diff.is_empty(), "no agent edits yet");

        write(workspace.root_path().join("root.txt"), "root edited\n").unwrap();

        assert!(agent_diff(&fixture, &workspace).diff.contains("root edited"), "agent diff should capture the edit");
    }

    #[test]
    fn bundles_round_trip_through_relative_paths() {
        let (_repo, spec) = init_repo();
        let bundle_dir = tempfile::tempdir_in(".").unwrap();
        let cwd = std::env::current_dir().unwrap();
        let bundle_path = bundle_dir.path().strip_prefix(cwd).unwrap().join("repo.bundle");

        fixture(spec.clone()).bundle(&bundle_path).unwrap();
        let (fixture, workspace) = checkout(GitSpec { source: GitSource::Bundle(bundle_path), ..spec.clone() });

        assert_eq!(git(workspace.root_path(), &["rev-parse", "HEAD"]), spec.start_commit);
        assert!(fixture.reference_diff().diff.contains("+root v2"));
    }

    #[test]
    fn workspaces_hold_only_the_start_commit() {
        let (_repo, spec) = init_repo();
        let bundle_dir = tempfile::tempdir().unwrap();
        let fixtures = [fixture(spec.clone()), fixture(bundle_spec(&spec, bundle_dir.path()))];

        for fixture in fixtures {
            let workspace = fixture.workspace().unwrap();
            let root = workspace.root_path();
            assert_eq!(git(root, &["rev-parse", "HEAD"]), spec.start_commit);
            assert_eq!(git(root, &["for-each-ref", "--format=%(refname)"]), "refs/heads/main");
            assert_eq!(git(root, &["remote"]), "");
            assert!(
                utils::git::git(Some(root), &["cat-file", "-e", &spec.gold_commit]).is_err(),
                "the gold commit must not be in the agent's repository"
            );
            assert!(fixture.reference_diff().diff.contains("+root v2"));
        }
    }

    #[test]
    fn each_workspace_starts_fresh() {
        let (_repo, spec) = init_repo();
        let (fixture, first) = checkout(spec);
        write(first.join("root.txt"), "agent\n").unwrap();

        let second = fixture.workspace().unwrap();

        assert_ne!(first.root_path(), second.root_path());
        assert_eq!(read_to_string(second.join("root.txt")).unwrap(), "root v1\n");
    }

    #[test]
    fn workspaces_check_out_the_requested_branch() {
        let (_repo, spec) = init_repo();
        let (_fixture, workspace) = checkout(GitSpec { branch: "trunk".into(), ..spec });

        assert_eq!(git(workspace.root_path(), &["branch", "--show-current"]), "trunk");
    }

    #[test]
    fn diff_exclude_leaves_paths_out_of_the_reference_and_agent_diffs() {
        let (_repo, spec) = init_repo();
        let (fixture, workspace) = checkout(GitSpec { diff_exclude: vec!["*root.txt".into()], ..spec });
        write(workspace.join("root.txt"), "agent\n").unwrap();
        write(workspace.join("pkg/inner.txt"), "agent inner\n").unwrap();

        let agent_diff = agent_diff(&fixture, &workspace);

        assert_eq!(fixture.reference_diff().diff, "");
        assert!(agent_diff.diff.contains("+agent inner"));
        assert!(!agent_diff.diff.contains("root.txt"));
    }

    #[test]
    fn agent_diff_includes_committed_changes() {
        let (_repo, spec) = init_repo();
        let (fixture, workspace) = checkout(spec);

        write(workspace.join("root.txt"), "agent committed\n").unwrap();
        git(workspace.root_path(), &["add", "."]);
        git(
            workspace.root_path(),
            &["-c", "user.email=agent@example.com", "-c", "user.name=Agent", "commit", "-m", "agent change"],
        );

        let agent_diff = agent_diff(&fixture, &workspace);

        assert!(agent_diff.diff.contains("+agent committed"));
        assert_eq!(agent_diff.stats.files_changed, 1);
        assert_eq!(agent_diff.stats.lines_added, 1);
        assert_eq!(agent_diff.stats.lines_removed, 1);
    }

    #[test]
    fn agent_diff_includes_staged_unstaged_added_deleted_and_untracked_changes() {
        let (_repo, spec) = init_repo();
        let (fixture, workspace) = checkout(spec);

        write(workspace.join("root.txt"), "staged change\n").unwrap();
        write(workspace.join("added.txt"), "added\n").unwrap();
        git(workspace.root_path(), &["add", "root.txt", "added.txt"]);
        write(workspace.join("pkg/inner.txt"), "unstaged change\n").unwrap();
        remove_file(workspace.join("deleted.txt")).unwrap();
        create_dir_all(workspace.join("dir")).unwrap();
        write(workspace.join("dir/untracked.txt"), "untracked\n").unwrap();

        let agent_diff = agent_diff(&fixture, &workspace);

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
    fn agent_diff_keeps_files_tracked_despite_gitignore() {
        let repo = TestWorkspace::new().file(".gitignore", "*.env\n").git_init();
        repo.write("app.env", "start\n");
        repo.git(&["add", "--force", "app.env"]);
        let start_commit = repo.commit("start");
        let source = GitSource::Url(format!("file://{}", repo.root().display()));
        let (fixture, workspace) = checkout(GitSpec::new(source, start_commit.clone(), start_commit));

        let agent_diff = agent_diff(&fixture, &workspace);

        assert_eq!(agent_diff.diff, "");
    }

    #[test]
    fn agent_diff_rebuilds_a_deleted_index() {
        let (_repo, spec) = init_repo();
        let (fixture, workspace) = checkout(spec);
        write(workspace.join("root.txt"), "agent\n").unwrap();
        remove_file(workspace.root_path().join(".git/index")).unwrap();

        let agent_diff = agent_diff(&fixture, &workspace);

        assert!(agent_diff.diff.contains("+agent"));
        assert_eq!(agent_diff.stats.files_changed, 1);
    }

    #[test]
    fn agent_diff_rebuilds_a_deleted_index_keeping_files_tracked_despite_gitignore() {
        let repo = TestWorkspace::new().file(".gitignore", "*.env\n").git_init();
        repo.write("app.env", "start\n");
        repo.git(&["add", "--force", "app.env"]);
        let start_commit = repo.commit("start");
        let source = GitSource::Url(format!("file://{}", repo.root().display()));
        let (fixture, workspace) = checkout(GitSpec::new(source, start_commit.clone(), start_commit));
        remove_file(workspace.root_path().join(".git/index")).unwrap();

        assert_eq!(agent_diff(&fixture, &workspace).diff, "");
    }

    #[test]
    fn agent_diff_errors_when_git_fails() {
        let (_repo, spec) = init_repo();
        let (fixture, workspace) = checkout(spec);
        write(workspace.root_path().join(".git/index"), "corrupt").unwrap();

        assert!(matches!(fixture.agent_diff(&workspace), Err(WorkspaceError::Git(_))));
    }

    #[test]
    fn fetching_a_missing_bundle_errors() {
        let spec = GitSpec::new(GitSource::Bundle(PathBuf::from("/nonexistent/repo.bundle")), "abc", "def");

        assert!(matches!(GitFixture::fetch(spec), Err(WorkspaceError::MissingBundle { .. })));
    }

    fn fixture(spec: GitSpec) -> GitFixture {
        GitFixture::fetch(spec).unwrap()
    }

    fn checkout(spec: GitSpec) -> (GitFixture, Workspace) {
        let fixture = fixture(spec);
        let workspace = fixture.workspace().unwrap();
        (fixture, workspace)
    }

    fn agent_diff(fixture: &GitFixture, workspace: &Workspace) -> GitDiff {
        fixture.agent_diff(workspace).unwrap()
    }

    fn bundle_spec(spec: &GitSpec, dir: &Path) -> GitSpec {
        let bundle_path = dir.join("repo.bundle");
        fixture(spec.clone()).bundle(&bundle_path).unwrap();
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
        (repo, GitSpec::new(source, start_commit, gold_commit))
    }
}
