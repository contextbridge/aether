use std::collections::HashSet;
use std::path::PathBuf;

use ratatui::layout::Position;
use tui_scrollview::{ScrollView, ScrollViewState};

use crate::edit_buffer::EditBuffer;
use crate::git_diff::{DiffScope, FileDiff, FileStatus, GitDiffDocument, PatchAnchor, ReviewQueue, StageState};
use crate::selection::{Direction, SelectionState};

use crate::surface::SurfaceMessage;

use super::effect;

use super::effects::GitDiffEffect;
use crate::effects::next_request_id;

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
    /// Bumped whenever a reload replaces the document, invalidating cached patches.
    pub(super) document_revision: usize,
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
    pub(super) scroll: ScrollViewState,
    /// Rows the pane had on the last frame, for scrolling the cursor into view.
    pub(super) height: u16,
    pub(super) cursor: PatchCursor,
    pub(super) view: Option<DiffView>,
    /// Bounded MRU cache of fully-rendered, stable (draft-free) diff views for files other
    /// than the one currently selected, so switching files does not re-run syntax
    /// highlighting for recently-visited patches.
    pub(super) cache: Vec<DiffView>,
}

/// The review being assembled: comments already filed, and the one being typed.
#[derive(Default)]
pub(super) struct Review {
    pub(super) queue: ReviewQueue,
    pub(super) draft: Option<DraftState>,
    /// Bumped on every change, invalidating any patch render that drew comments.
    pub(super) revision: usize,
}

/// The git operation in flight, if any.
#[derive(Default)]
pub(super) struct Request {
    pub(super) id: u64,
    pub(super) in_flight: bool,
    /// A destructive action armed and waiting for its key to be pressed again.
    pub(super) pending: Option<PendingAction>,
}

/// Maximum number of recently-rendered file patches kept for instant re-display when the
/// user browses between files.
pub(super) const DIFF_VIEW_CACHE_LIMIT: usize = 8;

/// A rendered patch, and everything about the state it was rendered from. It is
/// reusable exactly while [`DiffViewKey`] still matches.
pub(super) struct DiffView {
    pub(super) scroll_view: ScrollView,
    pub(super) cursor_rows: Vec<CursorRow>,
    pub(super) draft_cursor: Option<(usize, u16)>,
    pub(super) key: DiffViewKey,
}

/// Identity of a rendered patch. A cached view is reused only while every part
/// of this still holds — including the draft's text and cursor, so the box the
/// user is typing into redraws on each keystroke.
#[derive(Clone, PartialEq, Eq)]
pub(super) struct DiffViewKey {
    pub(super) file_path: String,
    pub(super) content_width: u16,
    pub(super) split: bool,
    pub(super) full_file: bool,
    pub(super) document_revision: usize,
    pub(super) comments_revision: usize,
    pub(super) draft: Option<DraftKey>,
}

#[derive(Clone, PartialEq, Eq)]
pub(super) struct DraftKey {
    pub(super) anchor: PatchAnchor,
    pub(super) text_len: usize,
    pub(super) cursor: usize,
}

