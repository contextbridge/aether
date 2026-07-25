use std::collections::HashSet;
use std::path::PathBuf;

use ratatui::layout::{Position, Rect};
use tui_scrollview::{ScrollView, ScrollViewState};

use crate::edit_buffer::EditBuffer;
use crate::git_diff::{DiffScope, FileDiff, FileStatus, GitDiffDocument, PatchAnchor, ReviewQueue, StageState};
use crate::selection::SelectionState;

use super::effects::{GitDiffEffect, GitDiffOutcome, next_request_id};

pub(super) type CursorRow = (usize, usize, usize);

/// Maximum number of recently-rendered file patches kept for instant re-display when the
/// user browses between files.
pub(super) const DIFF_VIEW_CACHE_LIMIT: usize = 8;

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
    pub(super) patch_scroll_state: ScrollViewState,
    pub(super) last_patch_height: u16,
    pub(super) diff_view: Option<DiffView>,
    /// Bounded MRU cache of fully-rendered, stable (draft-free) diff views for files other
    /// than the one currently selected, so switching files does not re-run syntax
    /// highlighting for recently-visited patches.
    pub(super) diff_view_cache: Vec<DiffView>,
    pub(super) document_revision: usize,
    pub(super) comments_revision: usize,
    pub(super) request_id: u64,
    pub(super) operation_in_flight: bool,
    pub(super) bottom_bar: BottomBar,
    pub(super) show_full_file: bool,
    pub(super) full_file_content: Option<String>,
    pub(super) review_queue: ReviewQueue,
    pub(super) draft: Option<DraftState>,
    pub(super) patch_cursor: PatchCursor,
    pub(super) pending_action: Option<PendingAction>,
    pub(super) last_area: Rect,
}

pub(super) struct DiffView {
    pub(super) scroll_view: ScrollView,
    pub(super) cursor_rows: Vec<CursorRow>,
    pub(super) draft_cursor: Option<(usize, u16)>,
    pub(super) file_path: String,
    pub(super) content_width: u16,
    pub(super) split: bool,
    pub(super) full_file: bool,
    pub(super) document_revision: usize,
    pub(super) comments_revision: usize,
    pub(super) draft_signature: Option<(usize, usize, usize, usize)>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum PendingAction {
    Reload,
    ScopeSwitch,
    Stage,
    Commit,
    Discard,
}

pub(super) struct DraftState {
    pub(super) anchor: PatchAnchor,
    pub(super) buffer: EditBuffer,
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
    pub fn new(working_dir: PathBuf) -> (Self, GitDiffEffect) {
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
            patch_scroll_state: ScrollViewState::new(),
            last_patch_height: 0,
            diff_view: None,
            diff_view_cache: Vec::new(),
            document_revision: 0,
            comments_revision: 0,
            request_id: 0,
            operation_in_flight: false,
            bottom_bar: BottomBar::Help,
            show_full_file: false,
            full_file_content: None,
            review_queue: ReviewQueue::default(),
            draft: None,
            patch_cursor: PatchCursor::default(),
            pending_action: None,
            last_area: Rect::new(0, 0, 120, 40),
        };
        let effect = screen.begin_load();
        (screen, effect)
    }
    pub fn cancel(&mut self) {}

    pub fn current_request_id(&self) -> u64 {
        self.request_id
    }

