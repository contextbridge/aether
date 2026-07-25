use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use super::GitDiffScreen;
use crate::edit_buffer::apply_edit_key;
use crate::git_diff::{CommentContext, QueuedComment};
use crate::screens::{MouseAction, ScreenEffect, ScreenEvent, ScreenOutcome};

use super::effects::{GitDiffEffect, next_request_id};
use super::rendering::{DRAWER_MIN_WIDTH, drawer_width};
use super::state::{BottomBar, DrawerEntry, Focus, GitDiffLoadState, PatchCursor, PendingAction};

impl GitDiffScreen {
    pub(super) fn handle_mouse(&mut self, action: MouseAction, row: u16, column: u16) {
        if self.operation_in_flight {
            return;
        }
        match action {
            MouseAction::ScrollUp => self.scroll_focused(-1),
            MouseAction::ScrollDown => self.scroll_focused(1),
            MouseAction::Click => self.click(row, column),
        }
    }

    /// Scrolls whichever pane has focus: the patch by a few lines, the file
    /// drawer by one entry.
    fn scroll_focused(&mut self, direction: isize) {
        if self.focus == Focus::Patch {
            self.move_patch_scroll(direction * 3);
        } else {
            self.move_vertical(direction);
        }
    }

    fn click(&mut self, row: u16, column: u16) {
        if self.draft.is_some() || !matches!(self.state, GitDiffLoadState::Ready(_)) {
            return;
        }
        let Some(body_row) = row.checked_sub(1) else {
            return;
        };
        let area = self.last_area;
        let drawer = area.width >= DRAWER_MIN_WIDTH;
        let body_x = area.x.saturating_add(1);
        if !drawer || column < body_x || column >= body_x + drawer_width(area.width) {
            self.focus = Focus::Patch;
            return;
        }

        self.focus = Focus::Drawer;
        let entries = self.drawer_entries();
        if entries.is_empty() {
            return;
        }
        self.drawer_selection.select_row(usize::from(body_row), entries.len());
        if let Some(DrawerEntry::File { index, .. }) =
            self.drawer_selection.selected().and_then(|selected| entries.get(selected))
        {
            self.selected_file = *index;
        }
    }

    pub(super) fn handle_key(&mut self, key: KeyEvent) -> ScreenOutcome {
        if is_dismiss_key(key) {
            return self.dismiss();
        }

        // A destructive action that would drop queued comments asks for the same
        // key twice; anything else cancels the confirmation.
        if let Some(action) = self.pending_action {
            self.bottom_bar = BottomBar::Help;
            if !is_confirm_key(key, action) {
                self.pending_action = None;
                return ScreenOutcome::None;
            }
        }

        match &self.bottom_bar {
            BottomBar::CommitEditor { .. } => return self.on_commit_editor_key(key),
            BottomBar::DiscardConfirmation { .. } => return self.on_discard_confirm_key(key),
            BottomBar::Error(_) => {
                self.bottom_bar = BottomBar::Help;
                return ScreenOutcome::None;
            }
            BottomBar::Help => {}
        }

        if self.draft.is_some() {
            return self.on_draft_key(key);
        }

        if self.operation_in_flight {
            return ScreenOutcome::None;
        }

        self.on_browse_key(key)
    }

    pub(super) fn handle_event(&mut self, event: ScreenEvent) -> Option<ScreenEffect> {
        // Ignore results for superseded requests.
        if event.request_id() != self.request_id {
            return None;
        }
        self.operation_in_flight = false;
        match event {
            ScreenEvent::Loaded { result, .. } => {
                match result {
                    Ok(document) => self.apply_document(document),
                    Err(error) => self.state = GitDiffLoadState::Error(error.to_string()),
                }
                None
            }
            ScreenEvent::ActionFinished { result, .. } => match result {
                Ok(()) => {
                    self.show_full_file = false;
                    self.full_file_content = None;
                    Some(self.begin_load())
                }
                Err(error) => {
                    self.bottom_bar = BottomBar::Error(error.to_string());
                    None
                }
            },
            ScreenEvent::FullFileLoaded { result, .. } => {
                match result {
                    Ok(content) => self.full_file_content = Some(content),
                    Err(error) => {
                        self.show_full_file = false;
                        self.full_file_content = None;
                        self.bottom_bar = BottomBar::Error(error.to_string());
                    }
                }
                None
            }
            ScreenEvent::SubmitReview { .. } => None,
        }
    }

