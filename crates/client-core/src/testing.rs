mod git_diff_server;
mod live_session;
mod session_config;
use git_diff_server::FakeGitDiffServer;
pub use live_session::LiveSession;
pub use session_config::SessionCoreConfigBuilder;

use acp_utils::testing::FakeAgent;
use agent_client_protocol::schema::v2::Implementation;
use agent_client_protocol::{Agent, Client, HandleDispatchFrom, NullRun, V2Builder};
use clankerdiff_client::{RemoteError, RemoteErrorCode};
use clankerdiff_core::{
    DiffDocument, DiffScope, FileDiff, FileStatus, RepositoryAction, ReviewTarget, SourceDocument, SourceResult,
    SourceUnavailable, StageState,
};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tokio::sync::watch;

#[derive(Clone)]
pub struct FakeGit {
    state: Arc<Mutex<FakeGitState>>,
    changes: Arc<watch::Sender<()>>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FakeGitFile {
    pub path: String,
    pub contents: Option<Vec<u8>>,
    pub staged_contents: Option<Vec<u8>>,
    pub committed_contents: Option<Vec<u8>>,
}

pub fn elicitation_id() -> crate::elicitation::ElicitationId {
    crate::elicitation::ElicitationId(0)
}

pub fn client_info() -> Implementation {
    Implementation::new("client-core-test", "1")
}

pub fn git_diff_agent(agent: FakeAgent, git: FakeGit) -> V2Builder<Agent, impl HandleDispatchFrom<Client>, NullRun> {
    agent.git_diff().agent().with_handler(FakeGitDiffServer::new(git))
}

impl FakeGit {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self::with_state(FakeGitState { root: root.into(), is_repo: true, ..FakeGitState::default() })
    }

    pub fn not_a_repository(root: impl Into<PathBuf>) -> Self {
        Self::with_state(FakeGitState { root: root.into(), ..FakeGitState::default() })
    }

    #[must_use]
    pub fn with_untracked(self, path: impl Into<String>, contents: impl AsRef<[u8]>) -> Self {
        self.add_file(path, contents);
        self
    }

    #[must_use]
    pub fn with_modified(self, path: &str, committed: impl AsRef<[u8]>, contents: impl AsRef<[u8]>) -> Self {
        let git = self.with_committed(path, committed);
        git.write_file(path, contents);
        git
    }

    pub fn set_repository_available(&self, available: bool) {
        self.mutate(|state| state.is_repo = available);
    }

    pub fn root(&self) -> PathBuf {
        self.state.lock().unwrap().root.clone()
    }

    pub fn add_file(&self, path: impl Into<String>, contents: impl AsRef<[u8]>) {
        let path = path.into();
        let file = FakeGitFile {
            path: path.clone(),
            contents: Some(contents.as_ref().to_vec()),
            staged_contents: None,
            committed_contents: None,
        };
        self.mutate(|state| state.files.insert(path, file));
    }

    pub fn write_file(&self, path: impl Into<String>, contents: impl AsRef<[u8]>) {
        let path = path.into();
        self.mutate(|state| {
            let file = state.files.entry(path.clone()).or_insert_with(|| FakeGitFile {
                path,
                contents: None,
                staged_contents: None,
                committed_contents: None,
            });
            file.contents = Some(contents.as_ref().to_vec());
        });
    }

    pub fn remove_file(&self, path: &str) {
        self.mutate(|state| {
            if let Some(file) = state.files.get_mut(path) {
                file.contents = None;
            }
        });
    }

    pub fn stage_all(&self) {
        self.mutate(|state| {
            for file in state.files.values_mut() {
                file.staged_contents = file.contents.clone();
            }
        });
    }

    pub fn unstage_all(&self) {
        self.mutate(|state| {
            for file in state.files.values_mut() {
                file.staged_contents = file.committed_contents.clone();
            }
        });
    }

    pub fn commit(&self, message: impl Into<String>) -> Result<(), RemoteError> {
        let message = message.into();
        if message.trim().is_empty() {
            return Err(git_error("commit message must not be empty"));
        }
        self.mutate(|state| {
            if !state.files.values().any(|file| file.staged_contents != file.committed_contents) {
                return Err(git_error("git operation `commit` failed with status Some(1)"));
            }
            for file in state.files.values_mut() {
                if file.staged_contents != file.committed_contents {
                    file.committed_contents = file.staged_contents.clone();
                }
            }
            state.commits.push(message);
            Ok(())
        })
    }

    pub fn file(&self, path: &str) -> Option<FakeGitFile> {
        self.state.lock().unwrap().files.get(path).cloned()
    }

    pub fn commits(&self) -> Vec<String> {
        self.state.lock().unwrap().commits.clone()
    }

    pub fn status(&self, path: &str) -> Option<(FileStatus, StageState)> {
        self.state.lock().unwrap().files.get(path).and_then(status_of)
    }

    pub fn apply(&self, action: RepositoryAction) -> Result<(), RemoteError> {
        if !self.state.lock().unwrap().is_repo {
            return Err(not_repository_error());
        }
        match action {
            RepositoryAction::StagePaths(paths) => {
                for path in paths {
                    self.stage(path.as_str());
                }
                Ok(())
            }
            RepositoryAction::UnstagePaths(paths) => {
                for path in paths {
                    self.unstage(path.as_str());
                }
                Ok(())
            }
            RepositoryAction::StageAll => {
                self.stage_all();
                Ok(())
            }
            RepositoryAction::UnstageAll => {
                self.unstage_all();
                Ok(())
            }
            RepositoryAction::Commit { message } => self.commit(message),
            RepositoryAction::Discard { path, status } => {
                if status == FileStatus::Untracked {
                    self.mutate(|state| state.files.remove(path.as_str()));
                } else {
                    self.discard(path.as_str());
                }
                Ok(())
            }
        }
    }

