use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use ratatui::layout::Rect;

use crate::edit_buffer::EditBuffer;
use crate::git_diff::{DiffScope, FileDiff, FileStatus, GitDiffDocument, PatchAnchor, ReviewQueue, StageState};
use crate::selection::SelectionState;

use super::effects::{GitDiffEffect, GitDiffOutcome, next_request_id};

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
    pub(super) scroll_offsets: HashMap<String, DiffScrollState>,
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

#[derive(Debug, Clone, Copy, Default)]
pub(super) struct DiffScrollState {
    pub(super) vertical: usize,
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
            scroll_offsets: HashMap::new(),
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

    #[allow(clippy::cast_sign_loss)]
    pub(super) fn sync_scroll_to_cursor(&mut self) {
        let Some(file) = self.selected_file() else {
            return;
        };
        let cursor_flat_index = self.cursor_line_index(file) as usize;
        let offset_key = if self.show_full_file { format!("full:{}", file.path) } else { file.path.clone() };
        let scroll = self.scroll_offsets.entry(offset_key).or_default();
        scroll.vertical = scroll.vertical.min(cursor_flat_index);
        if cursor_flat_index >= scroll.vertical + 2 {
            scroll.vertical = cursor_flat_index.saturating_sub(1);
        }
    }

    #[allow(clippy::cast_possible_wrap, clippy::cast_sign_loss)]
    pub(super) fn move_patch_scroll(&mut self, amount: isize) {
        let Some(file) = self.selected_file().cloned() else {
            return;
        };
        let offset_key = if self.show_full_file { format!("full:{}", file.path) } else { file.path.clone() };
        let scroll = self.scroll_offsets.entry(offset_key).or_default();
        scroll.vertical = scroll.vertical.saturating_add_signed(amount);
        if file.hunks.is_empty() {
            return;
        }
        let total_lines = file.hunks.iter().map(|h| h.lines.len()).sum::<usize>();
        scroll.vertical = scroll.vertical.min(total_lines.saturating_sub(1));

        let mut remaining = scroll.vertical as isize;
        for (hunk_idx, hunk) in file.hunks.iter().enumerate() {
            let hunk_len = hunk.lines.len() as isize;
            if remaining < hunk_len {
                self.patch_cursor = PatchCursor { hunk: hunk_idx, line: remaining as usize };
                break;
            }
            remaining -= hunk_len;
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
