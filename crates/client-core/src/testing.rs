//! In-memory fakes of the command runtime, for host test harnesses.

mod git_diff_server;
mod session_config;
pub use git_diff_server::FakeGitDiffServer;
pub use session_config::SessionCoreConfigBuilder;

use crate::command::{AgentCommand, Command, CommandResult, FilesystemCommand, GitReviewCommand};
use crate::composer::attachment::{AttachmentOutcome, PromptAttachment, build_attachments_with};
use crate::composer::file_index::{MAX_INDEXED_FILES, file_entries};
use acp_utils::notifications::WorkspaceStatusResponse;
use clankerdiff_client::protocol::server::ServerEvent;
use clankerdiff_client::protocol::shared::LIVE_PROTOCOL_VERSION;
use clankerdiff_client::{ClientState, ConnectionState, DiffSnapshot, RemoteError, RemoteErrorCode};
use clankerdiff_core::{
    DiffDocument, DiffReviewEvent, DiffScope, FileDiff, FileStatus, RepositoryAction, SourceDocument, SourceResult,
    SourceUnavailable, StageState,
};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

/// A deterministic command runner for integration tests.
///
/// Commands are recorded in submission order. Tests may enqueue completions for
/// commands whose results are part of the scenario; commands without a queued
/// completion are still recorded and produce no state change. Host commands
/// (`T`) are recorded but never completed.
pub struct FakeExecutor<T> {
    /// Commands not yet consumed by a test through `next_command`.
    available: VecDeque<Command<T>>,
    /// Commands not yet completed by settling.
    pending: VecDeque<Command<T>>,
    git: FakeGit,
    /// The review client's published state, or `None` when no review is attached.
    published: Option<Arc<ClientState>>,
    git_completion: Option<Result<(), String>>,
    filesystem: FakeFilesystem,
}

/// An in-memory filesystem used by command-oriented tests.
#[derive(Clone, Default)]
pub struct FakeFilesystem {
    files: BTreeMap<PathBuf, Vec<u8>>,
    directories: BTreeSet<PathBuf>,
}

/// A small stateful Git model. It keeps working-tree, index, and committed
/// snapshots separate so staging and discarding have observable semantics.
#[derive(Clone, Default)]
pub struct FakeGit {
    state: Arc<Mutex<FakeGitState>>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FakeGitFile {
    pub path: String,
    pub contents: Option<Vec<u8>>,
    pub staged_contents: Option<Vec<u8>>,
    pub committed_contents: Option<Vec<u8>>,
}

impl<T> Default for FakeExecutor<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T> FakeExecutor<T> {
    pub fn new() -> Self {
        Self::with_git(FakeGit::default())
    }

    pub fn with_git(git: FakeGit) -> Self {
        Self {
            available: VecDeque::new(),
            pending: VecDeque::new(),
            git,
            published: None,
            git_completion: None,
            filesystem: FakeFilesystem::default(),
        }
    }

    pub fn git(&self) -> &FakeGit {
        &self.git
    }

    /// Whether a review client is currently attached to the fake runtime.
    pub fn git_review_active(&self) -> bool {
        self.published.is_some()
    }

    pub fn git_mut(&mut self) -> &mut FakeGit {
        &mut self.git
    }

    pub fn filesystem_mut(&mut self) -> &mut FakeFilesystem {
        &mut self.filesystem
    }

    pub fn next_git_review_state(&mut self) -> Option<Arc<ClientState>> {
        self.sync_git_review()
    }

    pub fn take_commands(&mut self) -> Vec<Command<T>> {
        self.pending.clear();
        self.available.drain(..).collect()
    }

    /// The oldest recorded command a test has not consumed yet.
    pub fn next_command(&mut self) -> Option<Command<T>> {
        self.available.pop_front()
    }

