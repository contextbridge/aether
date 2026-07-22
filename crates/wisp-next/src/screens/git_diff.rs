use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use futures::FutureExt;
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};

use crate::git_diff::{
    CommentContext, DiffScope, FileDiff, FileStatus, GitDiffDocument, GitDiffError, PatchAnchor, PatchLine,
    PatchLineKind, QueuedComment, ReviewQueue, StageState, commit, discard_file, stage_all, stage_files, unstage_all,
    unstage_files,
};
use crate::syntax::SyntaxHighlighter;
use crate::theme::Theme;
use crate::wrap::wrap_line;

pub struct GitDiffScreen {
    working_dir: PathBuf,
    repo_root: Option<PathBuf>,
    scope: DiffScope,
    state: GitDiffLoadState,
    selected_file: usize,
    selected_path: Option<String>,
    selected_drawer_row: usize,
    focus: Focus,
    collapsed: HashSet<String>,
    scroll_offsets: HashMap<String, usize>,
    request_id: u64,
    operation_in_flight: bool,
    bottom_bar: BottomBar,
    show_full_file: bool,
    full_file_content: Option<String>,
    review_queue: ReviewQueue,
    draft: Option<DraftState>,
    patch_cursor: PatchCursor,
    pending_action: Option<PendingAction>,
    last_area: Rect,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PendingAction {
    Reload,
    ScopeSwitch,
    Stage,
    Commit,
    Discard,
}

struct DraftState {
    anchor: PatchAnchor,
    text: String,
    byte_cursor: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
struct PatchCursor {
    hunk: usize,
    line: usize,
}

pub enum GitDiffEffect {
    Load { request_id: u64, working_dir: PathBuf, repo_root: Option<PathBuf>, scope: DiffScope },
    StageFiles { request_id: u64, repo_root: PathBuf, paths: Vec<String> },
    UnstageFiles { request_id: u64, repo_root: PathBuf, paths: Vec<String> },
    StageAll { request_id: u64, repo_root: PathBuf },
    UnstageAll { request_id: u64, repo_root: PathBuf },
    Commit { request_id: u64, repo_root: PathBuf, message: String },
    DiscardFile { request_id: u64, repo_root: PathBuf, path: String, status: FileStatus },
    LoadFullFile { request_id: u64, repo_root: PathBuf, path: String },
    SubmitReview { request_id: u64, prompt: String },
}

pub enum GitDiffEvent {
    Loaded { request_id: u64, result: Result<GitDiffDocument, GitDiffError> },
    ActionFinished { request_id: u64, result: Result<(), GitDiffError> },
    FullFileLoaded { request_id: u64, path: String, result: Result<String, GitDiffError> },
    SubmitReview { request_id: u64, prompt: String },
}

pub enum GitDiffOutcome {
    None,
    Close,
    Effect(GitDiffEffect),
    SubmitReview(GitDiffEffect),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Focus {
    Drawer,
    Patch,
}

enum GitDiffLoadState {
    Loading,
    Ready(GitDiffDocument),
    Error(String),
}

#[derive(Clone)]
enum DrawerEntry {
    Directory { path: String, depth: usize },
    File { index: usize, depth: usize },
}

enum BottomBar {
    Help,
    CommitEditor { message: String, cursor: usize },
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
            selected_drawer_row: 0,
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

    #[allow(clippy::too_many_lines)]
    pub fn on_mouse_scroll_up(&mut self, _local_y: u16, _local_x: u16) {
        if self.operation_in_flight {
            return;
        }
        if self.focus == Focus::Patch {
            self.move_patch_scroll(-3);
        } else {
            self.move_vertical(-1);
        }
    }

    pub fn on_mouse_scroll_down(&mut self, _local_y: u16, _local_x: u16) {
        if self.operation_in_flight {
            return;
        }
        if self.focus == Focus::Patch {
            self.move_patch_scroll(3);
        } else {
            self.move_vertical(1);
        }
    }

    pub fn on_mouse_click(&mut self, local_y: u16, local_x: u16) {
        if self.operation_in_flight {
            return;
        }
        if self.draft.is_some() {
            return;
        }
        match &self.state {
            GitDiffLoadState::Ready(_) => {}
            _ => return,
        }
        if local_y < 1 {
            return;
        }
        let body_y = local_y.saturating_sub(1);
        let area = self.last_area;
        if area.width >= 72 {
            let drawer_width = (area.width / 3).clamp(24, 36);
            let body_x = area.x.saturating_add(1);
            if local_x >= body_x && local_x < body_x + drawer_width {
                self.focus = Focus::Drawer;
                let entries = self.drawer_entries();
                if !entries.is_empty() {
                    let sel = self.selected_drawer_row.min(entries.len().saturating_sub(1));
                    let start = sel.saturating_sub(usize::from(body_y));
                    let target = (start + usize::from(body_y)).min(entries.len().saturating_sub(1));
                    self.selected_drawer_row = target;
                    if let Some(DrawerEntry::File { index, .. }) = entries.get(target) {
                        self.selected_file = *index;
                    }
                }
            } else {
                self.focus = Focus::Patch;
            }
        } else {
            self.focus = Focus::Patch;
        }
    }

