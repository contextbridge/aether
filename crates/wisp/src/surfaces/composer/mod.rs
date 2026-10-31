mod queued_prompts_view;
mod view;
pub(crate) use queued_prompts_view::QueuedPromptsView;
pub(crate) use view::ComposerBodyView;

use crate::surfaces::picker::{CompletionOverlay, SlashCommand};
use crate::surfaces::prompt_search::PromptSearchPicker;
use crate::view::edit_buffer::{EditBuffer, apply_edit_key, is_composed_char};
use crate::view::input::{Nav, UiEvent};
use client_core::composer::attachment::{AttachedFile, PromptAttachment, attach_missing};
use client_core::composer::completion_token::{CompletionToken, Trigger};
use client_core::composer::file_listing::FileListing;
use client_core::composer::prompt_history::PromptHistory;
use client_core::composer::prompt_search::PromptSearchState;
use client_core::composer::submission::Submission;
use crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::Position;
use ratatui::text::Line;
use std::collections::HashSet;
use unicode_width::UnicodeWidthStr;

#[derive(Debug, Default)]
pub struct Composer {
    buffer: EditBuffer,
    overlay: Option<Overlay>,
    mentions: Vec<PromptAttachment>,
    pending_media: Vec<PromptAttachment>,
    history: PromptHistory,
    /// Columns the last layout wrapped the input at. Unset until the composer
    /// has been drawn once, which reads as "wide enough that nothing wraps".
    content_width: Option<usize>,
}

/// The inline picker open around the composer's text.
///
/// At most one, because they compete for the same keys: making that an enum
/// rather than two `Option` fields means the pair can never both be open.
#[derive(Debug)]
enum Overlay {
    /// The `/` command or `@` file list, drawn below the text.
    Completion(CompletionOverlay),
    /// Prompt-history search, drawn above the text. It carries the draft it
    /// replaced, so backing out restores what the user was writing.
    PromptSearch { picker: PromptSearchPicker, draft: String },
}

pub struct ComposerLayout {
    pub lines: Vec<Line<'static>>,
    pub cursor: Position,
}

/// What a keystroke or paste the composer's own overlay consumed asks the app
/// to do next.
///
/// The composer owns its overlays and every edit they imply; the app is told
/// only the two things it has to act on outside the composer, so the overlay
/// state itself never has to leave.
#[derive(Debug)]
pub enum ComposerOutcome {
    /// Fully handled inside the composer.
    Handled,
    /// A command was accepted from the `/` list and needs running.
    AcceptedCommand(SlashCommand),
    /// The history-search query changed and needs re-running against the agent.
    Search(String),
}

impl Composer {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn text(&self) -> &str {
        self.buffer.text()
    }

    pub fn is_empty(&self) -> bool {
        self.buffer.is_empty() && self.pending_media.is_empty()
    }

    pub fn selected_mentions(&self) -> Vec<PromptAttachment> {
        // Match whole whitespace-delimited tokens so that `@foo` does not also match the
        // longer mention `@foobar`. The insertion path always writes `@<display> `, so a
        // complete token comparison is the correct granularity.
        let tokens: HashSet<&str> = self.buffer.text().split_whitespace().collect();
        self.mentions
            .iter()
            .filter(|mention| {
                let needle = format!("@{}", mention.display_name);
                tokens.contains(needle.as_str())
            })
            .cloned()
            .collect()
    }

    pub fn take_submission(&mut self) -> Submission {
        let mentions = self.selected_mentions();
        let text = self.buffer.take();
        let media = std::mem::take(&mut self.pending_media);
        self.history.push(&text);
        self.overlay = None;
        self.mentions.clear();
        Submission { text, mentions, media }
    }

    pub fn restore_submission(&mut self, submission: Submission) {
        let restored = submission.restore_onto(self.buffer.text());
        self.buffer.set_text(restored);
        self.buffer.set_cursor(self.buffer.text().len());
        attach_missing(&mut self.mentions, submission.mentions);
        attach_missing(&mut self.pending_media, submission.media);
        self.history.reset();
    }

    pub fn clear(&mut self) {
        self.buffer.set_text("");
        self.pending_media.clear();
        self.overlay = None;
        self.mentions.clear();
        self.history.reset();
    }

