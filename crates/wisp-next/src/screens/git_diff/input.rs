use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use super::GitDiffScreen;
use crate::git_diff::{CommentContext, QueuedComment};

use super::effects::{GitDiffEffect, GitDiffEvent, GitDiffOutcome, next_request_id};
use super::state::{BottomBar, DrawerEntry, Focus, GitDiffLoadState, PatchCursor, PendingAction};

impl GitDiffScreen {
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
                    self.drawer_selection.select_row(usize::from(body_y), entries.len());
                    if let Some(DrawerEntry::File { index, .. }) =
                        self.drawer_selection.selected().and_then(|selected| entries.get(selected))
                    {
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
    fn on_commit_editor_key(&mut self, key: KeyEvent) -> GitDiffOutcome {
        if key.code == KeyCode::Esc || key.code == KeyCode::Char('g') && key.modifiers.contains(KeyModifiers::CONTROL) {
            self.bottom_bar = BottomBar::Help;
            return GitDiffOutcome::None;
        }
        match key.code {
            KeyCode::Enter => {
                let BottomBar::CommitEditor { mut buffer } = std::mem::replace(&mut self.bottom_bar, BottomBar::Help)
                else {
                    return GitDiffOutcome::None;
                };
                let trimmed = buffer.take().trim().to_string();
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
                if let BottomBar::CommitEditor { buffer } = &mut self.bottom_bar {
                    buffer.insert_char(c);
                }
                GitDiffOutcome::None
            }
            KeyCode::Backspace => {
                if let BottomBar::CommitEditor { buffer } = &mut self.bottom_bar {
                    buffer.backspace();
                }
                GitDiffOutcome::None
            }
            KeyCode::Delete => {
                if let BottomBar::CommitEditor { buffer } = &mut self.bottom_bar {
                    buffer.delete();
                }
                GitDiffOutcome::None
            }
            KeyCode::Left => {
                if let BottomBar::CommitEditor { buffer } = &mut self.bottom_bar {
                    buffer.move_left();
                }
                GitDiffOutcome::None
            }
            KeyCode::Right => {
                if let BottomBar::CommitEditor { buffer } = &mut self.bottom_bar {
                    buffer.move_right();
                }
                GitDiffOutcome::None
            }
            KeyCode::Home => {
                if let BottomBar::CommitEditor { buffer } = &mut self.bottom_bar {
                    buffer.set_cursor(0);
                }
                GitDiffOutcome::None
            }
            KeyCode::End => {
                if let BottomBar::CommitEditor { buffer } = &mut self.bottom_bar {
                    buffer.move_to_end();
                }
                GitDiffOutcome::None
            }
            _ => GitDiffOutcome::None,
        }
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
                let mut draft = self.draft.take().expect("draft exists");
                let text = draft.buffer.take();
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
                self.comments_revision = self.comments_revision.wrapping_add(1);
                GitDiffOutcome::None
            }
            KeyCode::Char(c) => {
                draft.buffer.insert_char(c);
                GitDiffOutcome::None
            }
            KeyCode::Backspace => {
                draft.buffer.backspace();
                GitDiffOutcome::None
            }
            KeyCode::Delete => {
                draft.buffer.delete();
                GitDiffOutcome::None
            }
            KeyCode::Left => {
                draft.buffer.move_left();
                GitDiffOutcome::None
            }
            KeyCode::Right => {
                draft.buffer.move_right();
                GitDiffOutcome::None
            }
            KeyCode::Home => {
                draft.buffer.set_cursor(0);
                GitDiffOutcome::None
            }
            KeyCode::End => {
                draft.buffer.move_to_end();
                GitDiffOutcome::None
            }
            _ => GitDiffOutcome::None,
        }
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
}