    #[allow(clippy::too_many_lines)]
    pub fn on_key(&mut self, key: KeyEvent) -> GitDiffOutcome {
        if matches!(key.code, KeyCode::Esc)
            || key.code == KeyCode::Char('g') && key.modifiers.contains(KeyModifiers::CONTROL)
        {
            if self.pending_action.is_some() {
                self.pending_action = None;
                self.bottom_bar = BottomBar::Help;
                return GitDiffOutcome::None;
            }
            match &self.bottom_bar {
                BottomBar::CommitEditor { .. } | BottomBar::DiscardConfirmation { .. } => {
                    self.bottom_bar = BottomBar::Help;
                    return GitDiffOutcome::None;
                }
                BottomBar::Error(_) => {
                    self.bottom_bar = BottomBar::Help;
                    return GitDiffOutcome::Close;
                }
                BottomBar::Help => return GitDiffOutcome::Close,
            }
        }

        if let Some(action) = self.pending_action {
            if Self::is_confirm_key(&key, action) {
                self.bottom_bar = BottomBar::Help;
            } else {
                self.bottom_bar = BottomBar::Help;
                self.pending_action = None;
                return GitDiffOutcome::None;
            }
        }

        match &self.bottom_bar {
            BottomBar::CommitEditor { .. } => return self.on_commit_editor_key(key),
            BottomBar::DiscardConfirmation { .. } => return self.on_discard_confirm_key(key),
            BottomBar::Error(_) => {
                self.bottom_bar = BottomBar::Help;
                return GitDiffOutcome::None;
            }
            BottomBar::Help => {}
        }

        if self.draft.is_some() {
            return self.on_draft_key(key);
        }

        if self.operation_in_flight {
            return GitDiffOutcome::None;
        }

        match key.code {
            KeyCode::Char('t') | KeyCode::Tab => {
                if self.needs_comment_confirmation(PendingAction::ScopeSwitch) {
                    return GitDiffOutcome::None;
                }
                self.scope = self.scope.next();
                self.show_full_file = false;
                self.full_file_content = None;
                self.patch_cursor = PatchCursor::default();
                GitDiffOutcome::Effect(self.begin_load())
            }
            KeyCode::Char('r') => {
                if self.needs_comment_confirmation(PendingAction::Reload) {
                    return GitDiffOutcome::None;
                }
                self.show_full_file = false;
                self.full_file_content = None;
                self.patch_cursor = PatchCursor::default();
                GitDiffOutcome::Effect(self.begin_load())
            }
            KeyCode::Char('a' | 'A' | ' ') => {
                if self.needs_comment_confirmation(PendingAction::Stage) {
                    return GitDiffOutcome::None;
                }
                match key.code {
                    KeyCode::Char('a') => self.stage_all(),
                    KeyCode::Char('A') => self.unstage_all(),
                    KeyCode::Char(' ') => self.toggle_stage(),
                    _ => GitDiffOutcome::None,
                }
            }
            KeyCode::Char('C') => {
                if self.needs_comment_confirmation(PendingAction::Commit) {
                    return GitDiffOutcome::None;
                }
                self.begin_commit()
            }
            KeyCode::Char('d') => {
                if self.needs_comment_confirmation(PendingAction::Discard) {
                    return GitDiffOutcome::None;
                }
                self.begin_discard()
            }
            KeyCode::Char('c') if self.focus == Focus::Patch => self.begin_draft(),
            KeyCode::Char('u') if self.focus == Focus::Patch => self.undo_last_comment(),
            KeyCode::Char('s') if self.focus == Focus::Patch => self.submit_review(),
            KeyCode::Char('o') if self.focus == Focus::Patch => self.toggle_full_file(),
            KeyCode::Left | KeyCode::Char('h') => {
                if self.focus == Focus::Patch {
                    self.focus = Focus::Drawer;
                } else {
                    self.collapse_selected();
                }
                GitDiffOutcome::None
            }
            KeyCode::Right | KeyCode::Char('l') | KeyCode::Enter => {
                if self.focus == Focus::Drawer && !self.expand_or_open_selected() {
                    self.focus = Focus::Patch;
                }
                GitDiffOutcome::None
            }
            KeyCode::Up | KeyCode::Char('k') => {
                self.move_vertical(-1);
                GitDiffOutcome::None
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.move_vertical(1);
                GitDiffOutcome::None
            }
            KeyCode::PageUp => {
                self.move_patch_scroll(-10);
                GitDiffOutcome::None
            }
            KeyCode::PageDown => {
                self.move_patch_scroll(10);
                GitDiffOutcome::None
            }
            _ => GitDiffOutcome::None,
        }
    }

    pub fn on_event(&mut self, event: GitDiffEvent) -> Option<GitDiffEffect> {
        match event {
            GitDiffEvent::Loaded { request_id, result } if request_id == self.request_id => {
                self.operation_in_flight = false;
                match result {
                    Ok(document) => self.apply_document(document),
                    Err(error) => self.state = GitDiffLoadState::Error(error.to_string()),
                }
                None
            }
            GitDiffEvent::ActionFinished { request_id, result } if request_id == self.request_id => {
                self.operation_in_flight = false;
                match result {
                    Ok(()) => {
                        self.show_full_file = false;
                        self.full_file_content = None;
                        Some(self.begin_load())
                    }
                    Err(error) => {
                        self.bottom_bar = BottomBar::Error(error.to_string());
                        None
                    }
                }
            }
            GitDiffEvent::FullFileLoaded { request_id, path: _, result } if request_id == self.request_id => {
                self.operation_in_flight = false;
                match result {
                    Ok(content) => {
                        self.full_file_content = Some(content);
                    }
                    Err(error) => {
                        self.show_full_file = false;
                        self.full_file_content = None;
                        self.bottom_bar = BottomBar::Error(error.to_string());
                    }
                }
                None
            }
            GitDiffEvent::SubmitReview { request_id, prompt: _ } if request_id == self.request_id => {
                self.operation_in_flight = false;
                None
            }
            _ => None,
        }
    }

    pub fn render(&mut self, frame: &mut Frame, theme: &Theme, highlighter: &mut SyntaxHighlighter) {
        let area = frame.area();
        self.last_area = area;
        frame.render_widget(Clear, area);
        let block = Block::default()
            .borders(Borders::ALL)
            .title(format!(" Git Diff · {} ", self.scope.label()))
            .border_style(Style::new().fg(theme.accent).add_modifier(Modifier::BOLD));
        let inner = block.inner(area);
        frame.render_widget(block, area);

        let [body, footer] = Layout::vertical([Constraint::Min(0), Constraint::Length(1)]).areas(inner);
        match &self.state {
            GitDiffLoadState::Loading => {
                frame.render_widget(
                    Paragraph::new(Line::styled("Loading changes…", Style::new().fg(theme.muted))),
                    body,
                );
            }
            GitDiffLoadState::Error(message) => {
                frame.render_widget(
                    Paragraph::new(Line::styled(
                        format!("Git diff unavailable: {message}"),
                        Style::new().fg(theme.error),
                    )),
                    body,
                );
            }
            GitDiffLoadState::Ready(document) if document.files.is_empty() => {
                frame.render_widget(
                    Paragraph::new(Line::styled(
                        "No changes in working tree for this scope",
                        Style::new().fg(theme.muted),
                    )),
                    body,
                );
            }
            GitDiffLoadState::Ready(_) => self.render_document(frame, body, theme, highlighter),
        }

        self.render_footer(frame, footer, theme);
    }

    pub fn cancel(&mut self) {}

    pub fn current_request_id(&self) -> u64 {
        self.request_id
    }

