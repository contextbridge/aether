use super::report::{DiffStats, GitDiff};
use crate::WorkspaceError;
use crate::git_repo::GitRepo;
use std::path::{Path, PathBuf};
use tempfile::TempDir;

pub struct Workspace {
    path: PathBuf,
    root_path: PathBuf,
    relative_cwd: Option<PathBuf>,
    source: WorkspaceSource,
    _drop_guard: TempDir,
}

#[derive(Debug, Clone)]
pub enum WorkspaceSource {
    Local,
    GitRepo { url: String, start_commit: String, gold_commit: String },
}

#[derive(Debug, Clone, serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct GitRepoSpec {
    pub url: String,
    pub start_commit: String,
    pub gold_commit: String,
    #[serde(default)]
    pub subdir: Option<PathBuf>,
}

impl Workspace {
    pub fn empty() -> Result<Self, WorkspaceError> {
        let temp_dir = new_temp_dir()?;
        let path = temp_dir.path().to_path_buf();
        Ok(Self {
            path: path.clone(),
            root_path: path,
            relative_cwd: None,
            source: WorkspaceSource::Local,
            _drop_guard: temp_dir,
        })
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

        Ok(Self {
            path: path.clone(),
            root_path: path,
            relative_cwd: None,
            source: WorkspaceSource::Local,
            _drop_guard: temp_dir,
        })
    }

    pub fn from_files<T, U>(files: impl IntoIterator<Item = (T, U)>) -> Result<Self, WorkspaceError>
    where
        T: AsRef<Path>,
        U: AsRef<str>,
    {
        let workspace = Self::empty()?;
        for (relative_path, contents) in files {
            let path = workspace.path().join(relative_path.as_ref());
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)
                    .map_err(|source| WorkspaceError::WriteFile { path: parent.to_path_buf(), source })?;
            }
            std::fs::write(&path, contents.as_ref()).map_err(|source| WorkspaceError::WriteFile { path, source })?;
        }
        Ok(workspace)
    }

    pub fn from_git_repo(spec: GitRepoSpec) -> Result<Self, WorkspaceError> {
        let GitRepoSpec { url, start_commit, gold_commit, subdir } = spec;
        let temp_dir = new_temp_dir()?;

        tracing::debug!("Cloning git repo {} at commit {}", url, start_commit);
        let repo = GitRepo::clone(&url, temp_dir.path())?;
        repo.checkout(&start_commit)?;

        let root_path = temp_dir.path().to_path_buf();
        let (path, relative_cwd) = match subdir {
            None => (root_path.clone(), None),
            Some(relative_cwd) => {
                let working_path = root_path.join(&relative_cwd);
                if !working_path.exists() {
                    return Err(WorkspaceError::MissingSubdir { path: working_path });
                }
                (working_path, Some(relative_cwd))
            }
        };

        Ok(Self {
            path,
            root_path,
            relative_cwd,
            source: WorkspaceSource::GitRepo { url, start_commit, gold_commit },
            _drop_guard: temp_dir,
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
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

    pub fn capture_git_diffs(&self) -> (Option<GitDiff>, Option<GitDiff>) {
        let WorkspaceSource::GitRepo { start_commit, gold_commit, .. } = self.source() else {
            return (None, None);
        };

        let repo = GitRepo::from_path(self.path());
        let agent_diff = repo.diff_unstaged().ok().map(|diff| GitDiff { stats: DiffStats::from_diff(&diff), diff });
        let reference_diff =
            repo.diff(start_commit, gold_commit).ok().map(|diff| GitDiff { stats: DiffStats::from_diff(&diff), diff });

        (agent_diff, reference_diff)
    }
}

fn new_temp_dir() -> Result<tempfile::TempDir, WorkspaceError> {
    tempfile::tempdir().map_err(WorkspaceError::CreateTempDir)
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