    /// Esc and Ctrl-G back out one level: a confirmation, then a bottom-bar
    /// mode, then the screen itself.
    fn dismiss(&mut self) -> ScreenOutcome {
        if self.pending_action.take().is_some() {
            self.bottom_bar = BottomBar::Help;
            return ScreenOutcome::None;
        }
        match std::mem::replace(&mut self.bottom_bar, BottomBar::Help) {
            BottomBar::CommitEditor { .. } | BottomBar::DiscardConfirmation { .. } => ScreenOutcome::None,
            BottomBar::Error(_) | BottomBar::Help => ScreenOutcome::Close,
        }
    }

    fn on_browse_key(&mut self, key: KeyEvent) -> ScreenOutcome {
        match key.code {
            KeyCode::Char('t') | KeyCode::Tab => self.guarded(PendingAction::ScopeSwitch, |screen| {
                screen.scope = screen.scope.next();
                screen.reset_file_view();
                ScreenOutcome::Effect(screen.begin_load())
            }),
            KeyCode::Char('r') => self.guarded(PendingAction::Reload, |screen| {
                screen.reset_file_view();
                ScreenOutcome::Effect(screen.begin_load())
            }),
            KeyCode::Char('a') => self.guarded(PendingAction::Stage, GitDiffScreen::stage_all),
            KeyCode::Char('A') => self.guarded(PendingAction::Stage, GitDiffScreen::unstage_all),
            KeyCode::Char(' ') => self.guarded(PendingAction::Stage, GitDiffScreen::toggle_stage),
            KeyCode::Char('C') => self.guarded(PendingAction::Commit, GitDiffScreen::begin_commit),
            KeyCode::Char('d') => self.guarded(PendingAction::Discard, GitDiffScreen::begin_discard),
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
                ScreenOutcome::None
            }
            KeyCode::Right | KeyCode::Char('l') | KeyCode::Enter => {
                if self.focus == Focus::Drawer && !self.expand_or_open_selected() {
                    self.focus = Focus::Patch;
                }
                ScreenOutcome::None
            }
            KeyCode::Up | KeyCode::Char('k') => {
                self.move_vertical(-1);
                ScreenOutcome::None
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.move_vertical(1);
                ScreenOutcome::None
            }
            KeyCode::PageUp => {
                self.move_patch_scroll(-10);
                ScreenOutcome::None
            }
            KeyCode::PageDown => {
                self.move_patch_scroll(10);
                ScreenOutcome::None
            }
            _ => ScreenOutcome::None,
        }
    }

    /// Runs `action` unless queued review comments would be lost, in which case
    /// it arms a confirmation instead.
    fn guarded(&mut self, action: PendingAction, run: impl FnOnce(&mut Self) -> ScreenOutcome) -> ScreenOutcome {
        if self.needs_comment_confirmation(action) {
            return ScreenOutcome::None;
        }
        run(self)
    }

    fn reset_file_view(&mut self) {
        self.show_full_file = false;
        self.full_file_content = None;
        self.patch_cursor = PatchCursor::default();
    }

    fn on_commit_editor_key(&mut self, key: KeyEvent) -> ScreenOutcome {
        if key.code != KeyCode::Enter {
            if let BottomBar::CommitEditor { buffer } = &mut self.bottom_bar {
                apply_edit_key(buffer, key);
            }
            return ScreenOutcome::None;
        }

        let BottomBar::CommitEditor { mut buffer } = std::mem::replace(&mut self.bottom_bar, BottomBar::Help) else {
            return ScreenOutcome::None;
        };
        let message = buffer.take().trim().to_string();
        if message.is_empty() {
            self.bottom_bar = BottomBar::Error("Commit message cannot be empty".to_string());
            return ScreenOutcome::None;
        }
        let Some(repo_root) = self.repo_root.clone() else {
            return ScreenOutcome::None;
        };
        self.request_id = next_request_id();
        self.operation_in_flight = true;
        ScreenOutcome::Effect(GitDiffEffect::Commit { request_id: self.request_id, repo_root, message })
    }

    fn on_discard_confirm_key(&mut self, key: KeyEvent) -> ScreenOutcome {
        match key.code {
            KeyCode::Char('y' | 'Y') => {
                let BottomBar::DiscardConfirmation { path, status } =
                    std::mem::replace(&mut self.bottom_bar, BottomBar::Help)
                else {
                    return ScreenOutcome::None;
                };
                let Some(repo_root) = self.repo_root.clone() else {
                    return ScreenOutcome::None;
                };
                self.request_id = next_request_id();
                self.operation_in_flight = true;
                ScreenOutcome::Effect(GitDiffEffect::DiscardFile {
                    request_id: self.request_id,
                    repo_root,
                    path,
                    status,
                })
            }
            KeyCode::Char('n' | 'N') | KeyCode::Esc => {
                self.bottom_bar = BottomBar::Help;
                ScreenOutcome::None
            }
            _ => ScreenOutcome::None,
        }
    }

    fn on_draft_key(&mut self, key: KeyEvent) -> ScreenOutcome {
        match key.code {
            KeyCode::Esc => self.draft = None,
            KeyCode::Enter => self.commit_draft(),
            _ => {
                if let Some(draft) = self.draft.as_mut() {
                    apply_edit_key(&mut draft.buffer, key);
                }
            }
        }
        ScreenOutcome::None
    }

    /// Files the draft as a review comment against the patch line it is anchored
    /// to, discarding it when the anchor no longer resolves.
    fn commit_draft(&mut self) {
        let Some(mut draft) = self.draft.take() else {
            return;
        };
        let body = draft.buffer.take();
        if body.trim().is_empty() {
            return;
        }
        let Some(file) = self.selected_file() else {
            return;
        };
        let Some(patch_line) = file.hunks.get(draft.anchor.hunk).and_then(|hunk| hunk.lines.get(draft.anchor.line))
        else {
            return;
        };
        self.review_queue.push(QueuedComment {
            anchor: draft.anchor,
            body,
            context: CommentContext {
                file_path: file.path.clone(),
                line_text: patch_line.text.clone(),
                line_number: patch_line.new_line_no.or(patch_line.old_line_no),
                line_kind: patch_line.kind,
            },
        });
        self.comments_revision = self.comments_revision.wrapping_add(1);
    }

    fn needs_comment_confirmation(&mut self, action: PendingAction) -> bool {
        if self.review_queue.is_empty() || self.pending_action == Some(action) {
            return false;
        }
        let count = self.review_queue.len();
        let (what, key) = match action {
            PendingAction::Reload => ("Reloading", "r"),
            PendingAction::ScopeSwitch => ("Switching scope", "t"),
            PendingAction::Stage => ("Staging/unstaging", "space/a/A"),
            PendingAction::Commit => ("Committing", "C"),
            PendingAction::Discard => ("Discarding", "d"),
        };
        self.pending_action = Some(action);
        self.bottom_bar = BottomBar::Error(format!(
            "{what} will clear {count} review comment{}. Press {key} again to confirm or Esc to cancel.",
            if count == 1 { "" } else { "s" }
        ));
        true
    }
}

fn is_dismiss_key(key: KeyEvent) -> bool {
    key.code == KeyCode::Esc || key.code == KeyCode::Char('g') && key.modifiers.contains(KeyModifiers::CONTROL)
}

fn is_confirm_key(key: KeyEvent, action: PendingAction) -> bool {
    matches!(
        (action, key.code),
        (PendingAction::Reload, KeyCode::Char('r'))
            | (PendingAction::ScopeSwitch, KeyCode::Char('t') | KeyCode::Tab)
            | (PendingAction::Stage, KeyCode::Char('a' | 'A' | ' '))
            | (PendingAction::Commit, KeyCode::Char('C'))
            | (PendingAction::Discard, KeyCode::Char('d'))
    )
}
