mod view;

use crate::attachments::{AttachmentKind, PromptAttachment, classify_attachment};
use crate::edit_buffer::EditBuffer;
use crate::effects::{self, Effect};
use crate::picker::{CommandEntry, CompletionOverlay, FileEntry};
use crate::prompt_search::{self, PromptSearchPicker};
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
        let mut existing: HashSet<std::path::PathBuf> =
            self.pending_media.iter().filter_map(|a| std::fs::canonicalize(&a.path).ok()).collect();

        let before = self.pending_media.len();

        for path in paths {
            let kind = classify_attachment(&path);
            if !matches!(kind, AttachmentKind::Image | AttachmentKind::Audio) {
                continue;
            }
            if let Ok(canon) = std::fs::canonicalize(&path)
                && !existing.insert(canon)
            {
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
        let filtered: String = text.chars().filter(|c| !c.is_control() || *c == '\n' || *c == '\t').collect();
        self.buffer.insert_str(&filtered);
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

    pub fn move_up(&mut self) -> bool {
        self.buffer.move_up()
    }

    pub fn move_down(&mut self) -> bool {
        self.buffer.move_down()
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
    pub fn open_file_picker(&mut self, root: &std::path::Path) -> Effect {
        let request_id = effects::next_request_id();
        self.overlay = Some(Overlay::Completion(CompletionOverlay::file(request_id)));
        Effect::IndexFiles { request_id, root: root.to_path_buf() }
    }

    pub fn on_files_indexed(&mut self, request_id: u64, files: Vec<FileEntry>) {
        if let Some(overlay) = self.completion() {
            overlay.set_files(request_id, files);
        }
    }

    /// Whether the completion list is open.
    pub fn has_overlay(&self) -> bool {
        self.completion_ref().is_some()
    }

    /// The open completion list, for navigation and rendering.
    pub fn completion(&mut self) -> Option<&mut CompletionOverlay> {
        match self.overlay.as_mut()? {
            Overlay::Completion(overlay) => Some(overlay),
            Overlay::PromptSearch { .. } => None,
        }
    }

    pub fn completion_ref(&self) -> Option<&CompletionOverlay> {
        match self.overlay.as_ref()? {
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

    pub fn prompt_search_ref(&self) -> Option<&PromptSearchPicker> {
        match self.overlay.as_ref()? {
            Overlay::PromptSearch { picker, .. } => Some(picker),
            Overlay::Completion(_) => None,
        }
    }

    /// Moves through history results and mirrors the newly selected prompt into
    /// the composer, so browsing previews each candidate in place.
    pub fn navigate_prompt_search(&mut self, navigate: impl FnOnce(&mut PromptSearchPicker)) {
        let Some(picker) = self.prompt_search() else {
            return;
        };
        navigate(picker);
        self.apply_selected_search_result();
    }

    pub fn has_prompt_search(&self) -> bool {
        self.prompt_search_ref().is_some()
    }

    pub fn open_prompt_search(&mut self) {
        let draft = self.buffer.text().to_string();
        self.overlay = Some(Overlay::PromptSearch { picker: PromptSearchPicker::new(), draft });
    }

    /// Closes the search, restoring the draft it replaced unless the user
    /// confirmed one of the results.
    pub fn close_prompt_search(&mut self, confirmed: bool) {
        let Some(Overlay::PromptSearch { draft, .. }) = self.overlay.take() else {
            return;
        };
        if !confirmed {
            self.buffer.set_text(draft);
        }
    }

    /// Applies a keystroke to the open history search, returning the query to
    /// re-run when the keystroke changed it.
    pub fn prompt_search_on_key(&mut self, key: crossterm::event::KeyEvent) -> Option<String> {
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
                picker.move_down();
                self.apply_selected_search_result();
                None
            }
            KeyCode::Up => {
                picker.move_up();
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

    pub fn prompt_search_on_paste(&mut self, text: &str) -> Option<String> {
        Some(self.prompt_search()?.push_str(text))
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
    pub fn restore_prompt_search_draft(&mut self) {
        if let Some(Overlay::PromptSearch { draft, .. }) = &self.overlay {
            self.buffer.set_text(draft.clone());
        }
    }

    fn apply_selected_search_result(&mut self) {
        let Some(result) = self.prompt_search_ref().and_then(PromptSearchPicker::selected_result) else {
            return;
        };
        let prompt = result.prompt.clone();
        let cursor = prompt_search::cursor_at_match_end(&prompt, result.match_end);
        self.buffer.set_text(prompt);
        self.buffer.set_cursor(cursor);
    }

    pub fn close_overlay(&mut self) {
        self.overlay = None;
    }

    pub fn accept_command(&mut self) -> Option<CommandEntry> {
        let command = self.completion_ref()?.selected_command()?;
        self.replace_token('/', &format!("/{}", command.name));
        self.overlay = None;
        Some(command)
    }

    pub fn accept_file(&mut self) -> Option<FileEntry> {
        let file = self.completion_ref()?.selected_file()?;
        self.replace_token('@', &format!("@{} ", file.display_name));
        self.mentions.push(SelectedFileMention { path: file.path.clone(), display_name: file.display_name.clone() });
        self.overlay = None;
        Some(file)
    }

    pub fn refresh_overlay_query(&mut self) {
        let Some(trigger) = self.completion_ref().map(CompletionOverlay::trigger) else {
            return;
        };
        let query = self
            .active_token(trigger)
            .map_or_else(String::new, |range| self.buffer.text()[range.start + 1..range.end].to_string());
        if let Some(overlay) = self.completion() {
            overlay.set_query(query);
        }
    }

    pub fn active_token_is_empty(&self) -> bool {
        self.completion_ref().is_some_and(|overlay| overlay.query().is_empty())
    }

    pub fn line_count(&self) -> usize {
        self.buffer.text().split('\n').count()
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