    pub fn attach_files(&mut self, files: Vec<AttachedFile>) -> bool {
        if files.iter().any(|file| matches!(file, AttachedFile::Unsupported(_))) {
            return false;
        }
        for file in files {
            match file {
                AttachedFile::Media(media) => attach_missing(&mut self.pending_media, [media]),
                AttachedFile::Mention(mention) => {
                    self.insert_str(&format!("@{} ", mention.display_name));
                    self.mentions.push(mention);
                }
                AttachedFile::Unsupported(_) => {}
            }
        }
        true
    }

    pub fn pending_media(&self) -> &[PromptAttachment] {
        &self.pending_media
    }

    /// Applies one shared editing keystroke. Anything that changes the text also
    /// ends history navigation, so the recalled prompt becomes the user's draft.
    pub fn apply_edit_key(&mut self, key: KeyEvent) -> bool {
        if key.code == KeyCode::Backspace {
            self.backspace();
            return true;
        }
        let before = self.buffer.text().len();
        let handled = apply_edit_key(&mut self.buffer, key);
        if self.buffer.text().len() != before {
            self.history.reset();
        }
        handled
    }

    pub fn insert_char(&mut self, character: char) {
        self.history.reset();
        self.buffer.insert_char(character);
    }

    pub fn insert_str(&mut self, text: &str) {
        self.history.reset();
        self.buffer.insert_str(text);
    }

    pub fn insert_paste(&mut self, text: &str) {
        self.history.reset();
        self.buffer.insert_paste(text);
    }

    pub fn insert_newline(&mut self) {
        self.insert_char('\n');
        self.overlay = None;
    }

    pub fn backspace(&mut self) {
        self.history.reset();
        if self.buffer.is_empty() && !self.pending_media.is_empty() {
            self.pending_media.pop();
            return;
        }
        self.buffer.backspace();
    }

    pub fn move_left(&mut self) {
        self.buffer.move_left();
    }

    pub fn move_line_start(&mut self) {
        self.buffer.move_line_start();
    }

    pub fn move_line_end(&mut self) {
        self.buffer.move_line_end();
    }

    /// Moves the cursor to the visual row above, reporting whether there was
    /// one. A long line the composer soft-wrapped has rows above without any
    /// newline before the cursor, so this follows what is on screen rather than
    /// the newlines in the text.
    pub fn move_up(&mut self) -> bool {
        self.move_visual_row(|row| row.checked_sub(1))
    }

    /// Moves the cursor to the visual row below, reporting whether there was one.
    pub fn move_down(&mut self) -> bool {
        self.move_visual_row(|row| Some(row + 1))
    }

    pub fn recall_previous(&mut self) -> bool {
        let Some(prompt) = self.history.older(self.buffer.text()).map(str::to_string) else {
            return false;
        };
        self.set_text(prompt);
        self.buffer.set_cursor(0);
        true
    }

    pub fn recall_next(&mut self) -> bool {
        let Some(prompt) = self.history.newer() else {
            return false;
        };
        self.set_text(prompt);
        self.buffer.set_cursor(self.buffer.text().len());
        true
    }

    pub fn open_command_picker(&mut self, commands: Vec<SlashCommand>) {
        self.overlay = Some(Overlay::Completion(CompletionOverlay::command(commands)));
    }

    pub fn open_file_picker(&mut self) {
        self.overlay = Some(Overlay::Completion(CompletionOverlay::file()));
    }

    pub fn on_files_listed(&mut self, listing: FileListing) {
        if let Some(overlay) = self.completion_mut() {
            overlay.set_files(listing);
        }
    }

    pub fn has_completion(&self) -> bool {
        matches!(self.overlay, Some(Overlay::Completion(_)))
    }

    pub fn completion(&self) -> Option<&CompletionOverlay> {
        match self.overlay.as_ref()? {
            Overlay::Completion(overlay) => Some(overlay),
            Overlay::PromptSearch { .. } => None,
        }
    }

    /// The open completion list, for navigation and stateful rendering.
    pub fn completion_mut(&mut self) -> Option<&mut CompletionOverlay> {
        match self.overlay.as_mut()? {
            Overlay::Completion(overlay) => Some(overlay),
            Overlay::PromptSearch { .. } => None,
        }
    }

