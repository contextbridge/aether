use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::Position;

use super::GitDiffScreen;
use crate::annotation::{apply_draft_key, paste_into_draft};
use crate::edit_buffer::apply_edit_key;
use crate::git_diff::{CommentContext, PatchAnchor, QueuedComment};
use crate::screens::git_diff::GitDiffEvent;
use crate::surface::Action;
use crate::surface::MouseAction;
use crate::surface::is_composed_char;

use super::rendering::plural;
use super::state::{BottomBar, Focus, GitDiffLoadState, PatchCursor, PendingAction};
use super::task;
use super::tasks::GitDiffTask;
use crate::selection::Direction;

/// Lines a mouse wheel notch and a page key move the patch pane.
const MOUSE_SCROLL_LINES: usize = 3;
const PAGE_SCROLL_LINES: usize = 10;

/// Columns each press of the resize keys moves the file drawer's edge.
const DRAWER_RESIZE_STEP: i16 = 4;

impl GitDiffScreen {
    pub(super) fn handle_mouse(&mut self, action: MouseAction, row: u16, column: u16) {
        if self.request.in_flight {
            return;
        }
        match action {
            MouseAction::ScrollUp => self.scroll_focused(Direction::Backward),
            MouseAction::ScrollDown => self.scroll_focused(Direction::Forward),
            MouseAction::Click => self.click(row, column),
        }
    }

    /// Scrolls whichever pane has focus: the patch by a few lines, the file
    /// drawer by one entry.
    fn scroll_focused(&mut self, direction: Direction) {
        if self.focus == Focus::Patch {
            self.move_patch_scroll(direction, MOUSE_SCROLL_LINES);
        } else {
            self.move_vertical(direction);
        }
    }

    /// A click inside the file drawer focuses and selects a row; anywhere else
    /// in the body focuses the patch.
    fn click(&mut self, row: u16, column: u16) {
        if self.review.draft.is_some() || !matches!(self.state, GitDiffLoadState::Ready(_)) {
            return;
        }
        if !self.drawer_selection.rows_area().contains(Position::new(column, row)) {
            self.focus = Focus::Patch;
            return;
        }

        self.focus = Focus::Drawer;
        if self.drawer_selection.select_at(row, self.drawer_entries().len()) {
            self.follow_drawer_selection();
        }
    }

    pub(super) fn handle_key(&mut self, key: KeyEvent) -> Vec<Action> {
        if is_dismiss_key(key) {
            return self.dismiss();
        }

        // A destructive action that would drop queued comments asks for the same
        // key twice; anything else cancels the confirmation.
        if let Some(action) = self.request.pending {
            self.bottom_bar = BottomBar::Help;
            if !is_confirm_key(key, action) {
                self.request.pending = None;
                return Vec::new();
            }
        }

        match &self.bottom_bar {
            BottomBar::CommitEditor { .. } => return self.on_commit_editor_key(key),
            BottomBar::DiscardConfirmation { .. } => return self.on_discard_confirm_key(key),
            BottomBar::Error(_) => {
                self.bottom_bar = BottomBar::Help;
                return Vec::new();
            }
            BottomBar::Help => {}
        }

        if self.review.draft.is_some() {
            return self.on_draft_key(key);
        }

        if self.request.in_flight {
            return Vec::new();
        }

        self.on_browse_key(key)
    }

    pub(super) fn handle_paste(&mut self, text: &str) {
        match &mut self.bottom_bar {
            BottomBar::CommitEditor { buffer } => buffer.insert_paste(text),
            _ => paste_into_draft(&mut self.review.draft, text),
        }
    }

    pub(super) fn handle_event(&mut self, event: GitDiffEvent) -> Vec<Action> {
        // Ignore results for superseded requests.
        if event.request_id() != self.request.id {
            return Vec::new();
        }
        self.request.in_flight = false;
        match event {
            GitDiffEvent::Loaded { result, .. } => {
                match result {
                    Ok(document) => self.apply_document(document),
                    Err(error) => self.state = GitDiffLoadState::Error(error.to_string()),
                }
                Vec::new()
            }
            GitDiffEvent::ActionFinished { result, .. } => match result {
                Ok(()) => {
                    self.show_full_file = false;
                    self.full_file_content = None;
                    task(self.begin_load())
                }
                Err(error) => {
                    self.bottom_bar = BottomBar::Error(error.to_string());
                    Vec::new()
                }
            },
            GitDiffEvent::FullFileLoaded { result, .. } => {
                match result {
                    Ok(content) => self.full_file_content = Some(content),
                    Err(error) => {
                        self.show_full_file = false;
                        self.full_file_content = None;
                        self.bottom_bar = BottomBar::Error(error.to_string());
                    }
                }
                Vec::new()
            }
        }
    }

