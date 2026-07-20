use crate::picker::{CommandEntry, FileEntry, Overlay};
use crate::theme::Theme;
use ratatui::layout::Position;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
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
    history: Vec<String>,
    history_index: Option<usize>,
    history_draft: Option<String>,
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
        self.text.is_empty()
    }

    pub fn selected_mentions(&self) -> Vec<SelectedFileMention> {
        let words: std::collections::HashSet<&str> = self.text.split_whitespace().collect();
        self.mentions
            .iter()
            .filter(|mention| words.contains(format!("@{}", mention.display_name).as_str()))
            .cloned()
            .collect()
    }

    pub fn take_submission(&mut self) -> String {
        let text = std::mem::take(&mut self.text);
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
        text
    }

    pub fn clear(&mut self) {
        self.text.clear();
        self.cursor = 0;
        self.overlay = None;
        self.mentions.clear();
        self.reset_history_navigation();
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

    pub fn insert_newline(&mut self) {
        self.insert_char('\n');
        self.overlay = None;
    }

    pub fn backspace(&mut self) {
        self.reset_history_navigation();
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
        let mut lines = Vec::new();
        let mut cursor = Position::new(2, 0);
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
        if lines.is_empty() {
            lines.push(Line::styled("> ", Style::new().fg(theme.accent)));
        }
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
