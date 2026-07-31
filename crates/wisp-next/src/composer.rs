mod view;

use crate::attachments::{AttachmentKind, PromptAttachment, classify_attachment};
use crate::edit_buffer::EditBuffer;
use crate::generation::Generation;
use crate::picker::{CommandEntry, CompletionOverlay, FileEntry};
use crate::prompt_search::{self, PromptSearchPicker};
use crate::selection::Direction;
use crate::surface::MouseAction;
use crate::tasks::Task;
use acp_utils::notifications::PromptSearchResponse;
use crossterm::event::KeyCode;
use ratatui::layout::Position;
use ratatui::text::Line;
use std::collections::HashSet;
use unicode_width::UnicodeWidthStr;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SelectedFileMention {
    pub path: std::path::PathBuf,
    pub display_name: String,
}

#[derive(Debug, Default)]
pub struct Composer {
    buffer: EditBuffer,
    overlay: Option<Overlay>,
    mentions: Vec<SelectedFileMention>,
    pending_media: Vec<PromptAttachment>,
    history: PromptHistory,
    /// Columns the last layout wrapped the input at. Unset until the composer
    /// has been drawn once, which reads as "wide enough that nothing wraps".
    content_width: Option<usize>,
}

/// The inline surface open around the composer's text.
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

/// The prompts submitted this session, and where recall has walked back to.
///
/// Navigation stashes the draft it replaced, so stepping past the newest entry
/// puts the user back where they started.
#[derive(Debug, Default)]
struct PromptHistory {
    entries: Vec<String>,
    index: Option<usize>,
    draft: Option<String>,
}

/// Prompts kept for recall. Older ones are dropped rather than growing forever.
const MAX_HISTORY_ENTRIES: usize = 500;

impl PromptHistory {
    fn push(&mut self, prompt: &str) {
        if prompt.trim().is_empty() {
            return;
        }
        self.entries.push(prompt.to_string());
        if self.entries.len() > MAX_HISTORY_ENTRIES {
            self.entries.remove(0);
        }
    }

    /// The previous prompt, stashing `draft` the first time recall starts.
    fn previous(&mut self, draft: &str) -> Option<&str> {
        let index = match self.index {
            Some(0) => return None,
            Some(index) => index - 1,
            None => {
                self.draft = Some(draft.to_string());
                self.entries.len().checked_sub(1)?
            }
        };
        self.index = Some(index);
        self.entries.get(index).map(String::as_str)
    }

    /// The next prompt, or the stashed draft once recall walks past the newest.
    fn next(&mut self) -> Option<String> {
        let index = self.index?;
        if index + 1 < self.entries.len() {
            self.index = Some(index + 1);
            return self.entries.get(index + 1).cloned();
        }
        self.index = None;
        Some(self.draft.take().unwrap_or_default())
    }

    /// Ends navigation, so the recalled prompt becomes the user's own draft.
    fn reset(&mut self) {
        self.index = None;
        self.draft = None;
    }
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
    AcceptedCommand(CommandEntry),
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

