use std::collections::HashSet;
use std::path::PathBuf;

use ratatui::text::Line;

use crate::annotation::{AnnotatedRows, Draft, DraftKey};
use crate::edit_buffer::EditBuffer;
use crate::git_diff::{DiffScope, FileDiff, FileStatus, GitDiffDocument, PatchAnchor, ReviewQueue, StageState};
use crate::selection::{Direction, SelectionState, scroll_into_view, step_clamped};

use crate::surface::Action;

use super::task;

use super::tasks::GitDiffTask;
use crate::generation::Generation;

pub struct GitDiffScreen {
    pub(super) working_dir: PathBuf,
    pub(super) repo_root: Option<PathBuf>,
    pub(super) scope: DiffScope,
    pub(super) state: GitDiffLoadState,
    pub(super) selected_file: usize,
    pub(super) selected_path: Option<String>,
    pub(super) drawer_selection: SelectionState,
    pub(super) focus: Focus,
    pub(super) collapsed: HashSet<String>,
    /// Columns the reviewer has widened the file drawer by, relative to the
    /// width the body would give it on its own.
    pub(super) drawer_offset: i16,
    /// The flattened file tree the drawer draws. Rebuilt only when the document
    /// or the collapsed set changes, rather than on every key, click, and frame.
    drawer_entries: Vec<DrawerEntry>,
    /// Bumped whenever a reload replaces the document, invalidating rendered patches.
    pub(super) document_revision: Generation,
    pub(super) bottom_bar: BottomBar,
    pub(super) show_full_file: bool,
    pub(super) full_file_content: Option<String>,
    pub(super) patch: PatchView,
    pub(super) review: Review,
    pub(super) request: Request,
}

/// The patch pane: where it is scrolled, where its cursor sits, and the
/// rendered views backing both.
#[derive(Default)]
pub(super) struct PatchView {
    /// First rendered row on screen.
    pub(super) scroll: usize,
    /// Rows the pane had on the last frame, for scrolling the cursor into view.
    pub(super) height: u16,
    pub(super) cursor: PatchCursor,
    /// The syntax-highlighted patch, independent of anything drawn over it.
    pub(super) rows: Option<PatchRows>,
    pub(super) view: Option<DiffView>,
}

/// The review being assembled: comments already filed, and the one being typed.
#[derive(Default)]
pub(super) struct Review {
    pub(super) queue: ReviewQueue,
    pub(super) draft: Option<Draft<PatchAnchor>>,
    /// Bumped on every change, invalidating any patch render that drew comments.
    pub(super) revision: Generation,
}

/// The git operation in flight, if any.
#[derive(Default)]
pub(super) struct Request {
    pub(super) id: Generation,
    pub(super) in_flight: bool,
    /// A destructive action armed and waiting for its key to be pressed again.
    pub(super) pending: Option<PendingAction>,
}

/// A rendered patch, and everything about the state it was rendered from. It is
/// reusable exactly while [`DiffViewKey`] still matches.
pub(super) struct DiffView {
    pub(super) rows: AnnotatedRows<PatchCursor>,
    pub(super) key: DiffViewKey,
}

/// The syntax-highlighted patch, one row per rendered line, before any review
/// comment is woven in.
pub(super) struct PatchRows {
    pub(super) rows: Vec<PatchRow>,
    pub(super) key: PatchKey,
}

/// One rendered row of a patch.
pub(super) struct PatchRow {
    pub(super) line: Line<'static>,
    /// The patch line a comment on this row anchors to, when it has one.
    pub(super) anchor: Option<PatchCursor>,
    /// Whether the patch cursor can rest on this row.
    pub(super) selectable: bool,
}

impl PatchRow {
    /// A row the cursor can rest on and comments can hang from.
    pub(super) fn at(line: Line<'static>, cursor: PatchCursor) -> Self {
        Self { line, anchor: Some(cursor), selectable: true }
    }

    /// A row comments can hang from, but the cursor skips over.
    pub(super) fn anchored(line: Line<'static>, cursor: PatchCursor) -> Self {
        Self { line, anchor: Some(cursor), selectable: false }
    }

    /// A row that is neither, such as a placeholder while a file loads.
    pub(super) fn inert(line: Line<'static>) -> Self {
        Self { line, anchor: None, selectable: false }
    }
}

/// Identity of the highlighted patch. Nothing a reviewer types is part of it,
/// so typing never re-highlights the file.
#[derive(Clone, PartialEq, Eq)]
pub(super) struct PatchKey {
    pub(super) file_path: String,
    pub(super) content_width: u16,
    pub(super) split: bool,
    pub(super) full_file: bool,
    pub(super) document_revision: Generation,
}