    fn complete<U>(&mut self, command: Command<T>) -> Option<CommandResult<U>> {
        match command {
            Command::Agent(AgentCommand::FetchWorkspaceStatus { cwd, .. }) => {
                let status = self.git.workspace_status();
                Some(CommandResult::WorkspaceResolved { cwd: cwd.clone(), status })
            }
            Command::GitReview(GitReviewCommand::Open { .. }) => {
                self.git_completion = None;
                self.published = Some(Arc::new(ClientState::default().apply(&ServerEvent::Initialize {
                    protocol_version: LIVE_PROTOCOL_VERSION,
                    repository_root: self.git.root().to_string_lossy().into_owned(),
                })));
                self.next_git_review_state().map(CommandResult::GitReview)
            }
            Command::GitReview(GitReviewCommand::Event(event)) => self.on_review_event(event),
            Command::GitReview(GitReviewCommand::Close) => {
                self.published = None;
                self.git_completion = None;
                None
            }
            Command::Filesystem(FilesystemCommand::PrepareSubmission { attachments }) => {
                Some(CommandResult::SubmissionPrepared(self.filesystem.build_attachments(&attachments)))
            }
            Command::Filesystem(FilesystemCommand::IndexFiles { request_id, root }) => {
                Some(CommandResult::FilesIndexed { request_id, files: self.filesystem.index_files(&root) })
            }
            _ => None,
        }
    }

    fn on_review_event<U>(&mut self, event: DiffReviewEvent) -> Option<CommandResult<U>> {
        match event {
            DiffReviewEvent::SetScope(scope) => self.reload(scope),
            DiffReviewEvent::Refresh => {
                self.git_completion = Some(Ok(()));
                self.next_git_review_state().map(CommandResult::GitReview)
            }
            DiffReviewEvent::RepositoryAction(action) => {
                self.git_completion = Some(self.git.apply(action).map_err(|error| error.to_string()));
                self.next_git_review_state().map(CommandResult::GitReview)
            }
            DiffReviewEvent::Cancel | DiffReviewEvent::SubmitReview(_) | DiffReviewEvent::CopyFormattedReview(_) => {
                None
            }
        }
    }

    fn sync_git_review(&mut self) -> Option<Arc<ClientState>> {
        let current = self.published.clone()?;
        let scope = current.snapshot.as_ref().map_or(DiffScope::default(), |snapshot| snapshot.scope);
        match self.git.load_diff(scope) {
            Ok(snapshot) if current.snapshot.as_deref() != Some(&snapshot) => {
                self.publish(&current, &ServerEvent::Document(Arc::new(snapshot)))
            }
            Ok(_) if current.error.is_some() => self.publish(&current, &ServerEvent::Health { error: None }),
            Ok(_) => None,
            Err(error) if current.error.as_ref() == Some(&error) => None,
            Err(error) if current.snapshot.is_none() && !matches!(current.connection, ConnectionState::Failed(_)) => {
                self.publish(&current, &ServerEvent::Error(error))
            }
            Err(error) => self.publish(&current, &ServerEvent::Health { error: Some(error) }),
        }
    }

    fn publish(&mut self, current: &Arc<ClientState>, event: &ServerEvent) -> Option<Arc<ClientState>> {
        let next = current.as_ref().clone().apply(event);
        if &next == current.as_ref() {
            return None;
        }
        let next = Arc::new(next);
        self.published = Some(Arc::clone(&next));
        Some(next)
    }

    fn reload<U>(&mut self, scope: DiffScope) -> Option<CommandResult<U>> {
        let current = self.published.clone()?;
        match self.git.load_diff(scope) {
            Ok(snapshot) => {
                self.git_completion = Some(Ok(()));
                self.publish(&current, &ServerEvent::Document(Arc::new(snapshot))).map(CommandResult::GitReview)
            }
            Err(error) => {
                self.git_completion = Some(Err(error.to_string()));
                None
            }
        }
    }
}

impl<T: Clone> FakeExecutor<T> {
    pub fn record(&mut self, commands: impl IntoIterator<Item = Command<T>>) {
        for command in commands {
            self.available.push_back(command.clone());
            self.pending.push_back(command);
        }
    }

