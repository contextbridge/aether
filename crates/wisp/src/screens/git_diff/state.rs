use std::collections::HashSet;
use std::path::PathBuf;

use crate::screens::annotation::{Draft, Row};
use crate::git_review::{DiffDocument, DiffScope, FileDiff, FileStatus, PatchAnchor, ReviewQueue, StageState};
use crate::screens::review::{DocumentPane, Pane};
use crate::view::edit_buffer::EditBuffer;
use crate::view::selection::{Direction, SelectionState};

use crate::surfaces::input::{GitReviewOutput, ReviewOutcome};

use super::task;

use crate::command::GitCommand;
use crate::request::RequestId;

pub struct GitDiffScreen {
    pub(super) working_dir: PathBuf,
    pub(super) repo_root: Option<PathBuf>,
    pub(super) scope: DiffScope,
    pub(super) state: GitDiffLoadState,
    pub(super) selected_file: usize,
    pub(super) drawer_selection: SelectionState,
    pub(super) focus: Pane,
    pub(super) collapsed: HashSet<String>,
    /// Columns the reviewer has widened the file drawer by, relative to the
    /// width the body would give it on its own.
    pub(super) drawer_offset: i16,
    /// The flattened file tree the drawer draws. Rebuilt only when the document
    /// or the collapsed set changes, rather than on every key, click, and frame.
    drawer_entries: Vec<DrawerEntry>,
    pub(super) bottom_bar: BottomBar,
    pub(super) full_file: FullFileView,
    pub(super) patch: PatchView,
    pub(super) review: Review,
    pub(super) shortcuts_open: bool,
    pub(super) request: Request,
}

/// Whether the patch pane shows the diff or the whole file. Keeping the load
/// in the same enum makes "on but not loaded yet" and "on and loaded" the only
/// on states, so neither has to be inferred from a flag-and-option pair.
#[derive(Default)]
pub(super) enum FullFileView {
    #[default]
    Off,
    Loading,
    Loaded(String),
}

impl FullFileView {
    pub(super) fn is_on(&self) -> bool {
        !matches!(self, Self::Off)
    }
}

/// The patch pane: the document under review.
#[derive(Default)]
pub(super) struct PatchView {
    pub(super) document: DocumentPane<PatchCursor>,
}

/// The review being assembled: comments already filed, and the one being typed.
#[derive(Default)]
pub(super) struct Review {
    pub(super) queue: ReviewQueue,
    pub(super) draft: Option<Draft<PatchAnchor>>,
}

/// The git operation in flight, if any.
#[derive(Default)]
pub(super) struct Request {
    pub(super) id: RequestId,
    pub(super) in_flight: bool,
    /// A destructive action armed and waiting for its key to be pressed again.
    pub(super) pending: Option<PendingAction>,
}

