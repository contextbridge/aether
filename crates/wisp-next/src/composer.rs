use crate::attachments::{AttachmentKind, PromptAttachment, classify_attachment};
use crate::picker::{CommandEntry, FileEntry, Overlay};
use crate::prompt_search::{self, PromptSearchMessage, PromptSearchPicker};
use crate::theme::Theme;
use acp_utils::notifications::PromptSearchResponse;
use crossterm::event::KeyCode;
use ratatui::layout::Position;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use std::collections::HashSet;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SelectedFileMention {
    pub path: std::path::PathBuf,
    pub display_name: String,
}

#[derive(Debug, Default)]
pub struct Composer {
    text: String,
    cursor: usize,
    overlay: Option<Overlay>,
    mentions: Vec<SelectedFileMention>,
    pending_media: Vec<PromptAttachment>,
    history: Vec<String>,
    history_index: Option<usize>,
    history_draft: Option<String>,
    prompt_search: Option<PromptSearchState>,
}

#[derive(Debug)]
struct PromptSearchState {
    picker: PromptSearchPicker,
    draft: String,
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
        &self.text
    }

    pub fn is_empty(&self) -> bool {
        self.text.is_empty() && self.pending_media.is_empty()
    }

    pub fn selected_mentions(&self) -> Vec<SelectedFileMention> {
        let words: HashSet<&str> = self.text.split_whitespace().collect();
        self.mentions
            .iter()
            .filter(|mention| words.contains(format!("@{}", mention.display_name).as_str()))
            .cloned()
            .collect()
    }

    pub fn take_submission(&mut self) -> (String, Vec<PromptAttachment>) {
        let text = std::mem::take(&mut self.text);
        let pending_media = std::mem::take(&mut self.pending_media);
        if !text.trim().is_empty() {
            self.history.push(text.clone());
            if self.history.len() > 500 {
                self.history.remove(0);
            }
        }
        self.cursor = 0;
        self.overlay = None;
        self.mentions.clear();
        self.reset_history_navigation();
        (text, pending_media)
    }

    pub fn clear(&mut self) {
        self.text.clear();
        self.pending_media.clear();
        self.cursor = 0;
        self.overlay = None;
        self.mentions.clear();
        self.reset_history_navigation();
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

    pub fn insert_char(&mut self, character: char) {
        self.reset_history_navigation();
        self.text.insert(self.cursor, character);
        self.cursor += character.len_utf8();
    }

    pub fn insert_str(&mut self, text: &str) {
        self.reset_history_navigation();
        self.text.insert_str(self.cursor, text);
        self.cursor += text.len();
    }

    pub fn insert_paste(&mut self, text: &str) {
        self.reset_history_navigation();
        let filtered: String = text.chars().filter(|c| !c.is_control() || *c == '\n' || *c == '\t').collect();
        self.text.insert_str(self.cursor, &filtered);
        self.cursor += filtered.len();
    }

    pub fn insert_newline(&mut self) {
        self.insert_char('\n');
        self.overlay = None;
    }

    pub fn backspace(&mut self) {
        self.reset_history_navigation();
        if self.text.is_empty() && !self.pending_media.is_empty() {
            self.pending_media.pop();
            return;
        }
        if let Some(character) = self.text[..self.cursor].chars().next_back() {
            self.cursor -= character.len_utf8();
            self.text.remove(self.cursor);
        }
    }

    pub fn delete(&mut self) {
        self.reset_history_navigation();
        if self.cursor < self.text.len() {
            self.text.remove(self.cursor);
        }
    }

    pub fn move_left(&mut self) {
        if let Some(character) = self.text[..self.cursor].chars().next_back() {
            self.cursor -= character.len_utf8();
        }
    }

    pub fn move_right(&mut self) {
        if let Some(character) = self.text[self.cursor..].chars().next() {
            self.cursor += character.len_utf8();
        }
    }

    pub fn move_line_start(&mut self) {
        self.cursor = self.line_start();
    }

    pub fn move_line_end(&mut self) {
        self.cursor = self.text[self.cursor..].find('\n').map_or(self.text.len(), |offset| self.cursor + offset);
    }

    pub fn move_up(&mut self) -> bool {
        let current_start = self.line_start();
        if current_start == 0 {
            return false;
        }
        let target_end = current_start - 1;
        let target_start = self.text[..target_end].rfind('\n').map_or(0, |index| index + 1);
        let column = self.text[current_start..self.cursor].width();
        self.cursor = target_start + byte_at_display_column(&self.text[target_start..target_end], column);
        true
    }

    pub fn move_down(&mut self) -> bool {
        let current_end = self.text[self.cursor..].find('\n').map_or(self.text.len(), |offset| self.cursor + offset);
        if current_end == self.text.len() {
            return false;
        }
        let target_start = current_end + 1;
        let target_end = self.text[target_start..].find('\n').map_or(self.text.len(), |offset| target_start + offset);
        let column = self.text[self.line_start()..self.cursor].width();
        self.cursor = target_start + byte_at_display_column(&self.text[target_start..target_end], column);
        true
    }

    pub fn recall_previous(&mut self) -> bool {
        if self.history.is_empty() {
            return false;
        }
        let index = match self.history_index {
            Some(0) => return false,
            Some(index) => index - 1,
            None => {
                self.history_draft = Some(self.text.clone());
                self.history.len() - 1
            }
        };
        self.history_index = Some(index);
        self.set_text(self.history[index].clone());
        self.cursor = 0;
        true
    }

    pub fn recall_next(&mut self) -> bool {
        let Some(index) = self.history_index else {
            return false;
        };
        if index + 1 < self.history.len() {
            self.history_index = Some(index + 1);
            self.set_text(self.history[index + 1].clone());
        } else {
            let draft = self.history_draft.take().unwrap_or_default();
            self.set_text(draft);
            self.history_index = None;
        }
        self.cursor = self.text.len();
        true
    }

    pub fn open_command_picker(&mut self, commands: Vec<CommandEntry>) {
        self.overlay = Some(Overlay::command(commands));
    }

    pub fn open_file_picker(&mut self, root: &std::path::Path) {
        self.overlay = Some(Overlay::file(root));
    }

    pub fn has_overlay(&self) -> bool {
        self.overlay.is_some()
    }

    pub fn has_prompt_search(&self) -> bool {
        self.prompt_search.is_some()
    }

    pub fn open_prompt_search(&mut self) {
        let draft = self.text.clone();
        self.prompt_search = Some(PromptSearchState { picker: PromptSearchPicker::new(), draft });
    }

    pub fn close_prompt_search(&mut self, confirmed: bool) {
        let Some(state) = self.prompt_search.take() else {
            return;
        };
        if !confirmed {
            self.text = state.draft;
            self.cursor = self.cursor.min(self.text.len());
        }
    }

    pub fn prompt_search_on_key(&mut self, key: crossterm::event::KeyEvent) -> Option<PromptSearchMessage> {
        let state = self.prompt_search.as_mut()?;
        match key.code {
            KeyCode::Esc => {
                self.close_prompt_search(false);
                None
            }
            KeyCode::Enter => {
                let confirmed = state.picker.selected_result().is_some();
                self.close_prompt_search(confirmed);
                None
            }
            KeyCode::Down => {
                state.picker.move_down();
                self.apply_selected_search_result();
                Some(PromptSearchMessage::SelectionChanged)
            }
            KeyCode::Up => {
                state.picker.move_up();
                self.apply_selected_search_result();
                Some(PromptSearchMessage::SelectionChanged)
            }
            KeyCode::Backspace => Some(state.picker.backspace()),
            KeyCode::Char(c)
                if !key
                    .modifiers
                    .intersects(crossterm::event::KeyModifiers::CONTROL | crossterm::event::KeyModifiers::ALT) =>
            {
                Some(state.picker.push_char(c))
            }
            _ => None,
        }
    }

    pub fn prompt_search_on_paste(&mut self, text: &str) -> Option<PromptSearchMessage> {
        let state = self.prompt_search.as_mut()?;
        Some(state.picker.push_str(text))
    }

    pub fn prompt_search_on_results(&mut self, response: PromptSearchResponse) {
        let Some(state) = self.prompt_search.as_mut() else {
            return;
        };
        if state.picker.on_results(response) {
            self.apply_selected_search_result();
        }
    }

    pub fn prompt_search_on_failed(&mut self, search_generation: u64, error: String) {
        let Some(state) = self.prompt_search.as_mut() else {
            return;
        };
        let _ = state.picker.on_failed(search_generation, error);
    }

    pub fn restore_prompt_search_draft(&mut self) {
        if let Some(state) = &self.prompt_search {
            self.text = state.draft.clone();
            self.cursor = self.cursor.min(self.text.len());
        }
    }

    pub fn prompt_search_query(&self) -> Option<&str> {
        self.prompt_search.as_ref().map(|state| state.picker.query())
    }

    pub fn prompt_search_generation(&self) -> Option<u64> {
        self.prompt_search.as_ref().map(|state| state.picker.search_generation())
    }

    pub fn prompt_search_move_up(&mut self) {
        if let Some(state) = &mut self.prompt_search {
            state.picker.move_up();
            self.apply_selected_search_result();
        }
    }

    pub fn prompt_search_move_down(&mut self) {
        if let Some(state) = &mut self.prompt_search {
            state.picker.move_down();
            self.apply_selected_search_result();
        }
    }

    pub fn prompt_search_select_row(&mut self, row: usize) {
        if let Some(state) = &mut self.prompt_search {
            state.picker.select_row(row);
            self.apply_selected_search_result();
        }
    }

    pub fn prompt_search_lines(&self, width: u16, max_rows: usize, theme: &Theme) -> Vec<Line<'static>> {
        let Some(_state) = self.prompt_search.as_ref() else {
            return Vec::new();
        };
        let height = u16::try_from(max_rows).unwrap_or(u16::MAX).saturating_add(1);
        let mut buf = ratatui::buffer::Buffer::empty(ratatui::layout::Rect::new(0, 0, width, height));
        if let Some(state) = &self.prompt_search {
            state.picker.render(ratatui::layout::Rect::new(0, 0, width, height), &mut buf, theme);
        }
        let mut lines = Vec::new();
        for y in 0..=max_rows {
            let mut line_spans = Vec::new();
            for x in 0..width {
                if let Some(cell) = buf.cell((x, u16::try_from(y).unwrap_or(u16::MAX))) {
                    line_spans.push(Span::styled(cell.symbol().to_string(), cell.style()));
                } else {
                    line_spans.push(Span::raw(" "));
                }
            }
            let text: String = line_spans.iter().map(|s| s.content.as_ref()).collect();
            if text.trim().is_empty() {
                break;
            }
            lines.push(Line::from(line_spans));
        }
        lines
    }

    fn apply_selected_search_result(&mut self) {
        let Some(state) = self.prompt_search.as_ref() else {
            return;
        };
        if let Some(result) = state.picker.selected_result() {
            let cursor = prompt_search::cursor_at_match_end(&result.prompt, result.match_end);
            self.text = result.prompt.clone();
            self.cursor = cursor;
        }
    }

    pub fn overlay_lines(&self, width: u16, max_rows: usize, theme: &Theme) -> Vec<Line<'static>> {
        self.overlay.as_ref().map_or_else(Vec::new, |overlay| overlay.lines(width, max_rows, theme))
    }

    pub fn overlay_query(&self) -> Option<&str> {
        self.overlay.as_ref().map(Overlay::query)
    }

    pub fn overlay_move_up(&mut self) {
        if let Some(overlay) = &mut self.overlay {
            overlay.move_up();
        }
    }

    pub fn overlay_move_down(&mut self) {
        if let Some(overlay) = &mut self.overlay {
            overlay.move_down();
        }
    }

    pub fn overlay_select_row(&mut self, row: usize) {
        if let Some(overlay) = &mut self.overlay {
            overlay.select_row(row);
        }
    }

    pub fn close_overlay(&mut self) {
        self.overlay = None;
    }

    pub fn accept_command(&mut self) -> Option<CommandEntry> {
        let command = self.overlay.as_ref()?.selected_command()?;
        self.replace_token('/', &format!("/{}", command.name));
        self.overlay = None;
        Some(command)
    }

    pub fn accept_file(&mut self) -> Option<FileEntry> {
        let file = self.overlay.as_ref()?.selected_file()?;
        self.replace_token('@', &format!("@{} ", file.display_name));
        self.mentions.push(SelectedFileMention { path: file.path.clone(), display_name: file.display_name.clone() });
        self.overlay = None;
        Some(file)
    }

    pub fn refresh_overlay_query(&mut self) {
        let Some(trigger) = self.overlay.as_ref().map(|overlay| match overlay {
            Overlay::Command(_) => '/',
            Overlay::File(_) => '@',
        }) else {
            return;
        };
        let query = self
            .active_token(trigger)
            .map_or_else(String::new, |range| self.text[range.start + 1..range.end].to_string());
        if let Some(overlay) = &mut self.overlay {
            overlay.set_query(query);
        }
    }

    pub fn active_token_is_empty(&self) -> bool {
        self.overlay_query().is_some_and(str::is_empty)
    }

    pub fn lines(&self) -> impl Iterator<Item = &str> {
        self.text.split('\n')
    }

    pub fn line_count(&self) -> usize {
        self.text.split('\n').count()
    }

    pub fn cursor_position(&self) -> (usize, usize) {
        let before = &self.text[..self.cursor];
        let row = before.matches('\n').count();
        let column = before[self.line_start()..].chars().map(|character| character.width().unwrap_or(0)).sum();
        (row, column)
    }

    pub fn layout(&self, width: u16, theme: &Theme) -> ComposerLayout {
        let content_width = usize::from(width.saturating_sub(2).max(1));
        let full_width = usize::from(width);
        let mut lines = Vec::new();
        let mut cursor = Position::new(2, 0);

        lines.push(Line::styled("─".repeat(full_width), Style::new().fg(theme.muted)));

        let mut byte_offset = 0;
        for (logical_row, raw_line) in self.text.split('\n').enumerate() {
            let prefix = if logical_row == 0 { "> " } else { "  " };
            let wrapped = wrap_composer_line(raw_line, content_width);
            for (wrapped_row, chunk) in wrapped.iter().enumerate() {
                let row = u16::try_from(lines.len()).unwrap_or(u16::MAX);
                lines.push(Line::from(vec![
                    Span::styled(if wrapped_row == 0 { prefix } else { "  " }, Style::new().fg(theme.accent)),
                    Span::styled(chunk.clone(), Style::new().fg(theme.text_primary)),
                ]));
                let chunk_end = byte_offset + chunk.len();
                if self.cursor >= byte_offset && self.cursor <= chunk_end {
                    let column = self.text[byte_offset..self.cursor].width();
                    cursor = Position::new(u16::try_from(2 + column).unwrap_or(u16::MAX), row);
                }
                byte_offset = chunk_end;
            }
            if logical_row + 1 < self.line_count() {
                byte_offset += 1;
            }
        }

        for attachment in &self.pending_media {
            let kind = classify_attachment(&attachment.path);
            let label = match kind {
                AttachmentKind::Image => "image",
                AttachmentKind::Audio => "audio",
                _ => "file",
            };
            lines.push(Line::styled(
                format!("  attached {label}: {}", attachment.display_name),
                Style::new().fg(theme.info),
            ));
        }

        lines.push(Line::styled("─".repeat(full_width), Style::new().fg(theme.muted)));

        ComposerLayout { lines, cursor }
    }

    fn active_token(&self, trigger: char) -> Option<std::ops::Range<usize>> {
        let before_cursor = &self.text[..self.cursor];
        let start = before_cursor.rfind(trigger)?;
        let before_trigger = &before_cursor[..start];
        (trigger == '/' && start == 0
            || trigger == '@' && (before_trigger.is_empty() || before_trigger.ends_with(char::is_whitespace)))
        .then_some(start..self.cursor)
    }

    fn replace_token(&mut self, trigger: char, replacement: &str) {
        let Some(range) = self.active_token(trigger) else {
            return;
        };
        self.text.replace_range(range.clone(), replacement);
        self.cursor = range.start + replacement.len();
    }

    fn set_text(&mut self, text: String) {
        self.text = text;
        self.cursor = self.cursor.min(self.text.len());
        self.mentions.clear();
        self.overlay = None;
    }

    fn reset_history_navigation(&mut self) {
        self.history_index = None;
        self.history_draft = None;
    }

    fn line_start(&self) -> usize {
        self.text[..self.cursor].rfind('\n').map_or(0, |index| index + 1)
    }
}

fn byte_at_display_column(text: &str, column: usize) -> usize {
    let mut width = 0;
    for (index, character) in text.char_indices() {
        let next_width = width + character.width().unwrap_or(0);
        if next_width > column {
            return index;
        }
        width = next_width;
    }
    text.len()
}

fn wrap_composer_line(line: &str, width: usize) -> Vec<String> {
    let mut lines = Vec::new();
    let mut current = String::new();
    let mut current_width = 0;
    for character in line.chars() {
        let character_width = character.width().unwrap_or(0);
        if current_width + character_width > width && !current.is_empty() {
            lines.push(std::mem::take(&mut current));
            current_width = 0;
        }
        current.push(character);
        current_width += character_width;
    }
    lines.push(current);
    lines
}