/// Identity of a rendered patch. The active view is reused only while every part
/// of this still holds — including the draft's text and cursor, so the box the
/// user is typing into redraws on each keystroke.
#[derive(Clone, PartialEq, Eq)]
pub(super) struct DiffViewKey {
    pub(super) patch: PatchKey,
    pub(super) comments_revision: Generation,
    pub(super) draft: Option<DraftKey<PatchAnchor>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum PendingAction {
    Reload,
    ScopeSwitch,
    Stage,
    Commit,
    Discard,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(super) struct PatchCursor {
    pub(super) hunk: usize,
    pub(super) line: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Focus {
    Drawer,
    Patch,
}

pub(super) enum GitDiffLoadState {
    Loading,
    Ready(GitDiffDocument),
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
    pub fn new(working_dir: PathBuf) -> (Self, GitDiffTask) {
        let mut screen = Self {
            working_dir,
            repo_root: None,
            scope: DiffScope::default(),
            state: GitDiffLoadState::Loading,
            selected_file: 0,
            selected_path: None,
            drawer_selection: SelectionState::default(),
            focus: Focus::Drawer,
            collapsed: HashSet::new(),
            drawer_offset: 0,
            drawer_entries: Vec::new(),
            document_revision: Generation::default(),
            bottom_bar: BottomBar::Help,
            show_full_file: false,
            full_file_content: None,
            patch: PatchView::default(),
            review: Review::default(),
            request: Request::default(),
        };
        let task = screen.begin_load();
        (screen, task)
    }

    pub(super) fn begin_load(&mut self) -> GitDiffTask {
        if let Some(path) = self.selected_file().map(|file| file.path.clone()) {
            self.selected_path = Some(path);
        }
        self.request.pending = None;
        self.review.queue.clear();
        self.bump_comments_revision();
        self.state = GitDiffLoadState::Loading;
        GitDiffTask::Load {
            request_id: self.begin_request(),
            working_dir: self.working_dir.clone(),
            repo_root: self.repo_root.clone(),
            scope: self.scope,
        }
    }

    pub(super) fn apply_document(&mut self, document: GitDiffDocument) {
        self.repo_root = Some(document.repo_root.clone());
        self.selected_file = self
            .selected_path
            .as_deref()
            .and_then(|path| document.files.iter().position(|file| file.path == path))
            .unwrap_or(0)
            .min(document.files.len().saturating_sub(1));
        self.patch.cursor = PatchCursor::default();
        self.document_revision.bump();
        self.bump_comments_revision();
        self.patch.rows = None;
        self.patch.view = None;
        self.state = GitDiffLoadState::Ready(document);
        self.rebuild_drawer();
    }

    pub(super) fn stage_all(&mut self) -> Vec<Action> {
        self.repo_operation(|request_id, repo_root| GitDiffTask::StageAll { request_id, repo_root })
    }

    pub(super) fn unstage_all(&mut self) -> Vec<Action> {
        self.repo_operation(|request_id, repo_root| GitDiffTask::UnstageAll { request_id, repo_root })
    }

    pub(super) fn toggle_stage(&mut self) -> Vec<Action> {
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
                GitDiffTask::UnstageFiles { request_id, repo_root, paths }
            } else {
                GitDiffTask::StageFiles { request_id, repo_root, paths }
            }
        })
    }

    pub(super) fn begin_commit(&mut self) -> Vec<Action> {
        if self.request.in_flight {
            return Vec::new();
        }
        if !self.any_staged() {
            self.bottom_bar = BottomBar::Error("Nothing staged to commit".to_string());
            return Vec::new();
        }
        self.bottom_bar = BottomBar::CommitEditor { buffer: EditBuffer::default() };
        Vec::new()
    }

    pub(super) fn begin_discard(&mut self) -> Vec<Action> {
        if self.request.in_flight {
            return Vec::new();
        }
        let Some(file) = self.selected_file().cloned() else {
            return Vec::new();
        };
        self.bottom_bar = BottomBar::DiscardConfirmation { path: file.path.clone(), status: file.status };
        Vec::new()
    }

    pub(super) fn toggle_full_file(&mut self) -> Vec<Action> {
        if self.request.in_flight {
            return Vec::new();
        }
        self.show_full_file = !self.show_full_file;
        if !self.show_full_file {
            self.full_file_content = None;
            return Vec::new();
        }
        if self.full_file_content.is_some() {
            return Vec::new();
        }
        let Some(path) = self.selected_file().map(|file| file.path.clone()) else {
            self.show_full_file = false;
            return Vec::new();
        };
        let messages =
            self.repo_operation(|request_id, repo_root| GitDiffTask::LoadFullFile { request_id, repo_root, path });
        if messages.is_empty() {
            self.show_full_file = false;
        }
        messages
    }