/// One rendered row of a patch.
pub(super) type PatchRow = Row<PatchCursor>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum PendingAction {
    Reload,
    ScopeSwitch,
    Stage,
    Commit,
    Discard,
    Close,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(super) struct PatchCursor {
    pub(super) hunk: usize,
    pub(super) line: usize,
}

pub(super) enum GitDiffLoadState {
    Loading {
        /// The path selected before the reload, so the reloaded document can
        /// put the selection back on the same file.
        restore_path: Option<String>,
    },
    Ready(DiffDocument),
    Error(String),
}

#[derive(Clone)]
pub(super) enum DrawerEntry {
    Directory { path: String, depth: usize },
    File { index: usize, depth: usize },
}

pub(super) enum BottomBar {
    Help,
    CommitEditor { buffer: EditBuffer },
    DiscardConfirmation { path: String, status: FileStatus },
    Error(String),
}

impl GitDiffScreen {
    pub fn new(working_dir: PathBuf) -> (Self, GitCommand) {
        let mut screen = Self {
            working_dir,
            repo_root: None,
            scope: DiffScope::default(),
            state: GitDiffLoadState::Loading { restore_path: None },
            selected_file: 0,
            drawer_selection: SelectionState::default(),
            focus: Pane::Nav,
            collapsed: HashSet::new(),
            drawer_offset: 0,
            drawer_entries: Vec::new(),
            bottom_bar: BottomBar::Help,
            full_file: FullFileView::default(),
            patch: PatchView::default(),
            review: Review::default(),
            shortcuts_open: false,
            request: Request::default(),
        };
        let task = screen.begin_load();
        (screen, task)
    }

    pub(super) fn begin_load(&mut self) -> GitCommand {
        let restore_path = self.selected_file().map(|file| file.path.clone());
        self.request.pending = None;
        self.review.queue.clear();
        self.state = GitDiffLoadState::Loading { restore_path };
        GitCommand::Load {
            request_id: self.begin_request(),
            working_dir: self.working_dir.clone(),
            repo_root: self.repo_root.clone(),
            scope: self.scope,
        }
    }

    pub(super) fn apply_document(&mut self, document: DiffDocument) {
        self.repo_root = Some(document.repo_root.clone());
        let restore_path = match &self.state {
            GitDiffLoadState::Loading { restore_path } => restore_path.clone(),
            GitDiffLoadState::Ready(_) | GitDiffLoadState::Error(_) => None,
        };
        self.selected_file = restore_path
            .as_deref()
            .and_then(|path| document.files.iter().position(|file| file.path == path))
            .unwrap_or(0)
            .min(document.files.len().saturating_sub(1));
        self.patch.document.cursor = PatchCursor::default();
        self.state = GitDiffLoadState::Ready(document);
        self.rebuild_drawer();
    }

    pub(super) fn stage_all(&mut self) -> Vec<GitReviewOutput> {
        self.repo_operation(|request_id, repo_root| GitCommand::StageAll { request_id, repo_root })
    }

    pub(super) fn unstage_all(&mut self) -> Vec<GitReviewOutput> {
        self.repo_operation(|request_id, repo_root| GitCommand::UnstageAll { request_id, repo_root })
    }

    pub(super) fn toggle_stage(&mut self) -> Vec<GitReviewOutput> {
        let Some(entry) = self.selected_drawer_entry().cloned() else {
            return Vec::new();
        };
        let files = self.files_for_entry(&entry);
        if files.is_empty() {
            return Vec::new();
        }
        let all_staged = files.iter().all(|file| file.staged == StageState::Staged);
        let paths: Vec<String> = files.iter().map(|file| file.path.clone()).collect();
        self.repo_operation(move |request_id, repo_root| {
            if all_staged {
                GitCommand::UnstageFiles { request_id, repo_root, paths }
            } else {
                GitCommand::StageFiles { request_id, repo_root, paths }
            }
        })
    }

    pub(super) fn begin_commit(&mut self) -> Vec<GitReviewOutput> {
        if self.request.in_flight {
            return Vec::new();
        }
        if !matches!(&self.state, GitDiffLoadState::Ready(document)
            if document.files.iter().any(|file| matches!(file.staged, StageState::Staged | StageState::PartiallyStaged)))
        {
            self.bottom_bar = BottomBar::Error("Nothing staged to commit".to_string());
            return Vec::new();
        }
        self.bottom_bar = BottomBar::CommitEditor { buffer: EditBuffer::default() };
        Vec::new()
    }

    pub(super) fn begin_discard(&mut self) -> Vec<GitReviewOutput> {
        if self.request.in_flight {
            return Vec::new();
        }
        let Some(file) = self.selected_file().cloned() else {
            return Vec::new();
        };
        self.bottom_bar = BottomBar::DiscardConfirmation { path: file.path.clone(), status: file.status };
        Vec::new()
    }

    pub(super) fn toggle_full_file(&mut self) -> Vec<GitReviewOutput> {
        if self.request.in_flight {
            return Vec::new();
        }
        if self.full_file.is_on() {
            self.full_file = FullFileView::Off;
            return Vec::new();
        }
        let Some(path) = self.selected_file().map(|file| file.path.clone()) else {
            return Vec::new();
        };
        let messages =
            self.repo_operation(|request_id, repo_root| GitCommand::LoadFullFile { request_id, repo_root, path });
        if !messages.is_empty() {
            self.full_file = FullFileView::Loading;
        }
        messages
    }

    /// Moves the focused pane's cursor by one entry.
    pub(super) fn move_vertical(&mut self, direction: Direction) {
        if self.focus == Pane::Document {
            self.move_patch_cursor(direction, 1);
            return;
        }
        if self.drawer_entries.is_empty() {
            return;
        }
        self.drawer_selection.step_clamped(self.drawer_entries.len(), direction, |_| true);
        self.follow_drawer_selection();
    }

    /// Widens the file drawer by `columns`, or narrows it when negative. The
    /// next frame clamps the result to what the body can honour.
    pub(super) fn resize_drawer(&mut self, columns: i16) -> Vec<GitReviewOutput> {
        if columns < 0 && self.drawer_offset <= -20 {
            return Vec::new();
        }
        if columns > 0 && self.drawer_offset < -20 {
            self.drawer_offset = -20;
        } else {
            self.drawer_offset = self.drawer_offset.saturating_add(columns);
        }
        Vec::new()
    }

    /// Points the patch pane at the file the drawer selection landed on, if it
    /// landed on a file rather than a directory.
    pub(super) fn follow_drawer_selection(&mut self) {
        if let Some(DrawerEntry::File { index, .. }) = self.selected_drawer_entry() {
            self.selected_file = *index;
        }
    }

    pub(super) fn selected_drawer_entry(&self) -> Option<&DrawerEntry> {
        self.drawer_selection.selected().and_then(|selected| self.drawer_entries.get(selected))
    }

    /// Moves the patch cursor `amount` patch lines, flattening hunk boundaries
    /// so it walks the file continuously.
    pub(super) fn move_patch_cursor(&mut self, direction: Direction, amount: usize) {
        let Some(file) = self.selected_file() else {
            return;
        };
        let total = file.hunks.iter().map(|hunk| hunk.lines.len()).sum::<usize>();
        let Some(last) = total.checked_sub(1) else {
            return;
        };
        let cursor = self.patch.document.cursor;
        let current = file
            .hunks
            .iter()
            .take(cursor.hunk)
            .map(|hunk| hunk.lines.len())
            .sum::<usize>()
            .saturating_add(cursor.line)
            .min(last);
        let next = match direction {
            Direction::Backward => current.saturating_sub(amount),
            Direction::Forward => current.saturating_add(amount).min(last),
        };
        let mut remaining = next;
        for (hunk, entry) in file.hunks.iter().enumerate() {
            if remaining < entry.lines.len() {
                self.patch.document.cursor = PatchCursor { hunk, line: remaining };
                break;
            }
            remaining -= entry.lines.len();
        }
        self.patch.document.follow_cursor();
    }

    pub(super) fn move_patch_scroll(&mut self, direction: Direction, amount: usize) {
        self.patch.document.scroll_by(direction, amount);
    }

    pub(super) fn begin_draft(&mut self) -> Vec<GitReviewOutput> {
        let cursor = self.patch.document.cursor;
        let anchored = self
            .selected_file()
            .and_then(|file| file.hunks.get(cursor.hunk))
            .is_some_and(|hunk| hunk.lines.len() > cursor.line);
        if anchored {
            let anchor = PatchAnchor { file_index: self.selected_file, hunk: cursor.hunk, line: cursor.line };
            self.review.draft = Some(Draft::new(anchor));
        }
        Vec::new()
    }

    pub(super) fn undo_last_comment(&mut self) -> Vec<GitReviewOutput> {
        self.review.queue.pop();
        Vec::new()
    }

    pub(super) fn submit_review(&mut self) -> Vec<GitReviewOutput> {
        if self.review.queue.is_empty() {
            self.bottom_bar = BottomBar::Error("No comments to submit".to_string());
            return Vec::new();
        }
        if self.request.in_flight {
            self.bottom_bar = BottomBar::Error("Already submitting".to_string());
            return Vec::new();
        }
        vec![GitReviewOutput::Outcome(ReviewOutcome::Submitted(self.review.queue.format_prompt()))]
    }

    /// Claims the next request id and marks an operation in flight, so results
    /// for anything older are dropped.
    pub(super) fn begin_request(&mut self) -> RequestId {
        self.request.id = RequestId::next();
        self.request.in_flight = true;
        self.request.id
    }

    pub(super) fn collapse_selected(&mut self) {
        if let Some(DrawerEntry::Directory { path, .. }) = self.selected_drawer_entry() {
            let path = path.clone();
            self.collapsed.insert(path);
            self.rebuild_drawer();
        }
    }

    /// Expands the selected directory, or points the patch pane at the selected
    /// file. Reports whether it was a directory.
    pub(super) fn expand_or_open_selected(&mut self) -> bool {
        match self.selected_drawer_entry() {
            Some(DrawerEntry::Directory { path, .. }) => {
                let path = path.clone();
                self.collapsed.remove(&path);
                self.rebuild_drawer();
                true
            }
            Some(DrawerEntry::File { index, .. }) => {
                self.selected_file = *index;
                false
            }
            None => false,
        }
    }

    pub(super) fn drawer_entries(&self) -> &[DrawerEntry] {
        &self.drawer_entries
    }

    /// Recomputes the file tree and puts the selection back on the current file.
    fn rebuild_drawer(&mut self) {
        self.drawer_entries = self.build_drawer_entries();
        let selected = self
            .drawer_entries
            .iter()
            .position(|entry| matches!(entry, DrawerEntry::File { index, .. } if *index == self.selected_file))
            .unwrap_or(0);
        self.drawer_selection.select(Some(selected), self.drawer_entries.len());
    }

    pub(super) fn selected_file(&self) -> Option<&FileDiff> {
        self.file_at(self.selected_file)
    }

    fn build_drawer_entries(&self) -> Vec<DrawerEntry> {
        let GitDiffLoadState::Ready(document) = &self.state else {
            return Vec::new();
        };
        let mut entries = Vec::new();
        let mut emitted = HashSet::new();
        for (index, file) in document.files.iter().enumerate() {
            let parts: Vec<&str> = file.path.split('/').collect();
            let mut parent = String::new();
            let mut hidden = false;
            for (depth, part) in parts.iter().take(parts.len().saturating_sub(1)).enumerate() {
                if !parent.is_empty() {
                    parent.push('/');
                }
                parent.push_str(part);
                if hidden {
                    continue;
                }
                if emitted.insert(parent.clone()) {
                    entries.push(DrawerEntry::Directory { path: parent.clone(), depth });
                }
                if self.collapsed.contains(&parent) {
                    hidden = true;
                }
            }
            if !hidden {
                entries.push(DrawerEntry::File { index, depth: parts.len().saturating_sub(1) });
            }
        }
        entries
    }

    pub(super) fn files_for_entry(&self, entry: &DrawerEntry) -> Vec<&FileDiff> {
        let GitDiffLoadState::Ready(document) = &self.state else {
            return Vec::new();
        };
        match entry {
            DrawerEntry::Directory { path, .. } => {
                let prefix = format!("{path}/");
                document.files.iter().filter(|file| file.path.starts_with(&prefix)).collect()
            }
            DrawerEntry::File { index, .. } => document.files.get(*index).into_iter().collect(),
        }
    }

    pub(super) fn file_at(&self, index: usize) -> Option<&FileDiff> {
        let GitDiffLoadState::Ready(document) = &self.state else {
            return None;
        };
        document.files.get(index)
    }

    /// Runs `build` against the repository root, doing nothing when the diff has
    /// not yet reported one.
    pub(super) fn repo_operation(
        &mut self,
        build: impl FnOnce(RequestId, PathBuf) -> GitCommand,
    ) -> Vec<GitReviewOutput> {
        let Some(repo_root) = self.repo_root.clone() else {
            return Vec::new();
        };
        task(build(self.begin_request(), repo_root))
    }
}