    #[must_use]
    fn with_committed(self, path: &str, contents: impl AsRef<[u8]>) -> Self {
        self.add_file(path, contents);
        self.stage(path);
        self.commit(format!("add {path}")).expect("a staged file commits");
        self
    }

    fn stage(&self, path: &str) -> bool {
        self.mutate(|state| {
            let Some(file) = state.files.get_mut(path) else { return false };
            file.staged_contents = file.contents.clone();
            true
        })
    }

    fn unstage(&self, path: &str) -> bool {
        self.mutate(|state| {
            let Some(file) = state.files.get_mut(path) else { return false };
            file.staged_contents = file.committed_contents.clone();
            true
        })
    }

    fn discard(&self, path: &str) -> bool {
        self.mutate(|state| {
            let Some(file) = state.files.get_mut(path) else { return false };
            file.contents = file.committed_contents.clone();
            file.staged_contents = file.committed_contents.clone();
            true
        })
    }

    pub(crate) fn subscribe(&self) -> watch::Receiver<()> {
        self.changes.subscribe()
    }

    fn with_state(state: FakeGitState) -> Self {
        Self { state: Arc::new(Mutex::new(state)), changes: Arc::new(watch::Sender::new(())) }
    }

    fn mutate<T>(&self, change: impl FnOnce(&mut FakeGitState) -> T) -> T {
        let value = change(&mut self.state.lock().unwrap());
        self.changes.send_replace(());
        value
    }

    pub(crate) fn load_diff(&self, target: &ReviewTarget) -> Result<DiffDocument, RemoteError> {
        let state = self.state.lock().unwrap();
        if !state.is_repo {
            return Err(not_repository_error());
        }
        let Some(scope) = target.scope() else {
            return Err(git_error(format!("no commit {target:?} in the fake repository")));
        };
        let mut files = Vec::new();
        for file in state.files.values() {
            let (old, new) = match scope {
                DiffScope::Staged => (&file.committed_contents, &file.staged_contents),
                DiffScope::Unstaged => (&file.staged_contents, &file.contents),
                DiffScope::Both => (&file.committed_contents, &file.contents),
            };
            if old == new {
                continue;
            }
            let binary = old.as_ref().is_some_and(|bytes| is_binary(bytes))
                || new.as_ref().is_some_and(|bytes| is_binary(bytes));
            let mut diff = if binary {
                FileDiff::from_texts(file.path.clone(), "", "").map_err(|error| git_error(error.to_string()))?
            } else {
                let old_text = old.as_deref().map(String::from_utf8_lossy).unwrap_or_default();
                let new_text = new.as_deref().map(String::from_utf8_lossy).unwrap_or_default();
                FileDiff::from_texts(file.path.clone(), &old_text, &new_text)
                    .map_err(|error| git_error(error.to_string()))?
            };
            diff.status = match (old, new) {
                (None, Some(_)) if file.committed_contents.is_none() && file.staged_contents.is_none() => {
                    FileStatus::Untracked
                }
                (None, Some(_)) => FileStatus::Added,
                (Some(_), None) => FileStatus::Deleted,
                _ => FileStatus::Modified,
            };
            diff.old_path = old.is_some().then(|| diff.path.clone());
            diff.staged = status_of(file).map_or(StageState::Unstaged, |(_, stage)| stage);
            diff.binary = binary;
            diff = diff.with_sources(fake_source(old.as_deref()), fake_source(new.as_deref()));
            files.push(diff);
        }
        Ok(DiffDocument {
            repo_root: state.root.to_string_lossy().into_owned(),
            target: target.clone(),
            commits: None,
            files,
        })
    }
}

impl Default for FakeGit {
    fn default() -> Self {
        Self::with_state(FakeGitState::default())
    }
}

#[derive(Default)]
struct FakeGitState {
    root: PathBuf,
    files: BTreeMap<String, FakeGitFile>,
    commits: Vec<String>,
    is_repo: bool,
}

fn git_error(message: impl Into<String>) -> RemoteError {
    RemoteError::new(RemoteErrorCode::Git, message)
}

fn not_repository_error() -> RemoteError {
    git_error("path is not inside a Git worktree")
}

fn status_of(file: &FakeGitFile) -> Option<(FileStatus, StageState)> {
    let staged_changed = file.staged_contents != file.committed_contents;
    let working_changed = file.contents != file.staged_contents;
    if !staged_changed && !working_changed {
        return None;
    }

    if file.committed_contents.is_none() {
        return Some(match (file.staged_contents.is_some(), working_changed) {
            (true, true) => (FileStatus::Added, StageState::PartiallyStaged),
            (true, false) => (FileStatus::Added, StageState::Staged),
            (false, _) => (FileStatus::Untracked, StageState::Unstaged),
        });
    }

    let stage = match (staged_changed, working_changed) {
        (true, true) => StageState::PartiallyStaged,
        (true, false) => StageState::Staged,
        (false, true) => StageState::Unstaged,
        (false, false) => unreachable!("clean files returned above"),
    };
    let status = if file.contents.is_none() { FileStatus::Deleted } else { FileStatus::Modified };
    Some((status, stage))
}

fn fake_source(bytes: Option<&[u8]>) -> SourceResult {
    match bytes {
        None => Err(SourceUnavailable::Absent),
        Some(bytes) if is_binary(bytes) => Err(SourceUnavailable::Binary),
        Some(bytes) => SourceDocument::new(String::from_utf8_lossy(bytes)).map(Arc::new),
    }
}

fn is_binary(bytes: &[u8]) -> bool {
    bytes.iter().take(8192).any(|byte| *byte == 0) || std::str::from_utf8(bytes).is_err()
}
