//! Public testing harness for driving the application model without side effects.
//!
//! Prefer [`TestUi`] over reaching for [`App`], [`Renderer`], or a raw
//! `Terminal<TestBackend>`: it owns all of them and routes input, ACP events,
//! task settling, and drawing through the same seams the event loop uses.

use crate::app::message::Message;
use crate::app::{App, AppConfig};
use crate::attachment::{AttachmentOutcome, PromptAttachment, build_attachments_with};
use crate::command::{AgentCommand, Command, CommandResult, FilesystemCommand, GitCommand};
use crate::file_index::{FileEntry, MAX_INDEXED_FILES, file_entries};
use crate::git_review::{
    DiffDocument, DiffScope, FileDiff, FileStatus, GitDiffError, GitDiffEvent, StageState, build_untracked_file_diff,
};
pub use crate::renderer::RenderStats;
use crate::renderer::Renderer;
use crate::session::platform::BrowserOpener;
use crate::session::terminal::inline_viewport_height;
use crate::session::workspace_status::WorkspaceStatus;
use crate::settings::UiSettings;
use crate::surfaces::composer::ComposerLayout;
use acp_utils::AETHER_TOOL_NAME_META_KEY;
use acp_utils::client::AcpEvent;
use acp_utils::notifications::{
    AetherCapabilities, SubAgentEvent, SubAgentProgressParams, SubAgentToolRequest, SubAgentToolResult,
};
use agent_client_protocol::schema::v1::{self as acp, SessionId};
use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
use ratatui::backend::{Backend, ClearType, TestBackend, WindowSize};
use ratatui::buffer::{Buffer, Cell};
use ratatui::layout::{Position, Rect, Size};
use ratatui::{Terminal, TerminalOptions, Viewport};
use serde_json::json;
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// A deterministic command runner for integration tests.
///
/// Commands are recorded in submission order. Tests may enqueue completions for
/// commands whose results are part of the scenario; commands without a queued
/// completion are still recorded and produce no state change.
pub struct FakeExecutor {
    /// Commands not yet consumed by a test through `next_command`.
    available: VecDeque<Command>,
    /// Commands not yet completed by `settle_tasks`.
    pending: VecDeque<Command>,
    git: FakeGit,
    filesystem: FakeFilesystem,
}

impl Default for FakeExecutor {
    fn default() -> Self {
        Self::new()
    }
}

impl FakeExecutor {
    pub fn new() -> Self {
        Self::with_git(FakeGit::default())
    }

    pub fn with_git(git: FakeGit) -> Self {
        Self { available: VecDeque::new(), pending: VecDeque::new(), git, filesystem: FakeFilesystem::default() }
    }

    pub fn git(&self) -> &FakeGit {
        &self.git
    }

    pub fn git_mut(&mut self) -> &mut FakeGit {
        &mut self.git
    }

    pub fn filesystem(&self) -> &FakeFilesystem {
        &self.filesystem
    }

    pub fn filesystem_mut(&mut self) -> &mut FakeFilesystem {
        &mut self.filesystem
    }

    pub fn record(&mut self, commands: impl IntoIterator<Item = Command>) {
        for command in commands {
            self.available.push_back(command.clone());
            self.pending.push_back(command);
        }
    }

    fn complete(&mut self, command: Command) -> Option<CommandResult> {
        match command {
            Command::ResolveWorkspace { cwd } => Some(CommandResult::WorkspaceResolved {
                status: WorkspaceStatus::new(cwd.display().to_string(), None),
                cwd,
            }),
            Command::Git(command) => Some(CommandResult::GitDiff(self.git.apply(command))),
            Command::Filesystem(FilesystemCommand::PrepareSubmission { attachments }) => {
                Some(CommandResult::SubmissionPrepared(self.filesystem.build_attachments(&attachments)))
            }
            Command::Filesystem(FilesystemCommand::IndexFiles { request_id, root }) => {
                Some(CommandResult::FilesIndexed { request_id, files: self.filesystem.index_files(&root) })
            }
            _ => None,
        }
    }

    fn take_pending(&mut self) -> Vec<Command> {
        self.pending.drain(..).collect()
    }

    fn clear_available(&mut self) {
        self.available.clear();
    }

    pub fn take_commands(&mut self) -> Vec<Command> {
        self.pending.clear();
        self.available.drain(..).collect()
    }
}

/// An in-memory filesystem used by command-oriented tests.
#[derive(Clone, Default)]
pub struct FakeFilesystem {
    files: BTreeMap<PathBuf, Vec<u8>>,
    directories: BTreeSet<PathBuf>,
    settings: Option<UiSettings>,
}

impl FakeFilesystem {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn create_dir(&mut self, path: impl Into<PathBuf>) {
        self.directories.insert(path.into());
    }

    pub fn write_file(&mut self, path: impl Into<PathBuf>, contents: impl AsRef<[u8]>) {
        let path = path.into();
        if let Some(parent) = path.parent() {
            self.directories.insert(parent.to_path_buf());
        }
        self.files.insert(path, contents.as_ref().to_vec());
    }

    pub fn remove_file(&mut self, path: &Path) -> bool {
        self.files.remove(path).is_some()
    }

    pub fn read_file(&self, path: &Path) -> Option<&[u8]> {
        self.files.get(path).map(Vec::as_slice)
    }

    pub fn read_to_string(&self, path: &Path) -> Option<String> {
        self.read_file(path).and_then(|contents| String::from_utf8(contents.to_vec()).ok())
    }

    pub fn contains(&self, path: &Path) -> bool {
        self.files.contains_key(path) || self.directories.contains(path)
    }

    pub fn files(&self) -> impl Iterator<Item = (&Path, &[u8])> {
        self.files.iter().map(|(path, contents)| (path.as_path(), contents.as_slice()))
    }

    pub fn directories(&self) -> impl Iterator<Item = &Path> {
        self.directories.iter().map(PathBuf::as_path)
    }

    pub fn save_settings(&mut self, settings: UiSettings) {
        self.settings = Some(settings);
    }

    pub fn settings(&self) -> Option<&UiSettings> {
        self.settings.as_ref()
    }