    /// Completes every recorded command without touching the real filesystem,
    /// Git repository, terminal, or ACP connection, handing each result to
    /// `deliver`. The commands `deliver` returns, which the host emitted while
    /// handling that result, are recorded and completed in a later batch.
    /// Once nothing is pending, the remaining review state and repository
    /// action outcome are delivered too.
    ///
    /// Commands left unconsumed are dropped, except that agent commands in the
    /// first settled batch stay available, so a test can still inspect what the
    /// settle sent to the agent.
    pub fn settle<U>(&mut self, mut deliver: impl FnMut(CommandResult<U>) -> Vec<Command<T>>) {
        self.available.clear();
        let mut first_batch = true;
        loop {
            let batch = std::mem::take(&mut self.pending);
            if batch.is_empty() {
                let review = self.next_git_review_state().map(CommandResult::GitReview);
                let Some(result) = review.or_else(|| self.git_completion.take().map(CommandResult::GitReviewAction))
                else {
                    return;
                };
                self.record(deliver(result));
                continue;
            }
            if std::mem::take(&mut first_batch) {
                let agent_commands = batch.iter().filter(|command| matches!(command, Command::Agent(_)));
                self.available.extend(agent_commands.cloned());
            }
            for command in batch {
                if let Some(result) = self.complete(command) {
                    self.record(deliver(result));
                }
            }
        }
    }
}

impl FakeFilesystem {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn write_file(&mut self, path: impl Into<PathBuf>, contents: impl AsRef<[u8]>) {
        let path = path.into();
        if let Some(parent) = path.parent() {
            self.directories.insert(parent.to_path_buf());
        }
        self.files.insert(path, contents.as_ref().to_vec());
    }

    /// The runtime's file index over the in-memory tree, minus gitignore
    /// semantics, which need a real repository.
    pub fn index_files(&self, root: &Path) -> Vec<PromptAttachment> {
        let paths = self.files.keys().filter(|path| path.starts_with(root)).cloned();
        file_entries(root, paths, MAX_INDEXED_FILES)
    }

    /// The runtime's attachment preparation over in-memory contents: the same
    /// pure encoding as the real reader, with reads answered from this tree.
    pub fn build_attachments(&self, attachments: &[PromptAttachment]) -> AttachmentOutcome {
        build_attachments_with(attachments, |path, display_name| {
            if self.directories.contains(path) {
                return Err(format!("Failed to read {display_name}: is a directory"));
            }
            self.files.get(path).cloned().ok_or_else(|| format!("Failed to read {display_name}: file not found"))
        })
    }
}

impl FakeGit {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        let state = FakeGitState { root: root.into(), is_repo: true, ..FakeGitState::default() };
        Self { state: Arc::new(Mutex::new(state)) }
    }

    pub fn not_a_repository(root: impl Into<PathBuf>) -> Self {
        let state = FakeGitState { root: root.into(), ..FakeGitState::default() };
        Self { state: Arc::new(Mutex::new(state)) }
    }

    pub fn set_repository_available(&mut self, available: bool) {
        self.state.lock().unwrap().is_repo = available;
    }

    pub fn root(&self) -> PathBuf {
        self.state.lock().unwrap().root.clone()
    }

    pub fn add_file(&mut self, path: impl Into<String>, contents: impl AsRef<[u8]>) {
        let path = path.into();
        self.state.lock().unwrap().files.insert(
            path.clone(),
            FakeGitFile {
                path,
                contents: Some(contents.as_ref().to_vec()),
                staged_contents: None,
                committed_contents: None,
            },
        );
    }

    pub fn write_file(&mut self, path: impl Into<String>, contents: impl AsRef<[u8]>) {
        let path = path.into();
        let mut state = self.state.lock().unwrap();
        let file = state.files.entry(path.clone()).or_insert_with(|| FakeGitFile {
            path,
            contents: None,
            staged_contents: None,
            committed_contents: None,
        });
        file.contents = Some(contents.as_ref().to_vec());
    }