    pub fn prompt_search(&self) -> Option<&PromptSearchPicker> {
        match self.overlay.as_ref()? {
            Overlay::PromptSearch { picker, .. } => Some(picker),
            Overlay::Completion(_) => None,
        }
    }

    /// The open prompt-history picker, for queries and stateful rendering.
    pub fn prompt_search_mut(&mut self) -> Option<&mut PromptSearchPicker> {
        match self.overlay.as_mut()? {
            Overlay::PromptSearch { picker, .. } => Some(picker),
            Overlay::Completion(_) => None,
        }
    }

    pub fn has_open_overlay(&self) -> bool {
        self.overlay.is_some()
    }

    /// Routes a mouse event to whichever overlay is open. Browsing history
    /// results previews each candidate in the composer, the way the arrow keys
    /// do.
    pub fn on_overlay_mouse(&mut self, event: &UiEvent, search: &PromptSearchState) {
        match self.overlay.as_mut() {
            Some(Overlay::Completion(overlay)) => {
                overlay.entries_mut().navigate(event);
            }
            Some(Overlay::PromptSearch { picker, .. }) => {
                if matches!(picker.navigate(event, search), Nav::Moved | Nav::Clicked) {
                    self.apply_selected_search_result(search);
                }
            }
            None => {}
        }
    }

    pub fn has_prompt_search(&self) -> bool {
        matches!(self.overlay, Some(Overlay::PromptSearch { .. }))
    }

    pub fn open_prompt_search(&mut self) {
        let draft = self.buffer.text().to_string();
        self.overlay = Some(Overlay::PromptSearch { picker: PromptSearchPicker::default(), draft });
    }

    /// Closes the search, restoring the draft it replaced unless the user
    /// confirmed one of the results.
    fn close_prompt_search(&mut self, confirmed: bool) {
        let Some(Overlay::PromptSearch { draft, .. }) = self.overlay.take() else {
            return;
        };
        if !confirmed {
            self.buffer.set_text(draft);
        }
    }

    /// Applies a keystroke to the open history search, or reports that there is
    /// none for it to go to.
    pub fn on_prompt_search_key(&mut self, key: KeyEvent, search: &PromptSearchState) -> Option<ComposerOutcome> {
        let picker = self.prompt_search_mut()?;
        let mut query = search.query().to_owned();
        match picker.navigate(&UiEvent::Key(key), search) {
            Nav::Close => {
                self.close_prompt_search(false);
                return Some(ComposerOutcome::Handled);
            }
            Nav::Activate => {
                let confirmed = picker.selected_result(search).is_some();
                self.close_prompt_search(confirmed);
                return Some(ComposerOutcome::Handled);
            }
            Nav::Moved | Nav::Clicked => {
                self.apply_selected_search_result(search);
                return Some(ComposerOutcome::Handled);
            }
            Nav::Unhandled => match key.code {
                KeyCode::Backspace => {
                    query.pop();
                }
                KeyCode::Char(c) if !is_composed_char(key) => query.push(c),
                _ => return Some(ComposerOutcome::Handled),
            },
        }
        Some(self.search_outcome(query))
    }

    /// Applies a paste to the open history search, or reports that there is none
    /// for it to go to.
    pub fn on_prompt_search_paste(&mut self, text: &str, search: &PromptSearchState) -> Option<ComposerOutcome> {
        if !self.has_prompt_search() {
            return None;
        }
        let query = search.query().chars().chain(text.chars().filter(|c| !c.is_control())).collect();
        Some(self.search_outcome(query))
    }

    /// Applies a keystroke to the open completion list, or reports that there is
    /// none for it to go to.
    pub fn on_completion_key(&mut self, key: KeyEvent) -> Option<ComposerOutcome> {
        if !self.has_completion() {
            return None;
        }
        Some(match self.completion_on_key(key) {
            Some(command) => ComposerOutcome::AcceptedCommand(command),
            None => ComposerOutcome::Handled,
        })
    }

    fn search_outcome(&mut self, query: String) -> ComposerOutcome {
        if query.trim().is_empty()
            && let Some(Overlay::PromptSearch { draft, .. }) = &self.overlay
        {
            self.buffer.set_text(draft.clone());
        }
        ComposerOutcome::Search(query)
    }

    pub fn show_prompt_search_results(&mut self, search: &PromptSearchState) {
        if let Some(picker) = self.prompt_search_mut() {
            picker.on_results(search);
            self.apply_selected_search_result(search);
        }
    }