    /// Esc and Ctrl-G back out one level: a confirmation, then a bottom-bar
    /// mode, then the screen itself.
    fn dismiss(&mut self) -> Vec<Action> {
        if self.request.pending.take().is_some() {
            self.bottom_bar = BottomBar::Help;
            return Vec::new();
        }
        match std::mem::replace(&mut self.bottom_bar, BottomBar::Help) {
            BottomBar::CommitEditor { .. } | BottomBar::DiscardConfirmation { .. } => Vec::new(),
            BottomBar::Error(_) | BottomBar::Help => vec![Action::Close],
        }
    }

    fn on_browse_key(&mut self, key: KeyEvent) -> Vec<Action> {
        // Ignore composed chords so a stray Ctrl+D or Alt+A can't stage, commit,
        // or discard. The plain keys below still drive the destructive actions.
        if is_composed_char(key) {
            return Vec::new();
        }
        match key.code {
            KeyCode::Char('t') | KeyCode::Tab => self.guarded(PendingAction::ScopeSwitch, |screen| {
                screen.scope = screen.scope.next();
                screen.reset_file_view();
                task(screen.begin_load())
            }),
            KeyCode::Char('r') => self.guarded(PendingAction::Reload, |screen| {
                screen.reset_file_view();
                task(screen.begin_load())
            }),
            KeyCode::Char('a') => self.guarded(PendingAction::Stage, GitDiffScreen::stage_all),
            KeyCode::Char('A') => self.guarded(PendingAction::Stage, GitDiffScreen::unstage_all),
            KeyCode::Char(' ') => self.guarded(PendingAction::Stage, GitDiffScreen::toggle_stage),
            KeyCode::Char('C') => self.guarded(PendingAction::Commit, GitDiffScreen::begin_commit),
            KeyCode::Char('d') => self.guarded(PendingAction::Discard, GitDiffScreen::begin_discard),
            KeyCode::Char('<') => self.resize_drawer(-DRAWER_RESIZE_STEP),
            KeyCode::Char('>') => self.resize_drawer(DRAWER_RESIZE_STEP),
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
                Vec::new()
            }
            KeyCode::Right | KeyCode::Char('l') | KeyCode::Enter => {
                if self.focus == Focus::Drawer && !self.expand_or_open_selected() {
                    self.focus = Focus::Patch;
                }
                Vec::new()
            }
            KeyCode::Up | KeyCode::Char('k') => {
                self.move_vertical(Direction::Backward);
                Vec::new()
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.move_vertical(Direction::Forward);
                Vec::new()
            }
            KeyCode::PageUp => {
                self.move_patch_scroll(Direction::Backward, PAGE_SCROLL_LINES);
                Vec::new()
            }
            KeyCode::PageDown => {
                self.move_patch_scroll(Direction::Forward, PAGE_SCROLL_LINES);
                Vec::new()
            }
            _ => Vec::new(),
        }
    }

    /// Runs `action` unless queued review comments would be lost, in which case
    /// it arms a confirmation instead.
    fn guarded(&mut self, action: PendingAction, run: impl FnOnce(&mut Self) -> Vec<Action>) -> Vec<Action> {
        if self.needs_comment_confirmation(action) {
            return Vec::new();
        }
        run(self)
    }

    fn reset_file_view(&mut self) {
        self.show_full_file = false;
        self.full_file_content = None;
        self.patch.cursor = PatchCursor::default();
    }

    fn on_commit_editor_key(&mut self, key: KeyEvent) -> Vec<Action> {
        if key.code != KeyCode::Enter {
            if let BottomBar::CommitEditor { buffer } = &mut self.bottom_bar {
                apply_edit_key(buffer, key);
            }
            return Vec::new();
        }

        let BottomBar::CommitEditor { mut buffer } = std::mem::replace(&mut self.bottom_bar, BottomBar::Help) else {
            return Vec::new();
        };
        let message = buffer.take().trim().to_string();
        if message.is_empty() {
            self.bottom_bar = BottomBar::Error("Commit message cannot be empty".to_string());
            return Vec::new();
        }
        self.repo_operation(|request_id, repo_root| GitDiffTask::Commit { request_id, repo_root, message })
    }

    fn on_discard_confirm_key(&mut self, key: KeyEvent) -> Vec<Action> {
        if is_composed_char(key) {
            return Vec::new();
        }
        match key.code {
            KeyCode::Char('y' | 'Y') => {
                let BottomBar::DiscardConfirmation { path, status } =
                    std::mem::replace(&mut self.bottom_bar, BottomBar::Help)
                else {
                    return Vec::new();
                };
                self.repo_operation(|request_id, repo_root| GitDiffTask::DiscardFile {
                    request_id,
                    repo_root,
                    path,
                    status,
                })
            }
            KeyCode::Char('n' | 'N') | KeyCode::Esc => {
                self.bottom_bar = BottomBar::Help;
                Vec::new()
            }
            _ => Vec::new(),
        }
    }

    fn on_draft_key(&mut self, key: KeyEvent) -> Vec<Action> {
        if let Some((anchor, body)) = apply_draft_key(&mut self.review.draft, key) {
            self.file_comment(anchor, body);
        }
        Vec::new()
    }

    /// Files a review comment against the patch line `anchor` points at,
    /// dropping it when the anchor no longer resolves.
    fn file_comment(&mut self, anchor: PatchAnchor, body: String) {
        let Some(file) = self.selected_file() else {
            return;
        };
        let Some(patch_line) = file.hunks.get(anchor.hunk).and_then(|hunk| hunk.lines.get(anchor.line)) else {
            return;
        };
        self.review.queue.push(QueuedComment {
            anchor,
            body,
            context: CommentContext {
                file_path: file.path.clone(),
                line_text: patch_line.text.clone(),
                line_number: patch_line.new_line_no.or(patch_line.old_line_no),
                line_kind: patch_line.kind,
            },
        });
        self.bump_comments_revision();
    }

    fn needs_comment_confirmation(&mut self, action: PendingAction) -> bool {
        if self.review.queue.is_empty() || self.request.pending == Some(action) {
            return false;
        }
        let count = self.review.queue.len();
        self.request.pending = Some(action);
        self.bottom_bar = BottomBar::Error(format!(
            "{} will clear {}. Press {} again to confirm or Esc to cancel.",
            action.gerund(),
            plural(count, "review comment"),
            action.key_hint(),
        ));
        true
    }
}