    pub fn remove_file(&mut self, path: &str) {
        if let Some(file) = self.state.lock().unwrap().files.get_mut(path) {
            file.contents = None;
        }
    }

    pub fn stage(&mut self, path: &str) -> bool {
        let mut state = self.state.lock().unwrap();
        let Some(file) = state.files.get_mut(path) else { return false };
        file.staged_contents = file.contents.clone();
        true
    }

    pub fn unstage(&mut self, path: &str) -> bool {
        let mut state = self.state.lock().unwrap();
        let Some(file) = state.files.get_mut(path) else { return false };
        file.staged_contents = file.committed_contents.clone();
        true
    }

    pub fn stage_all(&mut self) {
        let mut state = self.state.lock().unwrap();
        for file in state.files.values_mut() {
            file.staged_contents = file.contents.clone();
        }
    }

    pub fn unstage_all(&mut self) {
        let mut state = self.state.lock().unwrap();
        for file in state.files.values_mut() {
            file.staged_contents = file.committed_contents.clone();
        }
    }

    pub fn discard(&mut self, path: &str) -> bool {
        let mut state = self.state.lock().unwrap();
        let Some(file) = state.files.get_mut(path) else { return false };
        file.contents = file.committed_contents.clone();
        file.staged_contents = file.committed_contents.clone();
        true
    }

    pub fn commit(&mut self, message: impl Into<String>) -> Result<(), RemoteError> {
        let message = message.into();
        let mut state = self.state.lock().unwrap();
        if message.trim().is_empty() {
            return Err(git_error("commit message must not be empty"));
        }
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

    pub fn apply(&mut self, action: RepositoryAction) -> Result<(), RemoteError> {
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
                    self.state.lock().unwrap().files.remove(path.as_str());
                } else {
                    self.discard(path.as_str());
                }
                Ok(())
            }
        }
    }

    pub fn workspace_status(&self) -> WorkspaceStatusResponse {
        let state = self.state.lock().unwrap();
        WorkspaceStatusResponse {
            display_dir: state.root.to_string_lossy().into_owned(),
            git_ref: Some("main".to_string()),
        }
    }

    fn load_diff(&self, scope: DiffScope) -> Result<DiffSnapshot, RemoteError> {
        let state = self.state.lock().unwrap();
        if !state.is_repo {
            return Err(not_repository_error());
        }
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
        let document = DiffDocument { repo_root: state.root.to_string_lossy().into_owned(), files };
        Ok(DiffSnapshot { scope, document: Arc::new(document) })
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
        let stage = match (file.staged_contents.is_some(), working_changed) {
            (true, true) => StageState::PartiallyStaged,
            (true, false) => StageState::Staged,
            (false, _) => StageState::Unstaged,
        };
        return Some((FileStatus::Untracked, stage));
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::request::RequestId;

    #[test]
    fn fake_executor_preserves_command_order() {
        let mut executor = FakeExecutor::new();
        executor.record([
            Command::Filesystem(FilesystemCommand::IndexFiles { request_id: RequestId::next(), root: "/".into() }),
            Command::Host("ring bell"),
        ]);

        assert!(matches!(executor.take_commands()[..], [Command::Filesystem(_), Command::Host(_)]));
    }

    #[test]
    fn fake_git_models_staging_and_discarding_state() {
        let mut git = FakeGit::new("workspace");
        git.add_file("src/main.rs", "initial\n");
        assert_eq!(git.status("src/main.rs"), Some((FileStatus::Untracked, StageState::Unstaged)));

        git.stage("src/main.rs");
        assert_eq!(git.status("src/main.rs"), Some((FileStatus::Untracked, StageState::Staged)));
        git.commit("initial").unwrap();

        git.write_file("src/main.rs", "changed\n");
        assert_eq!(git.status("src/main.rs"), Some((FileStatus::Modified, StageState::Unstaged)));
        git.discard("src/main.rs");
        assert_eq!(git.file("src/main.rs").and_then(|file| file.contents), Some(b"initial\n".to_vec()));
    }
}