    /// The runtime's file index over the in-memory tree, minus gitignore
    /// semantics, which need a real repository.
    pub fn index_files(&self, root: &Path) -> Vec<FileEntry> {
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

/// A small stateful Git model. It keeps working-tree, index, and committed
/// snapshots separate so staging and discarding have observable semantics.
#[derive(Clone, Default)]
pub struct FakeGit {
    state: std::sync::Arc<std::sync::Mutex<FakeGitState>>,
}

#[derive(Default)]
struct FakeGitState {
    root: PathBuf,
    files: BTreeMap<String, FakeGitFile>,
    commits: Vec<String>,
    is_repo: bool,
    commit_error: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FakeGitFile {
    pub path: String,
    pub contents: Option<Vec<u8>>,
    pub staged_contents: Option<Vec<u8>>,
    pub committed_contents: Option<Vec<u8>>,
}

impl FakeGit {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        let state = FakeGitState { root: root.into(), is_repo: true, ..FakeGitState::default() };
        Self { state: std::sync::Arc::new(std::sync::Mutex::new(state)) }
    }

    pub fn not_a_repository(root: impl Into<PathBuf>) -> Self {
        let state = FakeGitState { root: root.into(), ..FakeGitState::default() };
        Self { state: std::sync::Arc::new(std::sync::Mutex::new(state)) }
    }

    pub fn fail_next_commit(&mut self, error: impl Into<String>) {
        self.state.lock().unwrap().commit_error = Some(error.into());
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

    pub fn commit(&mut self, message: impl Into<String>) -> Result<(), String> {
        let message = message.into();
        let mut state = self.state.lock().unwrap();
        if message.trim().is_empty() {
            return Err("empty commit message".to_string());
        }
        if !state.files.values().any(|file| file.staged_contents != file.committed_contents) {
            return Err("nothing to commit".to_string());
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

    pub fn files(&self) -> Vec<FakeGitFile> {
        self.state.lock().unwrap().files.values().cloned().collect()
    }

    pub fn commits(&self) -> Vec<String> {
        self.state.lock().unwrap().commits.clone()
    }

    pub fn status(&self, path: &str) -> Option<(FileStatus, StageState)> {
        self.state.lock().unwrap().files.get(path).and_then(status_of)
    }

    fn load_diff(&self, scope: DiffScope) -> Result<DiffDocument, GitDiffError> {
        let state = self.state.lock().unwrap();
        if !state.is_repo {
            return Err(GitDiffError::NotARepository);
        }

        let mut files = Vec::new();
        for file in state.files.values() {
            let untracked = file.committed_contents.is_none() && file.staged_contents.is_none();
            if untracked {
                if scope.includes_untracked()
                    && let Some(contents) = &file.contents
                {
                    files.push(build_untracked_file_diff(file.path.clone(), contents));
                }
                continue;
            }

            let (old, new) = match scope {
                DiffScope::Staged => (&file.committed_contents, &file.staged_contents),
                DiffScope::Unstaged => {
                    let old =
                        if file.staged_contents.is_some() { &file.staged_contents } else { &file.committed_contents };
                    (old, &file.contents)
                }
                DiffScope::Both => (&file.committed_contents, &file.contents),
            };
            if old == new {
                continue;
            }

            let staged = status_of(file).map_or(StageState::Unstaged, |(_, stage)| stage);
            let binary = old.as_ref().is_some_and(|bytes| is_binary(bytes))
                || new.as_ref().is_some_and(|bytes| is_binary(bytes));
            if binary {
                let status = match (old, new) {
                    (None, Some(_)) => FileStatus::Added,
                    (Some(_), None) => FileStatus::Deleted,
                    _ => FileStatus::Modified,
                };
                files.push(FileDiff {
                    old_path: (status != FileStatus::Added).then(|| file.path.clone()),
                    path: file.path.clone(),
                    status,
                    staged,
                    hunks: Vec::new(),
                    binary: true,
                });
                continue;
            }

            let old_text = old.as_deref().map(bytes_to_text).transpose()?.unwrap_or_default();
            let new_text = new.as_deref().map(bytes_to_text).transpose()?.unwrap_or_default();
            let mut diff = FileDiff::from_texts(file.path.clone(), &old_text, &new_text);
            diff.staged = staged;
            files.push(diff);
        }
        files.sort_by(|left, right| left.path.cmp(&right.path));
        Ok(DiffDocument { repo_root: state.root.clone(), files })
    }

    fn read_full_file(&self, path: &str) -> Result<String, GitDiffError> {
        let state = self.state.lock().unwrap();
        let Some(contents) = state.files.get(path).and_then(|file| {
            file.contents.as_deref().or(file.staged_contents.as_deref()).or(file.committed_contents.as_deref())
        }) else {
            return Err(GitDiffError::CommandFailed { stderr: format!("Cannot read {path}: file not found") });
        };
        String::from_utf8(contents.to_vec())
            .map_err(|error| GitDiffError::CommandFailed { stderr: format!("Cannot read {path}: {error}") })
    }

    fn apply(&mut self, command: GitCommand) -> GitDiffEvent {
        match command {
            GitCommand::Load { request_id, scope, .. } => {
                GitDiffEvent::Loaded { request_id, result: self.load_diff(scope) }
            }
            GitCommand::StageFiles { request_id, paths, .. } => {
                for path in paths {
                    self.stage(&path);
                }
                GitDiffEvent::ActionFinished { request_id, result: Ok(()) }
            }
            GitCommand::UnstageFiles { request_id, paths, .. } => {
                for path in paths {
                    self.unstage(&path);
                }
                GitDiffEvent::ActionFinished { request_id, result: Ok(()) }
            }
            GitCommand::StageAll { request_id, .. } => {
                self.stage_all();
                GitDiffEvent::ActionFinished { request_id, result: Ok(()) }
            }
            GitCommand::UnstageAll { request_id, .. } => {
                self.unstage_all();
                GitDiffEvent::ActionFinished { request_id, result: Ok(()) }
            }
            GitCommand::Commit { request_id, message, .. } => {
                let error = self.state.lock().unwrap().commit_error.take();
                let result = error.map_or_else(
                    || self.commit(message).map_err(|stderr| GitDiffError::CommandFailed { stderr }),
                    |stderr| Err(GitDiffError::CommandFailed { stderr }),
                );
                GitDiffEvent::ActionFinished { request_id, result }
            }
            GitCommand::DiscardFile { request_id, path, status, .. } => {
                if status == FileStatus::Untracked {
                    self.state.lock().unwrap().files.remove(&path);
                } else {
                    self.discard(&path);
                }
                GitDiffEvent::ActionFinished { request_id, result: Ok(()) }
            }
            GitCommand::LoadFullFile { request_id, path, .. } => {
                GitDiffEvent::FullFileLoaded { request_id, result: self.read_full_file(&path), path }
            }
        }
    }
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

fn is_binary(bytes: &[u8]) -> bool {
    bytes.iter().take(8192).any(|byte| *byte == 0) || std::str::from_utf8(bytes).is_err()
}

fn bytes_to_text(bytes: &[u8]) -> Result<String, GitDiffError> {
    std::str::from_utf8(bytes)
        .map(str::to_string)
        .map_err(|error| GitDiffError::CommandFailed { stderr: error.to_string() })
}

/// Deterministic terminal wrapper used by focused golden tests.
pub struct TestTerminal {
    terminal: Terminal<TestBackend>,
}

impl TestTerminal {
    pub fn new(width: u16, height: u16) -> Self {
        Self { terminal: test_terminal(TestBackend::new(width, height)) }
    }

    pub fn terminal(&mut self) -> &mut Terminal<TestBackend> {
        &mut self.terminal
    }

    pub fn resize(&mut self, width: u16, height: u16) {
        self.terminal.backend_mut().resize(width, height);
    }

    pub fn viewport(&mut self) -> Buffer {
        viewport_buffer(&mut self.terminal)
    }

    pub fn history(&mut self) -> Buffer {
        history_buffer(&mut self.terminal)
    }

    pub fn conversation(&mut self) -> Buffer {
        conversation_buffer(&mut self.terminal)
    }
}

/// A backend that records terminal history operations without involving a real
/// terminal. It is intentionally small: only operations with an observable
/// presentation contract are recorded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackendEvent {
    ShowCursor,
    Scroll,
}

#[derive(Debug)]
pub struct RecordingBackend {
    inner: TestBackend,
    events: Vec<BackendEvent>,
}

impl RecordingBackend {
    pub fn new(width: u16, height: u16) -> Self {
        Self { inner: TestBackend::new(width, height), events: Vec::new() }
    }

    pub fn events(&self) -> &[BackendEvent] {
        &self.events
    }

    pub fn clear_events(&mut self) {
        self.events.clear();
    }

    pub fn resize(&mut self, width: u16, height: u16) {
        self.inner.resize(width, height);
    }

    pub fn buffer(&self) -> &Buffer {
        self.inner.buffer()
    }

    pub fn scrollback(&self) -> &Buffer {
        self.inner.scrollback()
    }
}

impl Backend for RecordingBackend {
    type Error = std::convert::Infallible;

    fn draw<'a, I>(&mut self, content: I) -> Result<(), Self::Error>
    where
        I: Iterator<Item = (u16, u16, &'a Cell)>,
    {
        self.inner.draw(content)
    }

    fn append_lines(&mut self, lines: u16) -> Result<(), Self::Error> {
        self.inner.append_lines(lines)
    }

    fn hide_cursor(&mut self) -> Result<(), Self::Error> {
        self.inner.hide_cursor()
    }

    fn show_cursor(&mut self) -> Result<(), Self::Error> {
        self.events.push(BackendEvent::ShowCursor);
        self.inner.show_cursor()
    }

    fn get_cursor_position(&mut self) -> Result<Position, Self::Error> {
        self.inner.get_cursor_position()
    }

    fn set_cursor_position<P: Into<Position>>(&mut self, position: P) -> Result<(), Self::Error> {
        self.inner.set_cursor_position(position)
    }

    fn clear(&mut self) -> Result<(), Self::Error> {
        self.inner.clear()
    }

    fn clear_region(&mut self, clear_type: ClearType) -> Result<(), Self::Error> {
        self.inner.clear_region(clear_type)
    }

    fn size(&self) -> Result<Size, Self::Error> {
        self.inner.size()
    }

    fn window_size(&mut self) -> Result<WindowSize, Self::Error> {
        self.inner.window_size()
    }

    fn flush(&mut self) -> Result<(), Self::Error> {
        self.inner.flush()
    }

    fn scroll_region_up(&mut self, region: std::ops::Range<u16>, lines: u16) -> Result<(), Self::Error> {
        self.events.push(BackendEvent::Scroll);
        let region = if region == (0..1) { 0..self.inner.size().unwrap().height } else { region };
        self.inner.scroll_region_up(region, lines)
    }

    fn scroll_region_down(&mut self, region: std::ops::Range<u16>, lines: u16) -> Result<(), Self::Error> {
        self.events.push(BackendEvent::Scroll);
        self.inner.scroll_region_down(region, lines)
    }
}

/// Backend work since the last [`CountingBackend::take_stats`]: how much a
/// frame pushed at the terminal. Because ratatui diffs buffers before flushing,
/// `cells_drawn` counts only cells that actually changed — a frame that
/// repaints a settled screen costs nothing.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct BackendStats {
    pub draws: u64,
    pub cells_drawn: u64,
    pub scrolls: u64,
}

/// A [`TestBackend`] that counts what rendering pushes at it, for tests that
/// bound rendering work instead of timing it.
#[derive(Debug)]
pub struct CountingBackend {
    inner: TestBackend,
    stats: BackendStats,
}

impl CountingBackend {
    pub fn new(width: u16, height: u16) -> Self {
        Self { inner: TestBackend::new(width, height), stats: BackendStats::default() }
    }

    pub fn take_stats(&mut self) -> BackendStats {
        std::mem::take(&mut self.stats)
    }

    pub fn buffer(&self) -> &Buffer {
        self.inner.buffer()
    }

    pub fn scrollback(&self) -> &Buffer {
        self.inner.scrollback()
    }
}

impl Backend for CountingBackend {
    type Error = std::convert::Infallible;

    fn draw<'a, I>(&mut self, content: I) -> Result<(), Self::Error>
    where
        I: Iterator<Item = (u16, u16, &'a Cell)>,
    {
        self.stats.draws += 1;
        let mut cells = 0u64;
        let drawn = self.inner.draw(content.inspect(|_| cells += 1));
        self.stats.cells_drawn += cells;
        drawn
    }

    fn hide_cursor(&mut self) -> Result<(), Self::Error> {
        self.inner.hide_cursor()
    }

    fn show_cursor(&mut self) -> Result<(), Self::Error> {
        self.inner.show_cursor()
    }

    fn get_cursor_position(&mut self) -> Result<Position, Self::Error> {
        self.inner.get_cursor_position()
    }

    fn set_cursor_position<P: Into<Position>>(&mut self, position: P) -> Result<(), Self::Error> {
        self.inner.set_cursor_position(position)
    }

    fn clear(&mut self) -> Result<(), Self::Error> {
        self.inner.clear()
    }

    fn clear_region(&mut self, clear_type: ClearType) -> Result<(), Self::Error> {
        self.inner.clear_region(clear_type)
    }

    fn size(&self) -> Result<Size, Self::Error> {
        self.inner.size()
    }

    fn window_size(&mut self) -> Result<WindowSize, Self::Error> {
        self.inner.window_size()
    }

    fn flush(&mut self) -> Result<(), Self::Error> {
        self.inner.flush()
    }

    fn scroll_region_up(&mut self, region: std::ops::Range<u16>, lines: u16) -> Result<(), Self::Error> {
        self.stats.scrolls += 1;
        self.inner.scroll_region_up(region, lines)
    }

    fn scroll_region_down(&mut self, region: std::ops::Range<u16>, lines: u16) -> Result<(), Self::Error> {
        self.stats.scrolls += 1;
        self.inner.scroll_region_down(region, lines)
    }
}

/// A whole UI scenario: the app, the renderer that owns its scrollback, the
/// terminal it draws into, and the receiver for the commands the app sends.
///
/// Construct through [`TestUi::new`], [`TestUi::with_dimensions`], or
/// [`TestUiBuilder`]. Input, ACP events, ticks, and task results are routed the
/// same way the event loop routes them, and drawing happens against the same
/// renderer instance every frame so committed scrollback survives.
pub struct TestUi<B: Backend = TestBackend> {
    app: App,
    renderer: Renderer,
    terminal: Terminal<B>,
    executor: FakeExecutor,
    opened_urls: Arc<Mutex<Vec<String>>>,
}

impl<B: Backend> TestUi<B>
where
    B::Error: std::fmt::Debug,
{
    /// Builds a UI around an arbitrary backend (e.g. a recording backend that
    /// asserts on frame-level terminal commands). The app is the default
    /// scenario; configure anything else through [`TestUiBuilder`].
    pub fn with_backend(backend: B) -> Self {
        let builder = TestUiBuilder::new();
        let app = App::new(builder.app_config());
        Self {
            app,
            renderer: Renderer::new(),
            terminal: test_terminal(backend),
            executor: FakeExecutor::new(),
            opened_urls: builder.opened_urls.clone(),
        }
    }

    /// The app under test, for the read-only queries assertions are built from.
    /// Drive it through this harness rather than mutating it directly, so the
    /// commands it emits still reach the fake runtime.
    pub fn app(&self) -> &App {
        &self.app
    }

    /// Delivers one message through the public application boundary and records
    /// every command it emits in the fake runtime.
    pub fn deliver(&mut self, message: Message) {
        self.executor.record(self.app.update(message));
    }

    pub fn deliver_result(&mut self, result: CommandResult) {
        self.deliver(Message::CommandFinished(result));
    }

    /// Commands emitted by the public application boundary, in order.
    pub fn executor(&self) -> &FakeExecutor {
        &self.executor
    }

    pub fn executor_mut(&mut self) -> &mut FakeExecutor {
        &mut self.executor
    }

    pub fn take_commands(&mut self) -> Vec<Command> {
        self.executor.take_commands()
    }

    pub fn next_command(&mut self) -> Option<Command> {
        self.executor.available.pop_front()
    }

    pub fn next_agent_command(&mut self) -> Option<AgentCommand> {
        while let Some(command) = self.next_command() {
            if let Command::Agent(command) = command {
                return Some(command);
            }
        }
        None
    }

    pub fn backend(&self) -> &B {
        self.terminal.backend()
    }

    pub fn backend_mut(&mut self) -> &mut B {
        self.terminal.backend_mut()
    }

    pub fn viewport_area(&mut self) -> ratatui::layout::Rect {
        self.terminal.get_frame().area()
    }

    pub fn viewport_height(&mut self) -> u16 {
        self.viewport_area().height
    }

    /// Measures the composer at `width` with the active theme.
    pub fn composer_layout(&mut self, width: u16) -> ComposerLayout {
        let theme = self.app.theme().clone();
        let composer = self.app.composer_mut();
        composer.on_resize(width);
        composer.layout(width, &theme)
    }

    /// Draws one frame, like the event loop does after every input batch.
    pub fn draw(&mut self) {
        self.renderer.draw(&mut self.terminal, &mut self.app).unwrap();
    }

    pub fn render_stats(&mut self) -> RenderStats {
        self.renderer.take_stats()
    }

    /// URLs elicitation prompts asked to open, in order. The harness records
    /// them instead of spawning a browser, so no test touches the host.
    pub fn opened_urls(&self) -> Vec<String> {
        self.opened_urls.lock().unwrap().clone()
    }

    /// Seeds a deterministic long conversation: `turns` completed turns of
    /// user messages, thoughts, markdown prose with fenced code, bash and edit
    /// tool calls with diffs, and a sub-agent tree every eighth turn — drawing
    /// after each turn so scrollback commits exactly like a live session.
    pub fn seed_long_history(&mut self, turns: usize) {
        for turn in 0..turns {
            self.acp_event(user_chunk(&format!(
                "Turn {turn}: reconcile the writer path in module_{turn} and add a regression test."
            )));
            self.acp_event(thought_chunk(&format!(
                "Reading module_{turn} to find the torn-update window before touching any call site."
            )));
            self.acp_event(text_chunk(SEED_PROSE));
            self.acp_event(text_chunk(SEED_CODE_BLOCK));
            let bash = format!("seed-bash-{turn}");
            self.acp_event(seed_bash_tool(&bash));
            self.acp_event(tool_completed(&bash));
            let edit = format!("seed-edit-{turn}");
            self.acp_event(seed_edit_tool(&edit, turn));
            self.acp_event(seed_tool_diff(&edit, turn));
            if turn % 8 == 0 {
                self.seed_sub_agent_tree(turn);
            }
            self.acp_event(text_chunk(SEED_CLOSING));
            self.acp_event(AcpEvent::PromptDone(acp::StopReason::EndTurn));
            self.draw();
        }
    }

    /// One spawn tool whose sub-agents run and finish, leaving a sealed tree.
    fn seed_sub_agent_tree(&mut self, turn: usize) {
        let parent = format!("seed-spawn-{turn}");
        self.acp_event(seed_spawn_tool(&parent));
        self.acp_event(tool_completed(&parent));
        for agent in ["explorer", "fixer"] {
            let task = format!("{parent}-{agent}");
            self.acp_event(seed_sub_agent(
                &parent,
                &task,
                agent,
                SubAgentEvent::ToolCall {
                    request: SubAgentToolRequest {
                        id: format!("{task}-grep"),
                        name: "grep".to_string(),
                        arguments: r#"{"pattern":"torn update"}"#.to_string(),
                    },
                },
            ));
            self.acp_event(seed_sub_agent(
                &parent,
                &task,
                agent,
                SubAgentEvent::ToolResult {
                    result: SubAgentToolResult {
                        id: format!("{task}-grep"),
                        name: "grep".to_string(),
                        result_meta: None,
                    },
                },
            ));
            self.acp_event(seed_sub_agent(&parent, &task, agent, SubAgentEvent::Done));
        }
    }

    /// Streams one assistant message of `total_bytes` in `chunk_bytes` chunks,
    /// drawing after every chunk the way the event loop draws after every
    /// wakeup. The message stays one open item while it streams.
    pub fn stream_message(&mut self, content: StreamContent, total_bytes: usize, chunk_bytes: usize) {
        let thought = matches!(content, StreamContent::Thought);
        let message = match content {
            StreamContent::Prose => prose_message(total_bytes),
            StreamContent::CodeBlock => code_block_message(total_bytes),
            StreamContent::Thought => thought_message(total_bytes),
        };
        for chunk in chunk_message(&message, chunk_bytes.max(1)) {
            if thought {
                self.acp_event(thought_chunk(&chunk));
            } else {
                self.acp_event(text_chunk(&chunk));
            }
            self.draw();
        }
    }

    /// Finishes the in-flight turn and advances synthetic time past every
    /// grace period, leaving a session that owes the event loop nothing.
    pub fn settle(&mut self) {
        self.acp_event(AcpEvent::PromptDone(acp::StopReason::EndTurn));
        let mut now = Instant::now();
        // The plan tracker's grace period (3s) is the longest deadline a
        // settled session can still be waiting on.
        for _ in 0..12 {
            self.tick(now);
            now += Duration::from_millis(500);
            if !self.app().wants_tick() {
                break;
            }
        }
        assert!(!self.app().wants_tick(), "a settled session must stop driving the tick loop");
        self.draw();
    }

    /// Routes a terminal event (key, paste, mouse, resize) the way the event
    /// loop does.
    pub fn terminal_event(&mut self, event: Event) {
        self.deliver(Message::Terminal(event));
    }

    pub fn key(&mut self, key: KeyEvent) {
        self.deliver(Message::Terminal(Event::Key(key)));
    }

    pub fn type_text(&mut self, text: &str) {
        for character in text.chars() {
            self.key(KeyEvent::new(KeyCode::Char(character), KeyModifiers::NONE));
        }
    }

    /// Types `text` and submits it with Enter.
    pub fn submit(&mut self, text: &str) {
        self.type_text(text);
        self.key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    }

    pub fn paste(&mut self, text: &str) {
        self.deliver(Message::Terminal(Event::Paste(text.to_string())));
    }

    pub fn acp_event(&mut self, event: AcpEvent) {
        self.deliver(Message::Agent(Box::new(event)));
    }

    pub fn tick(&mut self, now: Instant) {
        self.deliver(Message::Tick(now));
    }

    /// Completes the commands configured on the fake executor without touching
    /// the real filesystem, Git repository, terminal, or ACP connection.
    pub fn settle_tasks(&mut self) {
        self.executor.clear_available();
        let mut initial_batch = true;
        loop {
            let pending = self.executor.take_pending();
            if pending.is_empty() {
                return;
            }
            if initial_batch {
                for command in &pending {
                    if matches!(command, Command::Agent(_)) {
                        self.executor.available.push_back(command.clone());
                    }
                }
            }
            for command in pending {
                if let Some(result) = self.executor.complete(command) {
                    self.deliver(Message::CommandFinished(result));
                }
            }
            initial_batch = false;
        }
    }
}

impl TestUi<TestBackend> {
    /// The default scenario: 40x15 terminal, plain app, recording prompt handle.
    pub fn new() -> Self {
        Self::with_dimensions(40, 15)
    }

    pub fn with_dimensions(width: u16, height: u16) -> Self {
        TestUiBuilder::new().dimensions(width, height).build()
    }

    /// Resizes the backing terminal. The next [`Self::draw`] re-measures the
    /// inline viewport the way `Renderer::draw` does in the event loop.
    pub fn resize(&mut self, width: u16, height: u16) {
        self.terminal.backend_mut().resize(width, height);
    }
}

impl Default for TestUi<TestBackend> {
    fn default() -> Self {
        Self::new()
    }
}

/// A backend whose screen and scrollback buffers can be read back, so a
/// [`TestUi`] built on it can expose viewport/history/conversation text.
pub trait BuffersReader {
    fn screen(&self) -> &Buffer;
    fn scrollback(&self) -> &Buffer;
}

impl BuffersReader for TestBackend {
    fn screen(&self) -> &Buffer {
        self.buffer()
    }

    fn scrollback(&self) -> &Buffer {
        self.scrollback()
    }
}

impl BuffersReader for CountingBackend {
    fn screen(&self) -> &Buffer {
        self.buffer()
    }

    fn scrollback(&self) -> &Buffer {
        self.scrollback()
    }
}

impl BuffersReader for RecordingBackend {
    fn screen(&self) -> &Buffer {
        self.buffer()
    }

    fn scrollback(&self) -> &Buffer {
        self.scrollback()
    }
}

impl<B> TestUi<B>
where
    B: Backend + BuffersReader,
    B::Error: std::fmt::Debug,
{
    /// What the inline viewport currently shows: the composer, status line,
    /// and the live tail of the conversation. Draws a frame first, so asserting
    /// right after an input never reads a stale screen.
    pub fn viewport(&mut self) -> Buffer {
        self.draw();
        viewport_buffer(&mut self.terminal)
    }

    /// The terminal's own native scrollback containing committed conversation
    /// rows, after drawing a frame.
    pub fn history(&mut self) -> Buffer {
        self.draw();
        history_buffer(&mut self.terminal)
    }

    /// [`Self::history`] stacked on [`Self::viewport`]: everything the
    /// conversation has shown, oldest at the top, after drawing a frame.
    pub fn conversation(&mut self) -> Buffer {
        self.draw();
        conversation_buffer(&mut self.terminal)
    }

    pub fn viewport_text(&mut self) -> String {
        buffer_text(&self.viewport())
    }

    pub fn history_text(&mut self) -> String {
        buffer_text(&self.history())
    }

    pub fn conversation_text(&mut self) -> String {
        buffer_text(&self.conversation())
    }

    /// Row (within the viewport buffer) of the first line containing `needle`.
    pub fn viewport_row(&mut self, needle: &str) -> Option<u16> {
        row_containing(&self.viewport(), needle)
    }

    pub fn assert_viewport_contains(&mut self, needle: &str) {
        let viewport = self.viewport_text();
        assert!(
            viewport.contains(needle),
            "viewport should contain {needle:?}:
{viewport}"
        );
    }

    pub fn assert_viewport_not_contains(&mut self, needle: &str) {
        let viewport = self.viewport_text();
        assert!(
            !viewport.contains(needle),
            "viewport should not contain {needle:?}:
{viewport}"
        );
    }

    pub fn assert_history_contains(&mut self, needle: &str) {
        let history = self.history_text();
        assert!(
            history.contains(needle),
            "history should contain {needle:?}:
{history}"
        );
    }

    pub fn assert_history_not_contains(&mut self, needle: &str) {
        let history = self.history_text();
        assert!(
            !history.contains(needle),
            "history should not contain {needle:?}:
{history}"
        );
    }

    pub fn assert_conversation_contains(&mut self, needle: &str) {
        let conversation = self.conversation_text();
        assert!(
            conversation.contains(needle),
            "conversation should contain {needle:?}:
{conversation}"
        );
    }

    pub fn assert_conversation_not_contains(&mut self, needle: &str) {
        let conversation = self.conversation_text();
        assert!(
            !conversation.contains(needle),
            "conversation should not contain {needle:?}:
{conversation}"
        );
    }

    /// Asserts the viewport's visible text matches `expected` row-by-row.
    pub fn assert_viewport<S: AsRef<str>>(&mut self, expected: &[S]) {
        assert_buffer_eq(&self.viewport(), expected);
    }

    /// Asserts the committed history's visible text matches `expected` row-by-row.
    pub fn assert_history<S: AsRef<str>>(&mut self, expected: &[S]) {
        assert_buffer_eq(&self.history(), expected);
    }

    /// Asserts the stitched conversation's visible text matches `expected` row-by-row.
    pub fn assert_conversation<S: AsRef<str>>(&mut self, expected: &[S]) {
        assert_buffer_eq(&self.conversation(), expected);
    }
}

/// Builds a [`TestUi`]: the terminal dimensions plus every app-scenario option
/// a test cares about. Defaults match a plain `make_app()`-style scenario.
pub struct TestUiBuilder {
    width: u16,
    height: u16,
    working_dir: Option<PathBuf>,
    capabilities: AetherCapabilities,
    prompt_capabilities: acp::PromptCapabilities,
    config_options: Vec<acp::SessionConfigOption>,
    auth_methods: Vec<acp::AuthMethod>,
    session_capabilities: Option<acp::SessionCapabilities>,
    settings: UiSettings,
    workspace_status: Option<WorkspaceStatus>,
    git: FakeGit,
    opened_urls: Arc<Mutex<Vec<String>>>,
}

impl Default for TestUiBuilder {
    fn default() -> Self {
        Self {
            width: 40,
            height: 15,
            working_dir: None,
            capabilities: AetherCapabilities::default(),
            prompt_capabilities: acp::PromptCapabilities::new(),
            config_options: Vec::new(),
            auth_methods: Vec::new(),
            session_capabilities: None,
            settings: UiSettings::default(),
            workspace_status: None,
            git: FakeGit::default(),
            opened_urls: Arc::new(Mutex::new(Vec::new())),
        }
    }
}

impl TestUiBuilder {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn dimensions(mut self, width: u16, height: u16) -> Self {
        self.width = width;
        self.height = height;
        self
    }

    pub fn working_dir(mut self, working_dir: impl Into<PathBuf>) -> Self {
        self.working_dir = Some(working_dir.into());
        self
    }

    pub fn prompt_capabilities(mut self, capabilities: acp::PromptCapabilities) -> Self {
        self.prompt_capabilities = capabilities;
        self
    }

    pub fn config_options(mut self, options: Vec<acp::SessionConfigOption>) -> Self {
        self.config_options = options;
        self
    }

    pub fn auth_methods(mut self, methods: Vec<acp::AuthMethod>) -> Self {
        self.auth_methods = methods;
        self
    }

    pub fn settings(mut self, settings: UiSettings) -> Self {
        self.settings = settings;
        self
    }

    pub fn workspace_status(mut self, workspace_status: WorkspaceStatus) -> Self {
        self.workspace_status = Some(workspace_status);
        self
    }

    pub fn git(mut self, git: FakeGit) -> Self {
        self.git = git;
        self
    }

    /// Overrides the capabilities wholesale, for tests that care about metadata
    /// the individual toggles do not cover.
    pub fn session_capabilities(mut self, capabilities: acp::SessionCapabilities) -> Self {
        self.session_capabilities = Some(capabilities);
        self
    }

    pub fn prompt_search(mut self) -> Self {
        self.capabilities.prompt_search = true;
        self
    }

    pub fn session_preview(mut self) -> Self {
        self.capabilities.session_preview = true;
        self
    }

    pub fn workspace_move(mut self) -> Self {
        self.capabilities.workspace_move = true;
        self
    }

    /// Builds the whole UI scenario.
    pub fn build(self) -> TestUi {
        self.finish()
    }

    fn finish(self) -> TestUi {
        let app = App::new(self.app_config());
        TestUi {
            app,
            renderer: Renderer::new(),
            terminal: test_terminal(TestBackend::new(self.width, self.height)),
            executor: FakeExecutor::with_git(self.git),
            opened_urls: self.opened_urls,
        }
    }

    fn app_config(&self) -> AppConfig {
        let session_capabilities = self
            .session_capabilities
            .clone()
            .unwrap_or_else(|| acp::SessionCapabilities::new().meta(Some(self.capabilities.clone().to_meta())));
        AppConfig {
            session_id: SessionId::new("test-session"),
            agent_name: "aether".to_string(),
            prompt_capabilities: self.prompt_capabilities.clone(),
            session_capabilities,
            config_options: self.config_options.clone(),
            auth_methods: self.auth_methods.clone(),
            workspace_status: self
                .workspace_status
                .clone()
                .unwrap_or_else(|| WorkspaceStatus::new("~/code/demo", Some("main".to_string()))),
            working_dir: self.working_dir.clone().unwrap_or_else(|| PathBuf::from(".")),
            settings: self.settings.clone(),
            browser_opener: {
                let opened = self.opened_urls.clone();
                Arc::new(move |url: &str| {
                    opened.lock().unwrap().push(url.to_string());
                    Ok(())
                }) as BrowserOpener
            },
            clipboard_writer: Arc::new(|_| Ok(())),
        }
    }
}

/// The terminal a scenario draws into: the inline viewport sized from the
/// backend, exactly as the real event loop enters it.
fn test_terminal<B: Backend>(backend: B) -> Terminal<B>
where
    B::Error: std::fmt::Debug,
{
    let height = backend.size().unwrap().height;
    Terminal::with_options(backend, TerminalOptions { viewport: Viewport::Inline(inline_viewport_height(height)) })
        .unwrap()
}

/// What the inline viewport shows: `terminal.get_frame().area()` clipped out of
/// the backend's full screen buffer.
fn viewport_buffer<B>(terminal: &mut Terminal<B>) -> Buffer
where
    B: Backend + BuffersReader,
{
    let area = terminal.get_frame().area();
    let screen = terminal.backend().screen();
    let mut viewport = Buffer::empty(Rect::new(0, 0, area.width, area.height));
    for y in 0..area.height {
        for x in 0..area.width {
            viewport[(x, y)] = screen[(area.x + x, area.y + y)].clone();
        }
    }
    viewport
}

/// Content Ratatui's `insert_before` committed above the inline viewport.
fn history_buffer<B>(terminal: &mut Terminal<B>) -> Buffer
where
    B: Backend + BuffersReader,
{
    let viewport_area = terminal.get_frame().area();
    let screen = terminal.backend().screen();
    let scrollback = terminal.backend().scrollback();
    let history_height = scrollback.area.height.saturating_add(viewport_area.top());
    let mut history = Buffer::empty(Rect::new(0, 0, screen.area.width, history_height));
    for y in 0..scrollback.area.height {
        for x in 0..scrollback.area.width {
            history[(x, y)] = scrollback[(x, y)].clone();
        }
    }
    for y in 0..viewport_area.top() {
        for x in 0..screen.area.width {
            history[(x, scrollback.area.height + y)] = screen[(x, y)].clone();
        }
    }
    history
}

fn conversation_buffer<B>(terminal: &mut Terminal<B>) -> Buffer
where
    B: Backend + BuffersReader,
{
    let history = history_buffer(terminal);
    let viewport = viewport_buffer(terminal);
    let mut conversation =
        Buffer::empty(Rect::new(0, 0, viewport.area.width, history.area.height.saturating_add(viewport.area.height)));
    for y in 0..history.area.height {
        for x in 0..history.area.width {
            conversation[(x, y)] = history[(x, y)].clone();
        }
    }
    for y in 0..viewport.area.height {
        for x in 0..viewport.area.width {
            conversation[(x, history.area.height + y)] = viewport[(x, y)].clone();
        }
    }
    conversation
}

/// Whether any cell drawn with `symbol` satisfies `predicate`.
pub fn has_cell(buffer: &Buffer, symbol: &str, predicate: impl Fn(&Cell) -> bool) -> bool {
    for y in buffer.area.top()..buffer.area.bottom() {
        for x in buffer.area.left()..buffer.area.right() {
            if let Some(cell) = buffer.cell((x, y))
                && cell.symbol() == symbol
                && predicate(cell)
            {
                return true;
            }
        }
    }
    false
}

pub fn line_text(line: &ratatui::text::Line<'_>) -> String {
    line.spans.iter().map(|span| span.content.as_ref()).collect()
}

/// How many rows contain at least one cell with `background`.
pub fn rows_with_background(buffer: &Buffer, background: ratatui::style::Color) -> usize {
    (buffer.area.top()..buffer.area.bottom())
        .filter(|&y| {
            (buffer.area.left()..buffer.area.right())
                .any(|x| buffer.cell((x, y)).is_some_and(|cell| cell.bg == background))
        })
        .count()
}

pub fn row_containing(buffer: &Buffer, needle: &str) -> Option<u16> {
    (buffer.area.top()..buffer.area.bottom()).find(|&y| {
        let row = (buffer.area.left()..buffer.area.right())
            .map(|x| buffer.cell((x, y)).map_or(" ", Cell::symbol))
            .collect::<String>();
        row.contains(needle)
    })
}

pub fn buffer_text(buffer: &Buffer) -> String {
    let mut out = String::new();
    for y in buffer.area.top()..buffer.area.bottom() {
        for x in buffer.area.left()..buffer.area.right() {
            out.push_str(buffer.cell((x, y)).map_or(" ", Cell::symbol));
        }
        out.push('\n');
    }
    out
}

/// Asserts `buffer`'s visible text matches `expected` row-by-row after trimming
/// trailing spaces, panicking on the first mismatched line with the full buffer
/// dumped.
pub fn assert_buffer_eq<S: AsRef<str>>(buffer: &Buffer, expected: &[S]) {
    let actual_lines: Vec<String> =
        (buffer.area.top()..buffer.area.bottom()).map(|y| row_text(buffer, y).trim_end().to_string()).collect();
    for index in 0..actual_lines.len().max(expected.len()) {
        let actual_line = actual_lines.get(index).map_or("", String::as_str);
        let expected_line = expected.get(index).map_or("", AsRef::as_ref).trim_end();
        assert_eq!(
            actual_line,
            expected_line,
            "line {index} mismatch:\n  expected: {expected_line:?}\n  actual:   {actual_line:?}\n\nfull buffer:\n{}",
            actual_lines.join("\n")
        );
    }
}

pub fn row_text(buffer: &Buffer, y: u16) -> String {
    (buffer.area.left()..buffer.area.right()).map(|x| buffer.cell((x, y)).map_or(" ", Cell::symbol)).collect()
}

/// The shape of message [`TestUi::stream_message`] streams.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StreamContent {
    /// Flowing paragraphs, so finalization sealing runs as blank lines land and
    /// the open item stays small.
    Prose,
    /// One fenced Rust code block. No blank line outside the fence can finalize
    /// it while it streams, so the open item grows to the whole message — the
    /// shape that keeps re-rendering everything received so far.
    CodeBlock,
    /// A thinking stream: line-per-sentence thought chunks, which drive the
    /// progress band's activity rather than appending to the conversation.
    Thought,
}

const SEED_PROSE: &str = "\
Examining the request. The module guards its invariants behind a shared handle,
so the fix has to land on the writer side rather than at each call site. I will
rework the boundary so retries cannot observe a torn update, then cover the
regression with a test that fails on the current code.

";

const SEED_CODE_BLOCK: &str = "\
```rust
fn reconcile(state: &mut State, incoming: Vec<Delta>) -> Outcome {
    let mut applied = Vec::with_capacity(incoming.len());
    for delta in incoming {
        if !state.accepts(&delta) {
            continue;
        }
        state.apply(&delta);
        applied.push(delta);
    }
    state.commit(applied)
}
```

";

const SEED_CLOSING: &str = "\
Done — the writer now retries atomically and the regression test covers the
torn window.

";

const SEED_DIFF_BEFORE: &str = "\
fn reconcile(state: &mut State, incoming: Vec<Delta>) -> Outcome {
    let mut applied = Vec::new();
    for delta in incoming {
        state.apply(&delta);
    }
    state.commit(Vec::new())
}
";

const SEED_DIFF_AFTER: &str = "\
fn reconcile(state: &mut State, incoming: Vec<Delta>) -> Outcome {
    let mut applied = Vec::with_capacity(incoming.len());
    for delta in incoming {
        state.apply(&delta);
        applied.push(delta);
    }
    state.commit(applied)
}
";

pub fn session_update(update: acp::SessionUpdate) -> AcpEvent {
    AcpEvent::SessionUpdate { session_id: SessionId::new("test-session"), update: Box::new(update) }
}

fn user_chunk(text: &str) -> AcpEvent {
    session_update(acp::SessionUpdate::UserMessageChunk(acp::ContentChunk::new(acp::ContentBlock::Text(
        acp::TextContent::new(text),
    ))))
}

pub fn text_chunk(text: &str) -> AcpEvent {
    session_update(acp::SessionUpdate::AgentMessageChunk(acp::ContentChunk::new(acp::ContentBlock::Text(
        acp::TextContent::new(text),
    ))))
}

pub fn thought_chunk(text: &str) -> AcpEvent {
    session_update(acp::SessionUpdate::AgentThoughtChunk(acp::ContentChunk::new(acp::ContentBlock::Text(
        acp::TextContent::new(text),
    ))))
}

fn seed_bash_tool(id: &str) -> AcpEvent {
    let mut tool_call = acp::ToolCall::new(id.to_string(), format!("Run {id}"));
    tool_call.meta = Some(seed_tool_meta("bash"));
    tool_call.raw_input = Some(json!({ "command": "cargo test --module writer" }));
    session_update(acp::SessionUpdate::ToolCall(tool_call))
}

fn seed_edit_tool(id: &str, turn: usize) -> AcpEvent {
    session_update(acp::SessionUpdate::ToolCall(acp::ToolCall::new(
        id.to_string(),
        format!("Editing src/module_{turn}.rs"),
    )))
}

fn seed_spawn_tool(id: &str) -> AcpEvent {
    let mut tool_call = acp::ToolCall::new(id.to_string(), format!("Spawning sub-agents ({id})"));
    tool_call.meta = Some(seed_tool_meta("spawn_subagent"));
    session_update(acp::SessionUpdate::ToolCall(tool_call))
}

fn seed_tool_meta(tool_name: &str) -> acp::Meta {
    let mut meta = serde_json::Map::new();
    meta.insert(AETHER_TOOL_NAME_META_KEY.to_string(), json!(tool_name));
    meta
}

pub fn tool_completed(id: &str) -> AcpEvent {
    session_update(acp::SessionUpdate::ToolCallUpdate(acp::ToolCallUpdate::new(
        id.to_string(),
        acp::ToolCallUpdateFields::new().status(acp::ToolCallStatus::Completed),
    )))
}

fn seed_tool_diff(id: &str, turn: usize) -> AcpEvent {
    let diff = acp::Diff::new(format!("src/module_{turn}.rs"), SEED_DIFF_AFTER).old_text(SEED_DIFF_BEFORE);
    session_update(acp::SessionUpdate::ToolCallUpdate(acp::ToolCallUpdate::new(
        id.to_string(),
        acp::ToolCallUpdateFields::new()
            .content(vec![acp::ToolCallContent::Diff(diff)])
            .status(acp::ToolCallStatus::Completed),
    )))
}

fn seed_sub_agent(parent: &str, task: &str, agent: &str, event: SubAgentEvent) -> AcpEvent {
    AcpEvent::SubAgentProgress(SubAgentProgressParams {
        parent_tool_id: parent.to_string(),
        task_id: task.to_string(),
        agent_name: agent.to_string(),
        event,
    })
}

fn prose_message(total_bytes: usize) -> String {
    let mut message = String::new();
    let mut sentence = 0;
    while message.len() < total_bytes {
        for _ in 0..4 {
            let _ =
                write!(message, "Sentence {sentence} carries ordinary words so wrapping and parsing do real work. ");
            sentence += 1;
        }
        message.push_str("\n\n");
    }
    message
}

fn code_block_message(total_bytes: usize) -> String {
    let mut message = String::from("```rust\n");
    let mut line = 0;
    while message.len() < total_bytes {
        let _ = writeln!(message, "let value_{line} = state.reconcile(incoming[{line}]).expect(\"delta accepted\");");
        line += 1;
    }
    message.push_str("```\n");
    message
}

fn thought_message(total_bytes: usize) -> String {
    let mut message = String::new();
    let mut step = 0;
    while message.len() < total_bytes {
        let _ = writeln!(message, "Considering step {step} of the plan before acting on it.");
        step += 1;
    }
    message
}

pub fn chunk_message(message: &str, chunk_bytes: usize) -> Vec<String> {
    let mut chunks = Vec::new();
    let mut rest = message;
    while !rest.is_empty() {
        let mut end = rest.len().min(chunk_bytes);
        while !rest.is_char_boundary(end) {
            end -= 1;
        }
        chunks.push(rest[..end].to_string());
        rest = &rest[end..];
    }
    chunks
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::command::TerminalCommand;
    use crate::git_review::{FileStatus, StageState};

    #[test]
    fn fake_executor_preserves_command_order() {
        let mut executor = FakeExecutor::new();
        executor
            .record([Command::Filesystem(FilesystemCommand::ListThemes), Command::Terminal(TerminalCommand::RingBell)]);

        assert!(matches!(executor.take_commands()[..], [Command::Filesystem(_), Command::Terminal(_)]));
    }

    #[test]
    fn fake_filesystem_persists_files_and_settings_in_memory() {
        let mut filesystem = FakeFilesystem::new();
        let path = PathBuf::from("workspace/src/main.rs");
        filesystem.write_file(&path, "fn main() {}");
        filesystem.save_settings(UiSettings::default());

        assert_eq!(filesystem.read_to_string(&path).as_deref(), Some("fn main() {}"));
        assert!(filesystem.contains(Path::new("workspace/src")));
        assert!(filesystem.settings().is_some());
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