    fn begin_load(&mut self) -> GitDiffEffect {
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

    fn apply_document(&mut self, document: GitDiffDocument) {
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

    fn stage_all(&mut self) -> GitDiffOutcome {
        let Some(repo_root) = self.repo_root.clone() else {
            return GitDiffOutcome::None;
        };
        self.request_id = next_request_id();
        self.operation_in_flight = true;
        GitDiffOutcome::Effect(GitDiffEffect::StageAll { request_id: self.request_id, repo_root })
    }

    fn unstage_all(&mut self) -> GitDiffOutcome {
        let Some(repo_root) = self.repo_root.clone() else {
            return GitDiffOutcome::None;
        };
        self.request_id = next_request_id();
        self.operation_in_flight = true;
        GitDiffOutcome::Effect(GitDiffEffect::UnstageAll { request_id: self.request_id, repo_root })
    }

    fn toggle_stage(&mut self) -> GitDiffOutcome {
        let Some(repo_root) = self.repo_root.clone() else {
            return GitDiffOutcome::None;
        };
        let entries = self.drawer_entries();
        let Some(entry) = entries.get(self.selected_drawer_row) else {
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

    fn begin_commit(&mut self) -> GitDiffOutcome {
        if self.operation_in_flight {
            return GitDiffOutcome::None;
        }
        if !self.any_staged() {
            self.bottom_bar = BottomBar::Error("Nothing staged to commit".to_string());
            return GitDiffOutcome::None;
        }
        self.bottom_bar = BottomBar::CommitEditor { message: String::new(), cursor: 0 };
        GitDiffOutcome::None
    }

    fn on_commit_editor_key(&mut self, key: KeyEvent) -> GitDiffOutcome {
        if key.code == KeyCode::Esc || key.code == KeyCode::Char('g') && key.modifiers.contains(KeyModifiers::CONTROL) {
            self.bottom_bar = BottomBar::Help;
            return GitDiffOutcome::None;
        }
        match key.code {
            KeyCode::Enter => {
                let BottomBar::CommitEditor { message, .. } = std::mem::replace(&mut self.bottom_bar, BottomBar::Help)
                else {
                    return GitDiffOutcome::None;
                };
                let trimmed = message.trim().to_string();
                if trimmed.is_empty() {
                    self.bottom_bar = BottomBar::Error("Commit message cannot be empty".to_string());
                    return GitDiffOutcome::None;
                }
                let Some(repo_root) = self.repo_root.clone() else {
                    return GitDiffOutcome::None;
                };
                self.request_id = next_request_id();
                self.operation_in_flight = true;
                GitDiffOutcome::Effect(GitDiffEffect::Commit {
                    request_id: self.request_id,
                    repo_root,
                    message: trimmed,
                })
            }
            KeyCode::Char(c) => {
                if let BottomBar::CommitEditor { message, cursor } = &mut self.bottom_bar {
                    let pos = (*cursor).min(message.len());
                    message.insert(pos, c);
                    *cursor = pos + c.len_utf8();
                }
                GitDiffOutcome::None
            }
            KeyCode::Backspace => {
                if let BottomBar::CommitEditor { message, cursor } = &mut self.bottom_bar
                    && *cursor > 0
                {
                    let prev = message.floor_char_boundary(*cursor - 1);
                    message.remove(prev);
                    *cursor = prev;
                }
                GitDiffOutcome::None
            }
            KeyCode::Delete => {
                if let BottomBar::CommitEditor { message, cursor } = &mut self.bottom_bar
                    && *cursor < message.len()
                {
                    let end = message.ceil_char_boundary(*cursor + 1);
                    message.drain(*cursor..end);
                }
                GitDiffOutcome::None
            }
            KeyCode::Left => {
                if let BottomBar::CommitEditor { message, cursor } = &mut self.bottom_bar
                    && *cursor > 0
                {
                    *cursor = message.floor_char_boundary(*cursor - 1);
                }
                GitDiffOutcome::None
            }
            KeyCode::Right => {
                if let BottomBar::CommitEditor { message, cursor } = &mut self.bottom_bar
                    && *cursor < message.len()
                {
                    *cursor = message.ceil_char_boundary(*cursor + 1);
                }
                GitDiffOutcome::None
            }
            KeyCode::Home => {
                if let BottomBar::CommitEditor { cursor, .. } = &mut self.bottom_bar {
                    *cursor = 0;
                }
                GitDiffOutcome::None
            }
            KeyCode::End => {
                if let BottomBar::CommitEditor { message, cursor } = &mut self.bottom_bar {
                    *cursor = message.len();
                }
                GitDiffOutcome::None
            }
            _ => GitDiffOutcome::None,
        }
    }

    fn begin_discard(&mut self) -> GitDiffOutcome {
        if self.operation_in_flight {
            return GitDiffOutcome::None;
        }
        let Some(file) = self.selected_file().cloned() else {
            return GitDiffOutcome::None;
        };
        self.bottom_bar = BottomBar::DiscardConfirmation { path: file.path.clone(), status: file.status };
        GitDiffOutcome::None
    }

    fn on_discard_confirm_key(&mut self, key: KeyEvent) -> GitDiffOutcome {
        match key.code {
            KeyCode::Char('y' | 'Y') => {
                let BottomBar::DiscardConfirmation { path, status } =
                    std::mem::replace(&mut self.bottom_bar, BottomBar::Help)
                else {
                    return GitDiffOutcome::None;
                };
                let Some(repo_root) = self.repo_root.clone() else {
                    return GitDiffOutcome::None;
                };
                self.request_id = next_request_id();
                self.operation_in_flight = true;
                GitDiffOutcome::Effect(GitDiffEffect::DiscardFile {
                    request_id: self.request_id,
                    repo_root,
                    path,
                    status,
                })
            }
            KeyCode::Char('n' | 'N') | KeyCode::Esc => {
                self.bottom_bar = BottomBar::Help;
                GitDiffOutcome::None
            }
            _ => GitDiffOutcome::None,
        }
    }

    fn toggle_full_file(&mut self) -> GitDiffOutcome {
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

    fn any_staged(&self) -> bool {
        matches!(&self.state, GitDiffLoadState::Ready(document)
            if document.files.iter().any(|file| matches!(file.staged, StageState::Staged | StageState::PartiallyStaged)))
    }

    fn move_vertical(&mut self, amount: isize) {
        if self.focus == Focus::Patch {
            self.move_patch_cursor(amount);
            return;
        }
        let entries = self.drawer_entries();
        if entries.is_empty() {
            return;
        }
        self.selected_drawer_row = self.selected_drawer_row.saturating_add_signed(amount).min(entries.len() - 1);
        if let Some(DrawerEntry::File { index, .. }) = entries.get(self.selected_drawer_row) {
            self.selected_file = *index;
        }
    }

    #[allow(clippy::cast_possible_wrap, clippy::cast_sign_loss)]
    fn move_patch_cursor(&mut self, amount: isize) {
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
    fn cursor_line_index(&self, file: &FileDiff) -> isize {
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
    fn sync_scroll_to_cursor(&mut self) {
        let Some(file) = self.selected_file() else {
            return;
        };
        let cursor_flat_index = self.cursor_line_index(file) as usize;
        let offset_key = if self.show_full_file { format!("full:{}", file.path) } else { file.path.clone() };
        let offset = self.scroll_offsets.entry(offset_key).or_default();
        *offset = (*offset).min(cursor_flat_index);
        if cursor_flat_index >= *offset + 2 {
            *offset = cursor_flat_index.saturating_sub(1);
        }
    }

    #[allow(clippy::cast_possible_wrap, clippy::cast_sign_loss)]
    fn move_patch_scroll(&mut self, amount: isize) {
        let Some(file) = self.selected_file().cloned() else {
            return;
        };
        let offset_key = if self.show_full_file { format!("full:{}", file.path) } else { file.path.clone() };
        let offset = self.scroll_offsets.entry(offset_key).or_default();
        *offset = offset.saturating_add_signed(amount);
        if file.hunks.is_empty() {
            return;
        }
        let total_lines = file.hunks.iter().map(|h| h.lines.len()).sum::<usize>();
        *offset = (*offset).min(total_lines.saturating_sub(1));

        let mut remaining = *offset as isize;
        for (hunk_idx, hunk) in file.hunks.iter().enumerate() {
            let hunk_len = hunk.lines.len() as isize;
            if remaining < hunk_len {
                self.patch_cursor = PatchCursor { hunk: hunk_idx, line: remaining as usize };
                break;
            }
            remaining -= hunk_len;
        }
    }

    fn begin_draft(&mut self) -> GitDiffOutcome {
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
        self.draft = Some(DraftState { anchor, text: String::new(), byte_cursor: 0 });
        GitDiffOutcome::None
    }

    fn on_draft_key(&mut self, key: KeyEvent) -> GitDiffOutcome {
        let Some(draft) = self.draft.as_mut() else {
            return GitDiffOutcome::None;
        };
        match key.code {
            KeyCode::Esc => {
                self.draft = None;
                GitDiffOutcome::None
            }
            KeyCode::Enter => {
                let draft = self.draft.take().expect("draft exists");
                let text = draft.text;
                if text.trim().is_empty() {
                    return GitDiffOutcome::None;
                }
                let Some(file) = self.selected_file() else {
                    return GitDiffOutcome::None;
                };
                let Some(hunk) = file.hunks.get(draft.anchor.hunk) else {
                    return GitDiffOutcome::None;
                };
                let Some(patch_line) = hunk.lines.get(draft.anchor.line) else {
                    return GitDiffOutcome::None;
                };
                let line_number = patch_line.new_line_no.or(patch_line.old_line_no);
                self.review_queue.push(QueuedComment {
                    anchor: draft.anchor,
                    body: text,
                    context: CommentContext {
                        file_path: file.path.clone(),
                        line_text: patch_line.text.clone(),
                        line_number,
                        line_kind: patch_line.kind,
                    },
                });
                GitDiffOutcome::None
            }
            KeyCode::Char(c) => {
                let pos = draft.byte_cursor.min(draft.text.len());
                draft.text.insert(pos, c);
                draft.byte_cursor = pos + c.len_utf8();
                GitDiffOutcome::None
            }
            KeyCode::Backspace => {
                if draft.byte_cursor > 0 {
                    let prev = draft.text.floor_char_boundary(draft.byte_cursor - 1);
                    draft.text.remove(prev);
                    draft.byte_cursor = prev;
                }
                GitDiffOutcome::None
            }
            KeyCode::Delete => {
                if draft.byte_cursor < draft.text.len() {
                    let end = draft.text.ceil_char_boundary(draft.byte_cursor + 1);
                    draft.text.drain(draft.byte_cursor..end);
                }
                GitDiffOutcome::None
            }
            KeyCode::Left => {
                if draft.byte_cursor > 0 {
                    draft.byte_cursor = draft.text.floor_char_boundary(draft.byte_cursor - 1);
                }
                GitDiffOutcome::None
            }
            KeyCode::Right => {
                if draft.byte_cursor < draft.text.len() {
                    draft.byte_cursor = draft.text.ceil_char_boundary(draft.byte_cursor + 1);
                }
                GitDiffOutcome::None
            }
            KeyCode::Home => {
                draft.byte_cursor = 0;
                GitDiffOutcome::None
            }
            KeyCode::End => {
                draft.byte_cursor = draft.text.len();
                GitDiffOutcome::None
            }
            _ => GitDiffOutcome::None,
        }
    }

    fn undo_last_comment(&mut self) -> GitDiffOutcome {
        self.review_queue.pop();
        GitDiffOutcome::None
    }

    fn submit_review(&mut self) -> GitDiffOutcome {
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

    fn needs_comment_confirmation(&mut self, action: PendingAction) -> bool {
        if self.review_queue.is_empty() {
            return false;
        }
        if self.pending_action == Some(action) {
            return false;
        }
        let count = self.review_queue.len();
        let label = match action {
            PendingAction::Reload => ("Reloading", "r"),
            PendingAction::ScopeSwitch => ("Switching scope", "t"),
            PendingAction::Stage => ("Staging/unstaging", "space/a/A"),
            PendingAction::Commit => ("Committing", "C"),
            PendingAction::Discard => ("Discarding", "d"),
        };
        self.pending_action = Some(action);
        self.bottom_bar = BottomBar::Error(format!(
            "{label} will clear {count} review comment{}. Press {key} again to confirm or Esc to cancel.",
            if count == 1 { "" } else { "s" },
            label = label.0,
            key = label.1
        ));
        true
    }

    fn is_confirm_key(key: &KeyEvent, action: PendingAction) -> bool {
        matches!(
            (action, key.code),
            (PendingAction::Reload, KeyCode::Char('r'))
                | (PendingAction::ScopeSwitch, KeyCode::Char('t') | KeyCode::Tab)
                | (PendingAction::Stage, KeyCode::Char('a' | 'A' | ' '))
                | (PendingAction::Commit, KeyCode::Char('C'))
                | (PendingAction::Discard, KeyCode::Char('d'))
        )
    }

    fn collapse_selected(&mut self) {
        let entries = self.drawer_entries();
        if let Some(DrawerEntry::Directory { path, .. }) = entries.get(self.selected_drawer_row) {
            self.collapsed.insert(path.clone());
            self.sync_drawer_selection();
        }
    }

    fn expand_or_open_selected(&mut self) -> bool {
        let entries = self.drawer_entries();
        match entries.get(self.selected_drawer_row) {
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

    fn sync_drawer_selection(&mut self) {
        let entries = self.drawer_entries();
        self.selected_drawer_row = entries
            .iter()
            .position(|entry| matches!(entry, DrawerEntry::File { index, .. } if *index == self.selected_file))
            .unwrap_or(0);
    }

    fn selected_file(&self) -> Option<&FileDiff> {
        let GitDiffLoadState::Ready(document) = &self.state else {
            return None;
        };
        document.files.get(self.selected_file)
    }

    fn drawer_entries(&self) -> Vec<DrawerEntry> {
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

    fn files_for_entry(&self, entry: &DrawerEntry) -> Vec<&FileDiff> {
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

    fn render_document(&mut self, frame: &mut Frame, area: Rect, theme: &Theme, highlighter: &mut SyntaxHighlighter) {
        if area.width >= 72 {
            let drawer_width = (area.width / 3).clamp(24, 36);
            let [drawer, separator, patch] =
                Layout::horizontal([Constraint::Length(drawer_width), Constraint::Length(1), Constraint::Min(1)])
                    .areas(area);
            self.render_drawer(frame, drawer, theme);
            frame.render_widget(Paragraph::new("│").style(Style::new().fg(theme.muted)), separator);
            self.render_patch(frame, patch, theme, highlighter);
        } else {
            self.render_patch(frame, area, theme, highlighter);
        }
    }

    fn render_drawer(&self, frame: &mut Frame, area: Rect, theme: &Theme) {
        let entries = self.drawer_entries();
        let selected = self.selected_drawer_row.min(entries.len().saturating_sub(1));
        let visible = usize::from(area.height);
        let start = selected.saturating_sub(visible.saturating_sub(1));
        let lines = entries
            .iter()
            .enumerate()
            .skip(start)
            .take(visible)
            .map(|(row, entry)| self.drawer_line(entry, row == selected, usize::from(area.width), theme))
            .collect::<Vec<_>>();
        frame.render_widget(Paragraph::new(Text::from(lines)), area);
    }

    fn drawer_line(&self, entry: &DrawerEntry, selected: bool, width: usize, theme: &Theme) -> Line<'static> {
        let selection = selected && self.focus == Focus::Drawer;
        let selected_style = Style::new().fg(theme.background).bg(theme.accent).add_modifier(Modifier::BOLD);
        let mut line = match entry {
            DrawerEntry::Directory { path, depth } => {
                let name = path.rsplit('/').next().unwrap_or(path);
                let marker = if self.collapsed.contains(path) { "▸" } else { "▾" };
                Line::from(vec![
                    Span::raw(format!("{}{} ", "  ".repeat(*depth), marker)),
                    Span::styled(format!("{name}/"), Style::new().fg(theme.info)),
                ])
            }
            DrawerEntry::File { index, depth } => {
                let Some(file) = self.file_at(*index) else {
                    return Line::default();
                };
                let name = file.path.rsplit('/').next().unwrap_or(&file.path);
                let stage = match file.staged {
                    StageState::Unstaged => "☐",
                    StageState::Staged => "☑",
                    StageState::PartiallyStaged => "◩",
                };
                Line::from(vec![
                    Span::raw(format!("{}{} ", "  ".repeat(*depth), stage)),
                    Span::styled(
                        file.status.marker().to_string(),
                        Style::new().fg(file_status_color(file.status, theme)),
                    ),
                    Span::raw(format!(" {name}")),
                    Span::styled(format!(" +{} -{}", file.additions(), file.deletions()), Style::new().fg(theme.muted)),
                ])
            }
        };
        if selection {
            line = line.style(selected_style);
        }
        fit_line(line, width, if selection { selected_style } else { Style::new().fg(theme.text_primary) })
    }

    fn render_patch(&mut self, frame: &mut Frame, area: Rect, theme: &Theme, highlighter: &mut SyntaxHighlighter) {
        let Some(file) = self.selected_file().cloned() else {
            return;
        };
        let header_style = if self.focus == Focus::Patch {
            Style::new().fg(theme.accent).add_modifier(Modifier::BOLD)
        } else {
            Style::new().fg(theme.text_primary).add_modifier(Modifier::BOLD)
        };
        let comment_count = self.review_queue.comments_for_file(&file.path).count();
        let header = if self.show_full_file {
            Line::from(vec![
                Span::styled(format!(" {}  {}", file.path, file.status.label()), header_style),
                Span::styled(format!("  +{} -{}", file.additions(), file.deletions()), Style::new().fg(theme.muted)),
                Span::styled("  [full file]", Style::new().fg(theme.info)),
            ])
        } else if comment_count > 0 {
            Line::from(vec![
                Span::styled(format!(" {}  {}", file.path, file.status.label()), header_style),
                Span::styled(format!("  +{} -{}", file.additions(), file.deletions()), Style::new().fg(theme.muted)),
                Span::styled(
                    format!("  {comment_count} comment{}", if comment_count == 1 { "" } else { "s" }),
                    Style::new().fg(theme.info),
                ),
            ])
        } else {
            Line::from(vec![
                Span::styled(format!(" {}  {}", file.path, file.status.label()), header_style),
                Span::styled(format!("  +{} -{}", file.additions(), file.deletions()), Style::new().fg(theme.muted)),
            ])
        };
        let [header_area, content_area] = Layout::vertical([Constraint::Length(1), Constraint::Min(0)]).areas(area);
        frame.render_widget(Paragraph::new(header), header_area);

        let mut draft_cursor = None;
        let lines = if self.show_full_file {
            self.render_full_file(&file, content_area.width, theme, highlighter)
        } else if file.binary {
            vec![Line::styled("Binary file", Style::new().fg(theme.muted))]
        } else if area.width >= 96 {
            self.render_split_with_comments(&file, area.width, theme, highlighter, &mut draft_cursor)
        } else {
            self.render_unified_with_comments(&file, area.width, theme, highlighter, &mut draft_cursor)
        };
        let offset_key = if self.show_full_file { format!("full:{}", file.path) } else { file.path.clone() };
        let offset = self.scroll_offsets.entry(offset_key).or_default();
        *offset = (*offset).min(lines.len().saturating_sub(1));
        let visible = lines.into_iter().skip(*offset).take(usize::from(content_area.height)).collect::<Vec<_>>();
        frame.render_widget(Paragraph::new(Text::from(visible)), content_area);

        if let Some((draft_line, draft_col)) = draft_cursor
            && draft_line >= *offset
            && draft_line < *offset + usize::from(content_area.height)
        {
            let row = content_area.y + u16::try_from(draft_line - *offset).unwrap_or(u16::MAX);
            let col = (content_area.x + draft_col).min(content_area.right().saturating_sub(1));
            frame.set_cursor_position((col, row));
        }
    }

    fn render_full_file(
        &self,
        file: &FileDiff,
        width: u16,
        theme: &Theme,
        highlighter: &mut SyntaxHighlighter,
    ) -> Vec<Line<'static>> {
        if file.status == FileStatus::Deleted {
            return vec![Line::styled("File has been deleted", Style::new().fg(theme.muted))];
        }
        if file.binary {
            return vec![Line::styled("Binary file — cannot display contents", Style::new().fg(theme.muted))];
        }
        match &self.full_file_content {
            None => {
                vec![Line::styled("Loading file…", Style::new().fg(theme.muted))]
            }
            Some(content) => {
                let language = file.language();
                let background = theme.background;
                content
                    .lines()
                    .enumerate()
                    .map(|(index, text)| {
                        let line_no = format!("{:>4} ", index + 1);
                        let style = Style::new().fg(theme.text_secondary).bg(background);
                        let mut spans = vec![Span::styled(line_no, style)];
                        spans.extend(highlighted_spans(text, language, background, theme, highlighter));
                        fit_line(
                            Line::from(spans).style(Style::new().bg(background)),
                            usize::from(width),
                            Style::new().fg(theme.text_secondary).bg(background),
                        )
                    })
                    .collect()
            }
        }
    }

    fn render_footer(&self, frame: &mut Frame, area: Rect, theme: &Theme) {
        match &self.bottom_bar {
            BottomBar::CommitEditor { message, cursor } => {
                let prefix = "commit › ";
                let prefix_width: u16 = u16::try_from(prefix.len()).unwrap_or(u16::MAX);
                let avail = area.width.saturating_sub(prefix_width).max(1) as usize;
                let displayed = if message.len() <= avail {
                    message.clone()
                } else {
                    message[message.len().saturating_sub(avail)..].to_string()
                };
                let scroll_start = message.len().saturating_sub(avail);
                let cursor_col: u16 = if *cursor <= scroll_start {
                    prefix_width
                } else {
                    let offset = (*cursor - scroll_start).min(displayed.len());
                    let chars_before = displayed[..offset].chars().count();
                    u16::try_from(prefix_width as usize + chars_before).unwrap_or(u16::MAX)
                };
                let line = Line::from(vec![
                    Span::styled(prefix.to_string(), Style::new().fg(theme.accent).add_modifier(Modifier::BOLD)),
                    Span::styled(displayed, Style::new().fg(theme.text_primary)),
                ]);
                frame.render_widget(Paragraph::new(line), area);
                frame.set_cursor_position(((area.x + cursor_col).min(area.right().saturating_sub(1)), area.y));
            }
            BottomBar::DiscardConfirmation { path, status } => {
                let status_label = format!("({})", status.label());
                let line = Line::from(vec![
                    Span::styled("Discard changes to ", Style::new().fg(theme.warning)),
                    Span::styled(path.clone(), Style::new().fg(theme.warning).add_modifier(Modifier::BOLD)),
                    Span::styled(format!(" {status_label}?  "), Style::new().fg(theme.warning)),
                    Span::styled("y", Style::new().fg(theme.accent)),
                    Span::styled(" confirm  ", Style::new().fg(theme.muted)),
                    Span::styled("n", Style::new().fg(theme.accent)),
                    Span::styled(" cancel", Style::new().fg(theme.muted)),
                ]);
                frame.render_widget(Paragraph::new(line), area);
            }
            BottomBar::Error(error) => {
                let line = Line::styled(error.clone(), Style::new().fg(theme.error));
                frame.render_widget(Paragraph::new(line), area);
            }
            BottomBar::Help => {
                let total = self.review_queue.len();
                let count_str = if total > 0 {
                    format!(" ({total} comment{})", if total == 1 { "" } else { "s" })
                } else {
                    String::new()
                };
                let hint = if self.focus == Focus::Drawer {
                    format!(
                        "j/k move · h/l pane · space stage · a/A all · t scope · C commit · d discard · o full file · r refresh{count_str} · Ctrl-G/Esc close"
                    )
                } else {
                    format!(
                        "j/k scroll · c comment · s submit · u undo · h/l pane · space stage · C commit · d discard · o full file · r refresh{count_str} · Ctrl-G/Esc close"
                    )
                };
                let line = Line::styled(hint, Style::new().fg(theme.muted));
                frame.render_widget(Paragraph::new(line), area);
            }
        }
    }

    fn file_at(&self, index: usize) -> Option<&FileDiff> {
        let GitDiffLoadState::Ready(document) = &self.state else {
            return None;
        };
        document.files.get(index)
    }
}

impl GitDiffEffect {
    pub async fn execute(self) -> GitDiffEvent {
        let request_id = self.request_id();
        let result = std::panic::AssertUnwindSafe(self.execute_inner()).catch_unwind().await;
        match result {
            Ok(event) => event,
            Err(_panic) => GitDiffEvent::ActionFinished {
                request_id,
                result: Err(GitDiffError::CommandFailed { stderr: "Internal error".to_string() }),
            },
        }
    }

    fn request_id(&self) -> u64 {
        match self {
            Self::Load { request_id, .. }
            | Self::StageFiles { request_id, .. }
            | Self::UnstageFiles { request_id, .. }
            | Self::StageAll { request_id, .. }
            | Self::UnstageAll { request_id, .. }
            | Self::Commit { request_id, .. }
            | Self::DiscardFile { request_id, .. }
            | Self::LoadFullFile { request_id, .. }
            | Self::SubmitReview { request_id, .. } => *request_id,
        }
    }

    async fn execute_inner(self) -> GitDiffEvent {
        match self {
            Self::Load { request_id, working_dir, repo_root, scope } => GitDiffEvent::Loaded {
                request_id,
                result: GitDiffDocument::load(&working_dir, repo_root.as_deref(), scope).await,
            },
            Self::StageFiles { request_id, repo_root, paths } => {
                GitDiffEvent::ActionFinished { request_id, result: stage_files(&repo_root, &paths).await }
            }
            Self::UnstageFiles { request_id, repo_root, paths } => {
                GitDiffEvent::ActionFinished { request_id, result: unstage_files(&repo_root, &paths).await }
            }
            Self::StageAll { request_id, repo_root } => {
                GitDiffEvent::ActionFinished { request_id, result: stage_all(&repo_root).await }
            }
            Self::UnstageAll { request_id, repo_root } => {
                GitDiffEvent::ActionFinished { request_id, result: unstage_all(&repo_root).await }
            }
            Self::Commit { request_id, repo_root, message } => {
                GitDiffEvent::ActionFinished { request_id, result: commit(&repo_root, &message).await }
            }
            Self::DiscardFile { request_id, repo_root, path, status } => {
                GitDiffEvent::ActionFinished { request_id, result: discard_file(&repo_root, &path, status).await }
            }
            Self::LoadFullFile { request_id, repo_root, path } => {
                let full_path = repo_root.join(&path);
                let result = tokio::fs::read_to_string(&full_path)
                    .await
                    .map_err(|error| GitDiffError::CommandFailed { stderr: format!("Cannot read {path}: {error}") });
                GitDiffEvent::FullFileLoaded { request_id, path, result }
            }
            Self::SubmitReview { request_id, prompt } => GitDiffEvent::SubmitReview { request_id, prompt },
        }
    }
}

static NEXT_REQUEST_ID: AtomicU64 = AtomicU64::new(1);

fn next_request_id() -> u64 {
    NEXT_REQUEST_ID.fetch_add(1, Ordering::Relaxed)
}

#[allow(dead_code)]
fn render_unified_patch(
    file: &FileDiff,
    width: u16,
    theme: &Theme,
    highlighter: &mut SyntaxHighlighter,
) -> Vec<Line<'static>> {
    file.hunks
        .iter()
        .flat_map(|hunk| {
            hunk.lines
                .iter()
                .map(|line| render_unified_line(line, file.language(), width, theme, highlighter))
                .collect::<Vec<_>>()
        })
        .collect()
}

impl GitDiffScreen {
    fn render_unified_with_comments(
        &self,
        file: &FileDiff,
        width: u16,
        theme: &Theme,
        highlighter: &mut SyntaxHighlighter,
        draft_cursor: &mut Option<(usize, u16)>,
    ) -> Vec<Line<'static>> {
        let mut lines = Vec::new();
        for (hunk_idx, hunk) in file.hunks.iter().enumerate() {
            for (line_idx, patch_line) in hunk.lines.iter().enumerate() {
                let anchor = PatchAnchor { file_index: self.selected_file, hunk: hunk_idx, line: line_idx };
                let rendered = render_unified_line(patch_line, file.language(), width, theme, highlighter);
                let is_cursor = self.focus == Focus::Patch
                    && self.patch_cursor.hunk == hunk_idx
                    && self.patch_cursor.line == line_idx
                    && self.draft.is_none();
                if is_cursor {
                    let cursor_line = add_cursor_indicator(rendered, theme);
                    lines.push(cursor_line);
                } else {
                    lines.push(rendered);
                }
                lines.extend(self.render_comments_for_anchor(anchor, width, theme));
                if let Some(draft) = &self.draft
                    && draft.anchor == anchor
                {
                    let (draft_lines, cursor) = render_draft_comment(draft, width, theme);
                    if let Some((line, col)) = cursor {
                        *draft_cursor = Some((lines.len() + line, col));
                    }
                    lines.extend(draft_lines);
                }
            }
        }
        lines
    }

    #[allow(clippy::too_many_lines)]
    fn render_split_with_comments(
        &self,
        file: &FileDiff,
        width: u16,
        theme: &Theme,
        highlighter: &mut SyntaxHighlighter,
        draft_cursor: &mut Option<(usize, u16)>,
    ) -> Vec<Line<'static>> {
        let left_width = width.saturating_sub(1) / 2;
        let right_width = width.saturating_sub(left_width + 1);
        let mut lines = Vec::new();
        for (hunk_idx, hunk) in file.hunks.iter().enumerate() {
            let mut index = 0;
            while index < hunk.lines.len() {
                let line = &hunk.lines[index];
                if line.kind == PatchLineKind::HunkHeader {
                    let anchor = PatchAnchor { file_index: self.selected_file, hunk: hunk_idx, line: index };
                    let row = fit_line(
                        Line::styled(line.text.clone(), Style::new().fg(theme.info)),
                        usize::from(width),
                        Style::new().fg(theme.info),
                    );
                    let is_cursor = self.focus == Focus::Patch
                        && self.draft.is_none()
                        && self.patch_cursor.hunk == hunk_idx
                        && self.patch_cursor.line == index;
                    if is_cursor {
                        lines.push(add_cursor_indicator(row, theme));
                    } else {
                        lines.push(row);
                    }
                    lines.extend(self.render_comments_for_anchor(anchor, width, theme));
                    if let Some(draft) = &self.draft
                        && draft.anchor == anchor
                    {
                        let (draft_lines, cursor) = render_draft_comment(draft, width, theme);
                        if let Some((line, col)) = cursor {
                            *draft_cursor = Some((lines.len() + line, col));
                        }
                        lines.extend(draft_lines);
                    }
                    index += 1;
                    continue;
                }
                if line.kind == PatchLineKind::Removed {
                    let removed_start = index;
                    while index < hunk.lines.len() && hunk.lines[index].kind == PatchLineKind::Removed {
                        index += 1;
                    }
                    let added_start = index;
                    while index < hunk.lines.len() && hunk.lines[index].kind == PatchLineKind::Added {
                        index += 1;
                    }
                    let removed = &hunk.lines[removed_start..added_start];
                    let added = &hunk.lines[added_start..index];
                    let mut block_last_line_idx = removed_start;
                    for offset in 0..removed.len().max(added.len()) {
                        block_last_line_idx = removed_start + offset;
                        let row = render_split_row(
                            removed.get(offset),
                            added.get(offset),
                            file.language(),
                            left_width,
                            right_width,
                            theme,
                            highlighter,
                        );
                        let is_cursor = self.focus == Focus::Patch
                            && self.draft.is_none()
                            && self.patch_cursor.hunk == hunk_idx
                            && self.patch_cursor.line == block_last_line_idx;
                        if is_cursor {
                            lines.push(add_cursor_indicator(row, theme));
                        } else {
                            lines.push(row);
                        }
                    }
                    let final_anchor =
                        PatchAnchor { file_index: self.selected_file, hunk: hunk_idx, line: block_last_line_idx };
                    lines.extend(self.render_comments_for_anchor(final_anchor, width, theme));
                    if let Some(draft) = &self.draft
                        && draft.anchor.hunk == hunk_idx
                        && draft.anchor.line >= removed_start
                        && draft.anchor.line < index
                    {
                        let (draft_lines, cursor) = render_draft_comment(draft, width, theme);
                        if let Some((line, col)) = cursor {
                            *draft_cursor = Some((lines.len() + line, col));
                        }
                        lines.extend(draft_lines);
                    }
                    continue;
                }
                let anchor = PatchAnchor { file_index: self.selected_file, hunk: hunk_idx, line: index };
                if line.kind == PatchLineKind::Added {
                    let row = render_split_row(
                        None,
                        Some(line),
                        file.language(),
                        left_width,
                        right_width,
                        theme,
                        highlighter,
                    );
                    let is_cursor = self.focus == Focus::Patch
                        && self.draft.is_none()
                        && self.patch_cursor.hunk == hunk_idx
                        && self.patch_cursor.line == index;
                    if is_cursor {
                        lines.push(add_cursor_indicator(row, theme));
                    } else {
                        lines.push(row);
                    }
                } else if line.kind == PatchLineKind::Context {
                    let row = render_split_row(
                        Some(line),
                        Some(line),
                        file.language(),
                        left_width,
                        right_width,
                        theme,
                        highlighter,
                    );
                    let is_cursor = self.focus == Focus::Patch
                        && self.draft.is_none()
                        && self.patch_cursor.hunk == hunk_idx
                        && self.patch_cursor.line == index;
                    if is_cursor {
                        lines.push(add_cursor_indicator(row, theme));
                    } else {
                        lines.push(row);
                    }
                } else {
                    lines.push(fit_line(
                        Line::styled(line.text.clone(), Style::new().fg(theme.muted)),
                        usize::from(width),
                        Style::new().fg(theme.muted),
                    ));
                }
                lines.extend(self.render_comments_for_anchor(anchor, width, theme));
                if let Some(draft) = &self.draft
                    && draft.anchor == anchor
                {
                    let (draft_lines, cursor) = render_draft_comment(draft, width, theme);
                    if let Some((line, col)) = cursor {
                        *draft_cursor = Some((lines.len() + line, col));
                    }
                    lines.extend(draft_lines);
                }
                index += 1;
            }
        }
        lines
    }

    fn render_comments_for_anchor(&self, anchor: PatchAnchor, width: u16, theme: &Theme) -> Vec<Line<'static>> {
        let mut lines = Vec::new();
        for comment in self.review_queue.comments() {
            if comment.anchor == anchor {
                lines.extend(render_submitted_comment(&comment.body, width, theme));
            }
        }
        lines
    }
}

fn render_unified_line(
    line: &PatchLine,
    language: &str,
    width: u16,
    theme: &Theme,
    highlighter: &mut SyntaxHighlighter,
) -> Line<'static> {
    if line.kind == PatchLineKind::HunkHeader {
        return fit_line(
            Line::styled(line.text.clone(), Style::new().fg(theme.info)),
            usize::from(width),
            Style::new().fg(theme.info),
        );
    }
    if line.kind == PatchLineKind::Meta {
        return fit_line(
            Line::styled(line.text.clone(), Style::new().fg(theme.muted)),
            usize::from(width),
            Style::new().fg(theme.muted),
        );
    }
    let (marker, foreground, background) = match line.kind {
        PatchLineKind::Added => ('+', theme.diff_added_fg, theme.diff_added_bg),
        PatchLineKind::Removed => ('-', theme.diff_removed_fg, theme.diff_removed_bg),
        _ => (' ', theme.text_secondary, theme.background),
    };
    let old = line.old_line_no.map_or_else(|| "    ".to_string(), |number| format!("{number:>4}"));
    let new = line.new_line_no.map_or_else(|| "    ".to_string(), |number| format!("{number:>4}"));
    let style = Style::new().fg(foreground).bg(background);
    let mut spans = vec![Span::styled(format!("{old} {new} {marker} "), style)];
    spans.extend(highlighted_spans(&line.text, language, background, theme, highlighter));
    fit_line(Line::from(spans).style(Style::new().bg(background)), usize::from(width), style)
}

#[allow(dead_code)]
fn render_split_patch(
    file: &FileDiff,
    width: u16,
    theme: &Theme,
    highlighter: &mut SyntaxHighlighter,
) -> Vec<Line<'static>> {
    let left_width = width.saturating_sub(1) / 2;
    let right_width = width.saturating_sub(left_width + 1);
    let mut rendered = Vec::new();
    for hunk in &file.hunks {
        let mut index = 0;
        while index < hunk.lines.len() {
            let line = &hunk.lines[index];
            if line.kind == PatchLineKind::HunkHeader {
                rendered.push(fit_line(
                    Line::styled(line.text.clone(), Style::new().fg(theme.info)),
                    usize::from(width),
                    Style::new().fg(theme.info),
                ));
                index += 1;
                continue;
            }
            if line.kind == PatchLineKind::Removed {
                let removed_start = index;
                while index < hunk.lines.len() && hunk.lines[index].kind == PatchLineKind::Removed {
                    index += 1;
                }
                let added_start = index;
                while index < hunk.lines.len() && hunk.lines[index].kind == PatchLineKind::Added {
                    index += 1;
                }
                let removed = &hunk.lines[removed_start..added_start];
                let added = &hunk.lines[added_start..index];
                for offset in 0..removed.len().max(added.len()) {
                    rendered.push(render_split_row(
                        removed.get(offset),
                        added.get(offset),
                        file.language(),
                        left_width,
                        right_width,
                        theme,
                        highlighter,
                    ));
                }
                continue;
            }
            if line.kind == PatchLineKind::Added {
                rendered.push(render_split_row(
                    None,
                    Some(line),
                    file.language(),
                    left_width,
                    right_width,
                    theme,
                    highlighter,
                ));
            } else if line.kind == PatchLineKind::Context {
                rendered.push(render_split_row(
                    Some(line),
                    Some(line),
                    file.language(),
                    left_width,
                    right_width,
                    theme,
                    highlighter,
                ));
            } else {
                rendered.push(fit_line(
                    Line::styled(line.text.clone(), Style::new().fg(theme.muted)),
                    usize::from(width),
                    Style::new().fg(theme.muted),
                ));
            }
            index += 1;
        }
    }
    rendered
}

fn render_split_row(
    old: Option<&PatchLine>,
    new: Option<&PatchLine>,
    language: &str,
    left_width: u16,
    right_width: u16,
    theme: &Theme,
    highlighter: &mut SyntaxHighlighter,
) -> Line<'static> {
    let mut spans = render_split_side(old, true, language, left_width, theme, highlighter).spans;
    spans.push(Span::styled("│", Style::new().fg(theme.muted).bg(theme.background)));
    spans.extend(render_split_side(new, false, language, right_width, theme, highlighter).spans);
    Line::from(spans)
}

fn render_split_side(
    line: Option<&PatchLine>,
    old_side: bool,
    language: &str,
    width: u16,
    theme: &Theme,
    highlighter: &mut SyntaxHighlighter,
) -> Line<'static> {
    let (foreground, background) = match line.map(|line| line.kind) {
        Some(PatchLineKind::Removed) => (theme.diff_removed_fg, theme.diff_removed_bg),
        Some(PatchLineKind::Added) => (theme.diff_added_fg, theme.diff_added_bg),
        _ => (theme.text_secondary, theme.background),
    };
    let style = Style::new().fg(foreground).bg(background);
    let mut spans = if let Some(line) = line {
        let number = if old_side { line.old_line_no } else { line.new_line_no };
        vec![Span::styled(number.map_or_else(|| "     ".to_string(), |number| format!("{number:>4} ")), style)]
    } else {
        vec![Span::styled("     ", style)]
    };
    if let Some(line) = line {
        spans.extend(highlighted_spans(&line.text, language, background, theme, highlighter));
    }
    fit_line(Line::from(spans).style(Style::new().bg(background)), usize::from(width), style)
}

fn highlighted_spans(
    source: &str,
    language: &str,
    background: ratatui::style::Color,
    theme: &Theme,
    highlighter: &mut SyntaxHighlighter,
) -> Vec<Span<'static>> {
    highlighter
        .highlight(source, language, theme)
        .into_iter()
        .next()
        .unwrap_or_else(|| Line::raw(source.to_string()))
        .spans
        .into_iter()
        .map(|mut span| {
            span.style = span.style.patch(Style::new().bg(background));
            span
        })
        .collect()
}

fn add_cursor_indicator(line: Line<'static>, theme: &Theme) -> Line<'static> {
    Line::from(line.spans).style(Style::new().bg(theme.accent).fg(theme.background))
}

fn wrap_text(text: &str, max_width: usize) -> Vec<String> {
    if max_width == 0 || text.is_empty() {
        return vec![String::new()];
    }
    let mut lines = Vec::new();
    let mut current = String::new();
    let mut current_width = 0usize;
    for ch in text.chars() {
        let ch_width = unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0);
        if current_width + ch_width > max_width && !current.is_empty() {
            lines.push(current);
            current = String::new();
            current_width = 0;
        }
        current.push(ch);
        current_width += ch_width;
    }
    if !current.is_empty() {
        lines.push(current);
    }
    if lines.is_empty() {
        lines.push(String::new());
    }
    lines
}

fn render_submitted_comment(body: &str, width: u16, theme: &Theme) -> Vec<Line<'static>> {
    let border_style = Style::new().fg(theme.info);
    let body_style = Style::new().fg(theme.text_primary);
    let inner_width = usize::from(width).saturating_sub(4).max(10);
    let body_width = inner_width.saturating_sub(3);