impl PendingAction {
    /// What arming this action is about to do, for the confirmation prompt.
    fn gerund(self) -> &'static str {
        match self {
            Self::Reload => "Reloading",
            Self::ScopeSwitch => "Switching scope",
            Self::Stage => "Staging/unstaging",
            Self::Commit => "Committing",
            Self::Discard => "Discarding",
        }
    }

    /// The keys that confirm this action. The first is the one the prompt names.
    fn confirm_keys(self) -> &'static [KeyCode] {
        match self {
            Self::Reload => &[KeyCode::Char('r')],
            Self::ScopeSwitch => &[KeyCode::Char('t'), KeyCode::Tab],
            Self::Stage => &[KeyCode::Char(' '), KeyCode::Char('a'), KeyCode::Char('A')],
            Self::Commit => &[KeyCode::Char('C')],
            Self::Discard => &[KeyCode::Char('d')],
        }
    }

    fn key_hint(self) -> String {
        self.confirm_keys().iter().copied().map(key_label).collect::<Vec<_>>().join("/")
    }
}

fn key_label(code: KeyCode) -> String {
    match code {
        KeyCode::Char(' ') => "space".to_string(),
        KeyCode::Char(character) => character.to_string(),
        other => format!("{other:?}").to_lowercase(),
    }
}

fn is_dismiss_key(key: KeyEvent) -> bool {
    key.code == KeyCode::Esc || key.code == KeyCode::Char('g') && key.modifiers.contains(KeyModifiers::CONTROL)
}

fn is_confirm_key(key: KeyEvent, action: PendingAction) -> bool {
    !is_composed_char(key) && action.confirm_keys().contains(&key.code)
}