    fn apply_selected_search_result(&mut self, search: &PromptSearchState) {
        let Some(result) = self.prompt_search().and_then(|picker| picker.selected_result(search)) else {
            return;
        };
        let (prompt, match_end) = (result.prompt.clone(), result.match_end);
        self.buffer.set_text(prompt);
        self.buffer.set_cursor(match_end);
    }

    /// Applies a keystroke to the open completion list, returning the command it
    /// accepted.
    ///
    /// The mirror of [`Composer::on_prompt_search_key`]: the composer owns the
    /// list's own keys and the edits they imply, and the app decides what an
    /// accepted command means.
    fn completion_on_key(&mut self, key: KeyEvent) -> Option<SlashCommand> {
        match self.completion_mut()?.entries_mut().navigate(&UiEvent::Key(key)) {
            Nav::Close => self.close_overlay(),
            Nav::Activate => return self.accept_selected(),
            Nav::Moved | Nav::Clicked => {}
            Nav::Unhandled => match key.code {
                KeyCode::Tab => return self.accept_selected(),
                KeyCode::Backspace if self.completion().is_some_and(|overlay| overlay.query().is_empty()) => {
                    self.backspace();
                    self.close_overlay();
                }
                KeyCode::Backspace => {
                    self.backspace();
                    self.refresh_overlay_query();
                }
                KeyCode::Char(character) if !is_composed_char(key) => {
                    self.insert_char(character);
                    if character.is_whitespace() {
                        self.close_overlay();
                    } else {
                        self.refresh_overlay_query();
                    }
                }
                _ => {}
            },
        }
        None
    }

    fn accept_selected(&mut self) -> Option<SlashCommand> {
        let command = self.accept_command();
        if command.is_none() {
            self.accept_file();
        }
        command
    }

    pub(crate) fn close_overlay(&mut self) {
        self.overlay = None;
    }

    pub fn accept_command(&mut self) -> Option<SlashCommand> {
        let command = self.completion()?.selected_command()?;
        self.replace_token(Trigger::Command, &format!("/{}", command.name()));
        self.overlay = None;
        Some(command)
    }

    pub fn accept_file(&mut self) -> Option<PromptAttachment> {
        let file = self.completion()?.selected_file()?;
        self.replace_token(Trigger::File, &format!("@{} ", file.display_name));
        self.mentions.push(file.clone());
        self.overlay = None;
        Some(file)
    }

    pub fn refresh_overlay_query(&mut self) {
        let Some(trigger) = self.completion().map(CompletionOverlay::trigger) else {
            return;
        };
        let Some(token) = self.active_token(trigger) else {
            self.close_overlay();
            return;
        };
        let query = token.query(self.buffer.text()).to_string();
        if let Some(overlay) = self.completion_mut() {
            overlay.set_query(query);
        }
    }

    /// Moves the cursor to the row `target` picks, keeping its column where that
    /// row is long enough to hold it.
    fn move_visual_row(&mut self, target: impl FnOnce(usize) -> Option<usize>) -> bool {
        let content_width = self.content_width.unwrap_or(usize::MAX);
        let layout = view::input_layout(self.buffer.text(), self.buffer.cursor(), content_width);
        let Some(cursor) =
            target(layout.cursor_row).and_then(|row| layout.byte_at(self.buffer.text(), row, layout.cursor_column))
        else {
            return false;
        };
        self.buffer.set_cursor(cursor);
        true
    }

    pub fn cursor_position(&self) -> (usize, usize) {
        let before = &self.buffer.text()[..self.buffer.cursor()];
        let row = before.matches('\n').count();
        let column = before[self.buffer.line_start()..].width();
        (row, column)
    }

    fn active_token(&self, trigger: Trigger) -> Option<CompletionToken> {
        CompletionToken::at(self.buffer.text(), self.buffer.cursor()).filter(|token| token.trigger == trigger)
    }

    fn replace_token(&mut self, trigger: Trigger, replacement: &str) {
        let Some(token) = self.active_token(trigger) else {
            return;
        };
        self.buffer.replace_range(token.range, replacement);
    }

    fn set_text(&mut self, text: String) {
        self.buffer.set_text(text);
        self.mentions.clear();
        self.overlay = None;
    }
}