    pub fn selected_mentions(&self) -> Vec<SelectedFileMention> {
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

    pub fn take_submission(&mut self) -> (String, Vec<PromptAttachment>) {
        let text = self.buffer.take();
        let pending_media = std::mem::take(&mut self.pending_media);
        self.history.push(&text);
        self.overlay = None;
        self.mentions.clear();
        self.history.reset();
        (text, pending_media)
    }

    pub fn clear(&mut self) {
        self.buffer.clear();
        self.pending_media.clear();
        self.overlay = None;
        self.mentions.clear();
        self.history.reset();
    }

    pub fn add_dropped_media(&mut self, paths: Vec<std::path::PathBuf>) -> bool {
        // Atomic across the whole parsed list: if any path is missing or not a
        // regular file, the paste is ambiguous, so attach nothing and let the
        // caller keep the original payload as composer text.
        if !paths.iter().all(|path| path.is_file()) {
            return false;
        }

        let mut existing: HashSet<std::path::PathBuf> = self.pending_media.iter().map(|a| a.path.clone()).collect();

        let before = self.pending_media.len();

        for path in paths {
            if !matches!(classify_attachment(&path), AttachmentKind::Image | AttachmentKind::Audio) {
                continue;
            }
            if !existing.insert(path.clone()) {
                continue;
            }
            let display_name = path
                .file_name()
                .map_or_else(|| path.to_string_lossy().into_owned(), |n| n.to_string_lossy().into_owned());
            self.pending_media.push(PromptAttachment { path, display_name });
        }

        self.pending_media.len() > before
    }

    pub fn pending_media(&self) -> &[PromptAttachment] {
        &self.pending_media
    }

    /// Applies one shared editing keystroke. Anything that changes the text also
    /// ends history navigation, so the recalled prompt becomes the user's draft.
    pub fn apply_edit_key(&mut self, key: crossterm::event::KeyEvent) -> bool {
        if key.code == KeyCode::Backspace {
            self.backspace();
            return true;
        }
        let before = self.buffer.text().len();
        let handled = crate::edit_buffer::apply_edit_key(&mut self.buffer, key);
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
        let Some(prompt) = self.history.previous(self.buffer.text()).map(str::to_string) else {
            return false;
        };
        self.set_text(prompt);
        self.buffer.set_cursor(0);
        true
    }

    pub fn recall_next(&mut self) -> bool {
        let Some(prompt) = self.history.next() else {
            return false;
        };
        self.set_text(prompt);
        self.buffer.move_to_end();
        true
    }

    pub fn open_command_picker(&mut self, commands: Vec<CommandEntry>) {
        self.overlay = Some(Overlay::Completion(CompletionOverlay::command(commands)));
    }

    /// Opens the `@` picker and asks for the file index it will show. The walk
    /// runs off the event loop, so opening the picker never stalls the keystroke
    /// that triggered it.
    pub fn open_file_picker(&mut self, root: &std::path::Path) -> Task {
        let request_id = Generation::next();
        self.overlay = Some(Overlay::Completion(CompletionOverlay::file(request_id)));
        Task::IndexFiles { request_id, root: root.to_path_buf() }
    }

    pub fn on_files_indexed(&mut self, request_id: Generation, files: Vec<FileEntry>) {
        if let Some(overlay) = self.completion() {
            overlay.set_files(request_id, files);
        }
    }

    /// Whether the completion list is open.
    pub fn has_completion(&self) -> bool {
        matches!(self.overlay, Some(Overlay::Completion(_)))
    }

    /// The open completion list, for navigation and rendering.
    pub fn completion(&mut self) -> Option<&mut CompletionOverlay> {
        match self.overlay.as_mut()? {
            Overlay::Completion(overlay) => Some(overlay),
            Overlay::PromptSearch { .. } => None,
        }
    }

    /// The open prompt-history picker, for queries and rendering.
    pub fn prompt_search(&mut self) -> Option<&mut PromptSearchPicker> {
        match self.overlay.as_mut()? {
            Overlay::PromptSearch { picker, .. } => Some(picker),
            Overlay::Completion(_) => None,
        }
    }

    /// Whether any inline surface is open around the composer's text.
    pub fn has_open_overlay(&self) -> bool {
        self.overlay.is_some()
    }

    /// Routes a mouse event to whichever overlay is open. Browsing history
    /// results previews each candidate in the composer, the way the arrow keys
    /// do.
    pub fn on_overlay_mouse(&mut self, action: MouseAction, row: u16) {
        let direction = match action {
            MouseAction::ScrollUp => Some(Direction::Backward),
            MouseAction::ScrollDown => Some(Direction::Forward),
            MouseAction::Click => None,
        };
        match self.overlay.as_mut() {
            Some(Overlay::Completion(overlay)) => match direction {
                Some(direction) => overlay.step(direction),
                None => overlay.select_at(row),
            },
            Some(Overlay::PromptSearch { picker, .. }) => {
                match direction {
                    Some(direction) => picker.step(direction),
                    None => picker.select_at(row),
                }
                self.apply_selected_search_result();
            }
            None => {}
        }
    }

    pub fn has_prompt_search(&self) -> bool {
        matches!(self.overlay, Some(Overlay::PromptSearch { .. }))
    }

    pub fn open_prompt_search(&mut self) {
        let draft = self.buffer.text().to_string();
        self.overlay = Some(Overlay::PromptSearch { picker: PromptSearchPicker::new(), draft });
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
    pub fn on_prompt_search_key(&mut self, key: crossterm::event::KeyEvent) -> Option<ComposerOutcome> {
        if !self.has_prompt_search() {
            return None;
        }
        let query = self.prompt_search_query_on_key(key);
        Some(self.search_outcome(query))
    }

    /// Applies a paste to the open history search, or reports that there is none
    /// for it to go to.
    pub fn on_prompt_search_paste(&mut self, text: &str) -> Option<ComposerOutcome> {
        let query = self.prompt_search()?.push_str(text);
        Some(self.search_outcome(Some(query)))
    }

    /// Applies a keystroke to the open completion list, or reports that there is
    /// none for it to go to.
    pub fn on_completion_key(&mut self, key: crossterm::event::KeyEvent) -> Option<ComposerOutcome> {
        if !self.has_completion() {
            return None;
        }
        Some(match self.completion_on_key(key) {
            Some(command) => ComposerOutcome::AcceptedCommand(command),
            None => ComposerOutcome::Handled,
        })
    }

    /// A new query goes to the agent; an emptied one puts back the draft the
    /// search replaced, which is the composer's own business.
    fn search_outcome(&mut self, query: Option<String>) -> ComposerOutcome {
        match query {
            Some(query) if !query.trim().is_empty() => ComposerOutcome::Search(query),
            Some(_) => {
                self.restore_prompt_search_draft();
                ComposerOutcome::Handled
            }
            None => ComposerOutcome::Handled,
        }
    }

    /// Applies a keystroke to the open history search, returning the query to
    /// re-run when the keystroke changed it.
    fn prompt_search_query_on_key(&mut self, key: crossterm::event::KeyEvent) -> Option<String> {
        let picker = self.prompt_search()?;
        match key.code {
            KeyCode::Esc => {
                self.close_prompt_search(false);
                None
            }
            KeyCode::Enter => {
                let confirmed = picker.selected_result().is_some();
                self.close_prompt_search(confirmed);
                None
            }
            KeyCode::Down => {
                picker.step(Direction::Forward);
                self.apply_selected_search_result();
                None
            }
            KeyCode::Up => {
                picker.step(Direction::Backward);
                self.apply_selected_search_result();
                None
            }
            KeyCode::Backspace => Some(picker.backspace()),
            KeyCode::Char(c)
                if !key
                    .modifiers
                    .intersects(crossterm::event::KeyModifiers::CONTROL | crossterm::event::KeyModifiers::ALT) =>
            {
                Some(picker.push_char(c))
            }
            _ => None,
        }
    }

    pub fn prompt_search_on_results(&mut self, response: PromptSearchResponse) {
        let Some(picker) = self.prompt_search() else {
            return;
        };
        if picker.on_results(response) {
            self.apply_selected_search_result();
        }
    }

    /// Puts back the draft the search replaced, for a query that no longer
    /// selects anything.
    fn restore_prompt_search_draft(&mut self) {
        if let Some(Overlay::PromptSearch { draft, .. }) = &self.overlay {
            self.buffer.set_text(draft.clone());
        }
    }

    fn apply_selected_search_result(&mut self) {
        let Some(result) = self.prompt_search().and_then(|picker| picker.selected_result()) else {
            return;
        };
        let prompt = result.prompt.clone();
        let cursor = prompt_search::cursor_at_match_end(&prompt, result.match_end);
        self.buffer.set_text(prompt);
        self.buffer.set_cursor(cursor);
    }

    /// Applies a keystroke to the open completion list, returning the command it
    /// accepted.
    ///
    /// The mirror of [`Composer::on_prompt_search_key`]: the composer owns the
    /// list's own keys and the edits they imply, and the app decides what an
    /// accepted command means.
    fn completion_on_key(&mut self, key: crossterm::event::KeyEvent) -> Option<CommandEntry> {
        match key.code {
            KeyCode::Esc => self.close_overlay(),
            KeyCode::Up => self.step_completion(Direction::Backward),
            KeyCode::Down => self.step_completion(Direction::Forward),
            KeyCode::Enter | KeyCode::Tab => {
                let command = self.accept_command();
                if command.is_none() {
                    self.accept_file();
                }
                return command;
            }
            KeyCode::Backspace if self.active_token_is_empty() => {
                self.backspace();
                self.close_overlay();
            }
            KeyCode::Backspace => {
                self.backspace();
                self.refresh_overlay_query();
            }
            KeyCode::Char(character)
                if !key
                    .modifiers
                    .intersects(crossterm::event::KeyModifiers::CONTROL | crossterm::event::KeyModifiers::ALT) =>
            {
                self.insert_char(character);
                if character.is_whitespace() {
                    self.close_overlay();
                } else {
                    self.refresh_overlay_query();
                }
            }
            _ => {}
        }
        None
    }

    fn step_completion(&mut self, direction: Direction) {
        if let Some(overlay) = self.completion() {
            overlay.step(direction);
        }
    }

    fn close_overlay(&mut self) {
        self.overlay = None;
    }

    pub fn accept_command(&mut self) -> Option<CommandEntry> {
        let command = self.completion()?.selected_command()?;
        self.replace_token('/', &format!("/{}", command.name));
        self.overlay = None;
        Some(command)
    }

    pub fn accept_file(&mut self) -> Option<FileEntry> {
        let file = self.completion()?.selected_file()?;
        self.replace_token('@', &format!("@{} ", file.display_name));
        self.mentions.push(SelectedFileMention { path: file.path.clone(), display_name: file.display_name.clone() });
        self.overlay = None;
        Some(file)
    }

    pub fn refresh_overlay_query(&mut self) {
        let Some(trigger) = self.completion().map(|overlay| overlay.trigger()) else {
            return;
        };
        let query = self
            .active_token(trigger)
            .map_or_else(String::new, |range| self.buffer.text()[range.start + 1..range.end].to_string());
        if let Some(overlay) = self.completion() {
            overlay.set_query(query);
        }
    }

    fn active_token_is_empty(&mut self) -> bool {
        self.completion().is_some_and(|overlay| overlay.query().is_empty())
    }

    fn set_content_width(&mut self, content_width: usize) {
        self.content_width = Some(content_width);
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

    /// The cursor's byte offset into [`Composer::text`].
    pub(crate) fn cursor_byte(&self) -> usize {
        self.buffer.cursor()
    }

    pub fn cursor_position(&self) -> (usize, usize) {
        let before = &self.buffer.text()[..self.buffer.cursor()];
        let row = before.matches('\n').count();
        let column = before[self.buffer.line_start()..].width();
        (row, column)
    }

    fn active_token(&self, trigger: char) -> Option<std::ops::Range<usize>> {
        let before_cursor = &self.buffer.text()[..self.buffer.cursor()];
        let start = before_cursor.rfind(trigger)?;
        let before_trigger = &before_cursor[..start];
        (trigger == '/' && start == 0
            || trigger == '@' && (before_trigger.is_empty() || before_trigger.ends_with(char::is_whitespace)))
        .then_some(start..self.buffer.cursor())
    }

    fn replace_token(&mut self, trigger: char, replacement: &str) {
        let Some(range) = self.active_token(trigger) else {
            return;
        };
        self.buffer.replace_range(range, replacement);
    }

    fn set_text(&mut self, text: String) {
        self.buffer.set_text(text);
        self.mentions.clear();
        self.overlay = None;
    }
}