    pub(super) fn begin_load(&mut self) -> GitDiffEffect {
        if let Some(path) = self.selected_file().map(|file| file.path.clone()) {
            self.selected_path = Some(path);
        }
        self.pending_action = None;
        self.review_queue.clear();
        self.comments_revision = self.comments_revision.wrapping_add(1);
        self.request_id = next_request_id();
        self.operation_in_flight = true;
        self.state = GitDiffLoadState::Loading;
        GitDiffEffect::Load {
            request_id: self.request_id,
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
        self.patch_cursor = PatchCursor::default();
        self.document_revision = self.document_revision.wrapping_add(1);
        self.comments_revision = self.comments_revision.wrapping_add(1);
        self.diff_view = None;
        self.diff_view_cache.clear();
        self.state = GitDiffLoadState::Ready(document);
        self.sync_drawer_selection();
    }
    pub(super) fn stage_all(&mut self) -> GitDiffOutcome {
        let Some(repo_root) = self.repo_root.clone() else {
            return GitDiffOutcome::None;
        };
        self.request_id = next_request_id();
        self.operation_in_flight = true;
        GitDiffOutcome::Effect(GitDiffEffect::StageAll { request_id: self.request_id, repo_root })
    }

    pub(super) fn unstage_all(&mut self) -> GitDiffOutcome {
        let Some(repo_root) = self.repo_root.clone() else {
            return GitDiffOutcome::None;
        };
        self.request_id = next_request_id();
        self.operation_in_flight = true;
        GitDiffOutcome::Effect(GitDiffEffect::UnstageAll { request_id: self.request_id, repo_root })
    }

    pub(super) fn toggle_stage(&mut self) -> GitDiffOutcome {
        let Some(repo_root) = self.repo_root.clone() else {
            return GitDiffOutcome::None;
        };
        let entries = self.drawer_entries();
        let Some(entry) = self.drawer_selection.selected().and_then(|selected| entries.get(selected)) else {
            return GitDiffOutcome::None;
        };
        let files = self.files_for_entry(entry);
        if files.is_empty() {
            return GitDiffOutcome::None;
        }
        let all_staged = files.iter().all(|file| file.staged == StageState::Staged);
        let paths = files.iter().map(|file| file.path.clone()).collect();
        self.request_id = next_request_id();
        self.operation_in_flight = true;
        let effect = if all_staged {
            GitDiffEffect::UnstageFiles { request_id: self.request_id, repo_root, paths }
        } else {
            GitDiffEffect::StageFiles { request_id: self.request_id, repo_root, paths }
        };
        GitDiffOutcome::Effect(effect)
    }

    pub(super) fn begin_commit(&mut self) -> GitDiffOutcome {
        if self.operation_in_flight {
            return GitDiffOutcome::None;
        }
        if !self.any_staged() {
            self.bottom_bar = BottomBar::Error("Nothing staged to commit".to_string());
            return GitDiffOutcome::None;
        }
        self.bottom_bar = BottomBar::CommitEditor { buffer: EditBuffer::default() };
        GitDiffOutcome::None
    }
    pub(super) fn begin_discard(&mut self) -> GitDiffOutcome {
        if self.operation_in_flight {
            return GitDiffOutcome::None;
        }
        let Some(file) = self.selected_file().cloned() else {
            return GitDiffOutcome::None;
        };
        self.bottom_bar = BottomBar::DiscardConfirmation { path: file.path.clone(), status: file.status };
        GitDiffOutcome::None
    }

    pub(super) fn toggle_full_file(&mut self) -> GitDiffOutcome {
        if self.operation_in_flight {
            return GitDiffOutcome::None;
        }
        self.show_full_file = !self.show_full_file;
        if !self.show_full_file {
            self.full_file_content = None;
        }
        if self.show_full_file && self.full_file_content.is_none() {
            let path = self.selected_file().map(|f| f.path.clone());
            let repo_root = self.repo_root.clone();
            let (Some(path), Some(repo_root)) = (path, repo_root) else {
                self.show_full_file = false;
                return GitDiffOutcome::None;
            };
            let request_id = next_request_id();
            self.request_id = request_id;
            self.operation_in_flight = true;
            GitDiffOutcome::Effect(GitDiffEffect::LoadFullFile { request_id, repo_root, path })
        } else {
            GitDiffOutcome::None
        }
    }

    pub(super) fn any_staged(&self) -> bool {
        matches!(&self.state, GitDiffLoadState::Ready(document)
            if document.files.iter().any(|file| matches!(file.staged, StageState::Staged | StageState::PartiallyStaged)))
    }

    pub(super) fn move_vertical(&mut self, amount: isize) {
        if self.focus == Focus::Patch {
            self.move_patch_cursor(amount);
            return;
        }
        let entries = self.drawer_entries();
        if entries.is_empty() {
            return;
        }
        let current = self.drawer_selection.selected().unwrap_or_default();
        let selected = current.saturating_add_signed(amount).min(entries.len() - 1);
        self.drawer_selection.select(Some(selected), entries.len());
        if let Some(DrawerEntry::File { index, .. }) = entries.get(selected) {
            self.selected_file = *index;
        }
    }

    #[allow(clippy::cast_possible_wrap, clippy::cast_sign_loss)]
    pub(super) fn move_patch_cursor(&mut self, amount: isize) {
        let Some(file) = self.selected_file().cloned() else {
            return;
        };
        if file.hunks.is_empty() {
            return;
        }
        let total_lines: isize = file.hunks.iter().map(|h| h.lines.len() as isize).sum();
        let current_line = self.cursor_line_index(&file);
        let new_index = (current_line + amount).clamp(0, total_lines - 1);
        let mut remaining = new_index;
        for (hunk_idx, hunk) in file.hunks.iter().enumerate() {
            let hunk_len = hunk.lines.len() as isize;
            if remaining < hunk_len {
                self.patch_cursor = PatchCursor { hunk: hunk_idx, line: remaining as usize };
                break;
            }
            remaining -= hunk_len;
        }
        self.sync_scroll_to_cursor();
    }

    #[allow(clippy::cast_possible_wrap, clippy::cast_sign_loss)]
    pub(super) fn cursor_line_index(&self, file: &FileDiff) -> isize {
        let mut count: isize = 0;
        for (hunk_idx, hunk) in file.hunks.iter().enumerate() {
            if hunk_idx < self.patch_cursor.hunk {
                count += hunk.lines.len() as isize;
            } else if hunk_idx == self.patch_cursor.hunk {
                count += self.patch_cursor.line as isize;
                break;
            }
        }
        count
    }

    /// Moves the currently-active diff view into the MRU cache (when it is a stable,
    /// draft-free snapshot) so it can be reused if the user switches back to its file.
    pub(super) fn park_active_diff_view(&mut self) {
        if let Some(view) = self.diff_view.take()
            && view.draft_signature.is_none()
        {
            self.diff_view_cache.insert(0, view);
            if self.diff_view_cache.len() > DIFF_VIEW_CACHE_LIMIT {
                self.diff_view_cache.pop();
            }
        }
    }

    pub(super) fn sync_scroll_to_cursor(&mut self) {
        let Some(view) = &self.diff_view else {
            return;
        };
        let cursor_row = view
            .cursor_rows
            .iter()
            .find_map(|(h, l, row)| (*h == self.patch_cursor.hunk && *l == self.patch_cursor.line).then_some(*row));
        let Some(cursor_row) = cursor_row else {
            return;
        };
        let offset = usize::from(self.patch_scroll_state.offset().y);
        let viewport = usize::from(self.last_patch_height);
        if cursor_row < offset {
            self.patch_scroll_state.set_offset(Position::new(0, u16::try_from(cursor_row).unwrap_or(u16::MAX)));
        } else if viewport > 0 && cursor_row >= offset + viewport {
            let new_offset = cursor_row.saturating_sub(viewport.saturating_sub(1));
            self.patch_scroll_state.set_offset(Position::new(0, u16::try_from(new_offset).unwrap_or(u16::MAX)));
        }
    }

    #[allow(clippy::cast_possible_wrap)]
    pub(super) fn move_patch_scroll(&mut self, amount: isize) {
        let current = self.patch_scroll_state.offset().y as isize;
        let new_offset = current.saturating_add(amount).max(0);
        self.patch_scroll_state.set_offset(Position::new(0, u16::try_from(new_offset).unwrap_or(u16::MAX)));
        self.sync_cursor_to_scroll();
    }

    pub(super) fn sync_cursor_to_scroll(&mut self) {
        let Some(view) = &self.diff_view else {
            return;
        };
        let offset = usize::from(self.patch_scroll_state.offset().y);
        let idx = match view.cursor_rows.binary_search_by_key(&offset, |&(_, _, row)| row) {
            Ok(idx) => idx,
            Err(idx) => idx.saturating_sub(1),
        };
        if let Some(&(hunk, line, _)) = view.cursor_rows.get(idx) {
            self.patch_cursor = PatchCursor { hunk, line };
        }
    }

    pub(super) fn begin_draft(&mut self) -> GitDiffOutcome {
        let Some(file) = self.selected_file() else {
            return GitDiffOutcome::None;
        };
        let Some(hunk) = file.hunks.get(self.patch_cursor.hunk) else {
            return GitDiffOutcome::None;
        };
        if hunk.lines.get(self.patch_cursor.line).is_none() {
            return GitDiffOutcome::None;
        }
        let anchor =
            PatchAnchor { file_index: self.selected_file, hunk: self.patch_cursor.hunk, line: self.patch_cursor.line };
        self.draft = Some(DraftState { anchor, buffer: EditBuffer::default() });
        GitDiffOutcome::None
    }
    pub(super) fn undo_last_comment(&mut self) -> GitDiffOutcome {
        self.review_queue.pop();
        self.comments_revision = self.comments_revision.wrapping_add(1);
        GitDiffOutcome::None
    }

    pub(super) fn submit_review(&mut self) -> GitDiffOutcome {
        if self.review_queue.is_empty() {
            self.bottom_bar = BottomBar::Error("No comments to submit".to_string());
            return GitDiffOutcome::None;
        }
        if self.operation_in_flight {
            self.bottom_bar = BottomBar::Error("Already submitting".to_string());
            return GitDiffOutcome::None;
        }
        let prompt = self.review_queue.format_prompt();
        self.request_id = next_request_id();
        self.operation_in_flight = true;
        GitDiffOutcome::SubmitReview(GitDiffEffect::SubmitReview { request_id: self.request_id, prompt })
    }
    pub(super) fn collapse_selected(&mut self) {
        let entries = self.drawer_entries();
        if let Some(DrawerEntry::Directory { path, .. }) =
            self.drawer_selection.selected().and_then(|selected| entries.get(selected))
        {
            self.collapsed.insert(path.clone());
            self.sync_drawer_selection();
        }
    }

    pub(super) fn expand_or_open_selected(&mut self) -> bool {
        let entries = self.drawer_entries();
        match self.drawer_selection.selected().and_then(|selected| entries.get(selected)) {
            Some(DrawerEntry::Directory { path, .. }) => {
                self.collapsed.remove(path);
                true
            }
            Some(DrawerEntry::File { index, .. }) => {
                self.selected_file = *index;
                self.selected_path = self.file_at(*index).map(|file| file.path.clone());
                false
            }
            None => false,
        }
    }

    pub(super) fn sync_drawer_selection(&mut self) {
        let entries = self.drawer_entries();
        let selected = entries
            .iter()
            .position(|entry| matches!(entry, DrawerEntry::File { index, .. } if *index == self.selected_file))
            .unwrap_or(0);
        self.drawer_selection.select(Some(selected), entries.len());
    }

    pub(super) fn selected_file(&self) -> Option<&FileDiff> {
        let GitDiffLoadState::Ready(document) = &self.state else {
            return None;
        };
        document.files.get(self.selected_file)
    }

    pub(super) fn drawer_entries(&self) -> Vec<DrawerEntry> {
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
}