    let body_lines = wrap_text(body, body_width);

    let mut lines = Vec::new();
    let top = fit_line(Line::from(vec![Span::styled("┌─ Comment ─", border_style)]), usize::from(width), border_style);
    lines.push(top);
    for body_line in &body_lines {
        let content = format!("│ > {body_line}");
        let line = fit_line(
            Line::styled(content, body_style),
            usize::from(width),
            body_style.patch(Style::new().bg(theme.background)),
        );
        lines.push(line);
    }
    let bottom = fit_line(Line::from(vec![Span::styled("└", border_style)]), usize::from(width), border_style);
    lines.push(bottom);
    lines
}

fn render_draft_comment(draft: &DraftState, width: u16, theme: &Theme) -> (Vec<Line<'static>>, Option<(usize, u16)>) {
    let border_style = Style::new().fg(theme.accent);
    let body_style = Style::new().fg(theme.text_primary);
    let inner_width = usize::from(width).saturating_sub(4).max(10);
    let body_width = inner_width.saturating_sub(3);

    let body_lines: Vec<String>;
    let cursor: Option<(usize, u16)>;
    if draft.text.is_empty() {
        body_lines = vec!["█".to_string()];
        cursor = Some((1, 3));
    } else {
        let display = draft.text.as_str();
        body_lines = wrap_text(display, body_width);
        let text_before = &draft.text[..draft.byte_cursor];
        let (cursor_line, cursor_col) = text_position_in_wrap(text_before, body_width);
        cursor = Some((1 + cursor_line, 3u16 + cursor_col));
    }

    let mut lines = Vec::new();
    let top = fit_line(Line::from(vec![Span::styled("┌ Draft ─", border_style)]), usize::from(width), border_style);
    lines.push(top);
    for body_line in &body_lines {
        let content = format!("│ > {body_line}");
        let line = fit_line(
            Line::styled(content, body_style),
            usize::from(width),
            body_style.patch(Style::new().bg(theme.background)),
        );
        lines.push(line);
    }
    let bottom = fit_line(Line::from(vec![Span::styled("└", border_style)]), usize::from(width), border_style);
    lines.push(bottom);
    (lines, cursor)
}

fn text_position_in_wrap(prefix: &str, max_width: usize) -> (usize, u16) {
    let mut line = 0usize;
    let mut current_width = 0usize;
    for ch in prefix.chars() {
        let ch_width = unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0);
        if current_width + ch_width > max_width && current_width > 0 {
            line += 1;
            current_width = 0;
        }
        current_width += ch_width;
    }
    (line, u16::try_from(current_width).unwrap_or(u16::MAX))
}

fn fit_line(mut line: Line<'static>, width: usize, fill_style: Style) -> Line<'static> {
    if width == 0 {
        return Line::default();
    }
    if line.width() > width {
        let content_width = width.saturating_sub(1).max(1);
        line = wrap_line(line, u16::try_from(content_width).unwrap_or(u16::MAX)).into_iter().next().unwrap_or_default();
        line.spans.push(Span::styled("…", fill_style));
    }
    if line.width() < width {
        line.spans.push(Span::styled(" ".repeat(width - line.width()), fill_style));
    }
    line
}

fn file_status_color(status: FileStatus, theme: &Theme) -> ratatui::style::Color {
    match status {
        FileStatus::Modified => theme.warning,
        FileStatus::Added | FileStatus::Untracked => theme.diff_added_fg,
        FileStatus::Deleted => theme.diff_removed_fg,
        FileStatus::Renamed => theme.info,
    }
}