/// The patch line a rendered row belongs to, so the cursor can be mapped to a
/// row and back.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct CursorRow {
    pub(super) hunk: usize,
    pub(super) line: usize,
    pub(super) row: usize,
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
            document_revision: 0,
            bottom_bar: BottomBar::Help,
            show_full_file: false,
            full_file_content: None,
            patch: PatchView::default(),
            review: Review::default(),
            request: Request::default(),
        };
        let effect = screen.begin_load();
        (screen, effect)
    }

    pub(super) fn begin_load(&mut self) -> GitDiffEffect {
        if let Some(path) = self.selected_file().map(|file| file.path.clone()) {
            self.selected_path = Some(path);
        }
        self.request.pending = None;
        self.review.queue.clear();
        self.bump_comments_revision();
        self.state = GitDiffLoadState::Loading;
        GitDiffEffect::Load {
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
        self.document_revision = self.document_revision.wrapping_add(1);
        self.bump_comments_revision();
        self.patch.view = None;
        self.patch.cache.clear();
        self.state = GitDiffLoadState::Ready(document);
        self.sync_drawer_selection();
    }

    pub(super) fn stage_all(&mut self) -> Vec<SurfaceMessage> {
        self.repo_operation(|request_id, repo_root| GitDiffEffect::StageAll { request_id, repo_root })
    }

    pub(super) fn unstage_all(&mut self) -> Vec<SurfaceMessage> {
        self.repo_operation(|request_id, repo_root| GitDiffEffect::UnstageAll { request_id, repo_root })
    }

    pub(super) fn toggle_stage(&mut self) -> Vec<SurfaceMessage> {
        let entries = self.drawer_entries();
        let Some(entry) = self.drawer_selection.selected().and_then(|selected| entries.get(selected)) else {
            return Vec::new();
        };
        let files = self.files_for_entry(entry);
        if files.is_empty() {
            return Vec::new();
        }
        let all_staged = files.iter().all(|file| file.staged == StageState::Staged);
        let paths: Vec<String> = files.iter().map(|file| file.path.clone()).collect();
        self.repo_operation(move |request_id, repo_root| {
            if all_staged {
                GitDiffEffect::UnstageFiles { request_id, repo_root, paths }
            } else {
                GitDiffEffect::StageFiles { request_id, repo_root, paths }
            }
        })
    }

    pub(super) fn begin_commit(&mut self) -> Vec<SurfaceMessage> {
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

    pub(super) fn begin_discard(&mut self) -> Vec<SurfaceMessage> {
        if self.request.in_flight {
            return Vec::new();
        }
        let Some(file) = self.selected_file().cloned() else {
            return Vec::new();
        };
        self.bottom_bar = BottomBar::DiscardConfirmation { path: file.path.clone(), status: file.status };
        Vec::new()
    }

    pub(super) fn toggle_full_file(&mut self) -> Vec<SurfaceMessage> {
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
            self.repo_operation(|request_id, repo_root| GitDiffEffect::LoadFullFile { request_id, repo_root, path });
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
        let entries = self.drawer_entries();
        if entries.is_empty() {
            return;
        }
        self.drawer_selection.step_clamped(entries.len(), direction, |_| true);
        if let Some(DrawerEntry::File { index, .. }) =
            self.drawer_selection.selected().and_then(|selected| entries.get(selected))
        {
            self.selected_file = *index;
        }
    }

    /// Moves the patch cursor `amount` patch lines, flattening hunk boundaries
    /// so it walks the file continuously.
    pub(super) fn move_patch_cursor(&mut self, direction: Direction, amount: usize) {
        let Some(file) = self.selected_file() else {
            return;
        };
        let positions: Vec<PatchCursor> = file
            .hunks
            .iter()
            .enumerate()
            .flat_map(|(hunk, entry)| (0..entry.lines.len()).map(move |line| PatchCursor { hunk, line }))
            .collect();
        let Some(last) = positions.len().checked_sub(1) else {
            return;
        };
        let current = positions.iter().position(|position| *position == self.patch.cursor).unwrap_or(0);
        let next = match direction {
            Direction::Backward => current.saturating_sub(amount),
            Direction::Forward => current.saturating_add(amount).min(last),
        };
        self.patch.cursor = positions[next];
        self.sync_scroll_to_cursor();
    }

    /// Moves the currently-active diff view into the MRU cache (when it is a stable,
    /// draft-free snapshot) so it can be reused if the user switches back to its file.
    pub(super) fn park_active_diff_view(&mut self) {
        if let Some(view) = self.patch.view.take()
            && view.key.draft.is_none()
        {
            self.patch.cache.insert(0, view);
            self.patch.cache.truncate(DIFF_VIEW_CACHE_LIMIT);
        }
    }

    /// Identity of the active draft: its anchor plus the text and cursor that
    /// were rendered, so a cached view is reused only while all three match.
    pub(super) fn draft_key(&self) -> Option<DraftKey> {
        self.review.draft.as_ref().map(|draft| DraftKey {
            anchor: draft.anchor,
            text_len: draft.buffer.text().len(),
            cursor: draft.buffer.cursor(),
        })
    }

    /// Row within `view` that the patch cursor currently occupies.
    pub(super) fn cursor_row_in(&self, view: &DiffView) -> Option<usize> {
        view.cursor_rows
            .iter()
            .find(|entry| entry.hunk == self.patch.cursor.hunk && entry.line == self.patch.cursor.line)
            .map(|entry| entry.row)
    }

    pub(super) fn sync_scroll_to_cursor(&mut self) {
        let Some(cursor_row) = self.patch.view.as_ref().and_then(|view| self.cursor_row_in(view)) else {
            return;
        };
        let offset = usize::from(self.patch.scroll.offset().y);
        let viewport = usize::from(self.patch.height);
        if cursor_row < offset {
            self.set_patch_offset(cursor_row);
        } else if viewport > 0 && cursor_row >= offset + viewport {
            self.set_patch_offset(cursor_row.saturating_sub(viewport.saturating_sub(1)));
        }
    }

    pub(super) fn move_patch_scroll(&mut self, direction: Direction, amount: usize) {
        let current = usize::from(self.patch.scroll.offset().y);
        self.set_patch_offset(match direction {
            Direction::Backward => current.saturating_sub(amount),
            Direction::Forward => current.saturating_add(amount),
        });
        self.sync_cursor_to_scroll();
    }

    pub(super) fn sync_cursor_to_scroll(&mut self) {
        let Some(view) = &self.patch.view else {
            return;
        };
        let offset = usize::from(self.patch.scroll.offset().y);
        let index = match view.cursor_rows.binary_search_by_key(&offset, |entry| entry.row) {
            Ok(index) => index,
            Err(index) => index.saturating_sub(1),
        };
        if let Some(entry) = view.cursor_rows.get(index) {
            self.patch.cursor = PatchCursor { hunk: entry.hunk, line: entry.line };
        }
    }

    pub(super) fn begin_draft(&mut self) -> Vec<SurfaceMessage> {
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
            self.review.draft = Some(DraftState { anchor, buffer: EditBuffer::default() });
        }
        Vec::new()
    }

    pub(super) fn undo_last_comment(&mut self) -> Vec<SurfaceMessage> {
        self.review.queue.pop();
        self.bump_comments_revision();
        Vec::new()
    }

    pub(super) fn submit_review(&mut self) -> Vec<SurfaceMessage> {
        if self.review.queue.is_empty() {
            self.bottom_bar = BottomBar::Error("No comments to submit".to_string());
            return Vec::new();
        }
        if self.request.in_flight {
            self.bottom_bar = BottomBar::Error("Already submitting".to_string());
            return Vec::new();
        }
        vec![SurfaceMessage::SubmitReview(self.review.queue.format_prompt())]
    }

    pub(super) fn bump_comments_revision(&mut self) {
        self.review.revision = self.review.revision.wrapping_add(1);
    }

    /// Claims the next request id and marks an operation in flight, so results
    /// for anything older are dropped.
    pub(super) fn begin_request(&mut self) -> u64 {
        self.request.id = next_request_id();
        self.request.in_flight = true;
        self.request.id
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
        self.file_at(self.selected_file)
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

    fn set_patch_offset(&mut self, row: usize) {
        self.patch.scroll.set_offset(Position::new(0, u16::try_from(row).unwrap_or(u16::MAX)));
    }

    /// Runs `build` against the repository root, doing nothing when the diff has
    /// not yet reported one.
    pub(super) fn repo_operation(&mut self, build: impl FnOnce(u64, PathBuf) -> GitDiffEffect) -> Vec<SurfaceMessage> {
        let Some(repo_root) = self.repo_root.clone() else {
            return Vec::new();
        };
        effect(build(self.begin_request(), repo_root))
    }
}