    pub(super) fn any_staged(&self) -> bool {
        matches!(&self.state, GitDiffLoadState::Ready(document)
            if document.files.iter().any(|file| matches!(file.staged, StageState::Staged | StageState::PartiallyStaged)))
    }

    /// Moves the focused pane's cursor by one entry.
    pub(super) fn move_vertical(&mut self, direction: Direction) {
        if self.focus == Focus::Patch {
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
    pub(super) fn resize_drawer(&mut self, columns: i16) -> Vec<Action> {
        self.drawer_offset = self.drawer_offset.saturating_add(columns);
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
        let current = file
            .hunks
            .iter()
            .take(self.patch.cursor.hunk)
            .map(|hunk| hunk.lines.len())
            .sum::<usize>()
            .saturating_add(self.patch.cursor.line)
            .min(last);
        let next = match direction {
            Direction::Backward => current.saturating_sub(amount),
            Direction::Forward => current.saturating_add(amount).min(last),
        };
        let mut remaining = next;
        for (hunk, entry) in file.hunks.iter().enumerate() {
            if remaining < entry.lines.len() {
                self.patch.cursor = PatchCursor { hunk, line: remaining };
                break;
            }
            remaining -= entry.lines.len();
        }
        self.sync_scroll_to_cursor();
    }

    /// Row the patch cursor currently occupies.
    pub(super) fn cursor_row(&self) -> Option<usize> {
        self.patch.view.as_ref()?.rows.row_of(self.patch.cursor)
    }

    pub(super) fn sync_scroll_to_cursor(&mut self) {
        let Some(cursor_row) = self.cursor_row() else {
            return;
        };
        self.patch.scroll = scroll_into_view(self.patch.scroll, cursor_row, usize::from(self.patch.height));
    }

    pub(super) fn move_patch_scroll(&mut self, direction: Direction, amount: usize) {
        let last_row = self.patch.view.as_ref().map_or(0, |view| view.rows.len().saturating_sub(1));
        self.patch.scroll = step_clamped(self.patch.scroll, direction, amount, last_row);
        self.sync_cursor_to_scroll();
    }

    /// Puts the cursor on whichever patch line the pane is now scrolled to.
    pub(super) fn sync_cursor_to_scroll(&mut self) {
        if let Some(cursor) = self.patch.view.as_ref().and_then(|view| view.rows.anchor_at_or_above(self.patch.scroll))
        {
            self.patch.cursor = cursor;
        }
    }

    pub(super) fn begin_draft(&mut self) -> Vec<Action> {
        let anchored = self
            .selected_file()
            .and_then(|file| file.hunks.get(self.patch.cursor.hunk))
            .is_some_and(|hunk| hunk.lines.len() > self.patch.cursor.line);
        if anchored {
            let anchor = PatchAnchor {
                file_index: self.selected_file,
                hunk: self.patch.cursor.hunk,
                line: self.patch.cursor.line,
            };
            self.review.draft = Some(Draft::new(anchor));
        }
        Vec::new()
    }

    pub(super) fn undo_last_comment(&mut self) -> Vec<Action> {
        self.review.queue.pop();
        self.bump_comments_revision();
        Vec::new()
    }

    pub(super) fn submit_review(&mut self) -> Vec<Action> {
        if self.review.queue.is_empty() {
            self.bottom_bar = BottomBar::Error("No comments to submit".to_string());
            return Vec::new();
        }
        if self.request.in_flight {
            self.bottom_bar = BottomBar::Error("Already submitting".to_string());
            return Vec::new();
        }
        vec![Action::SubmitReview(self.review.queue.format_prompt())]
    }

    pub(super) fn bump_comments_revision(&mut self) {
        self.review.revision.bump();
    }

    /// Claims the next request id and marks an operation in flight, so results
    /// for anything older are dropped.
    pub(super) fn begin_request(&mut self) -> Generation {
        self.request.id = Generation::next();
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
                self.selected_path = self.file_at(self.selected_file).map(|file| file.path.clone());
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
    pub(super) fn repo_operation(&mut self, build: impl FnOnce(Generation, PathBuf) -> GitDiffTask) -> Vec<Action> {
        let Some(repo_root) = self.repo_root.clone() else {
            return Vec::new();
        };
        task(build(self.begin_request(), repo_root))
    }
}
