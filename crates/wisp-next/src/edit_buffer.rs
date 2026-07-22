use ratatui_textarea::{CursorMove, TextArea};

#[derive(Clone, Debug)]
pub struct EditBuffer {
    textarea: Box<TextArea<'static>>,
    text: String,
}

impl Default for EditBuffer {
    fn default() -> Self {
        Self::new("")
    }
}

impl PartialEq for EditBuffer {
    fn eq(&self, other: &Self) -> bool {
        self.text == other.text && self.cursor() == other.cursor()
    }
}

impl Eq for EditBuffer {}

impl EditBuffer {
    pub fn new(text: impl Into<String>) -> Self {
        let text = text.into();
        let mut buffer = Self { textarea: Box::new(TextArea::from(text.lines())), text };
        if buffer.text.ends_with('\n') {
            buffer.textarea.insert_newline();
        }
        buffer.move_to_end();
        buffer
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn cursor(&self) -> usize {
        let cursor = self.textarea.cursor();
        self.text.split_inclusive('\n').take(cursor.0).map(str::len).sum::<usize>()
            + self.textarea.lines()[cursor.0]
                .char_indices()
                .nth(cursor.1)
                .map_or_else(|| self.textarea.lines()[cursor.0].len(), |(offset, _)| offset)
    }

    pub fn is_empty(&self) -> bool {
        self.textarea.is_empty()
    }

    pub fn set_text(&mut self, text: impl Into<String>) {
        let cursor = self.cursor();
        self.replace_text(text.into(), cursor);
    }

    pub fn set_cursor(&mut self, cursor: usize) {
        let mut cursor = cursor.min(self.text.len());
        while !self.text.is_char_boundary(cursor) {
            cursor -= 1;
        }
        let cursor = self.text[..cursor]
            .char_indices()
            .next_back()
            .filter(|(offset, _)| *offset < cursor)
            .map_or(cursor, |(offset, character)| offset + character.len_utf8());
        let before = &self.text[..cursor];
        let row = before.matches('\n').count();
        let line_start = before.rfind('\n').map_or(0, |offset| offset + 1);
        let column = self.text[line_start..cursor].chars().count();
        self.textarea.move_cursor(CursorMove::Jump(
            u16::try_from(row).unwrap_or(u16::MAX),
            u16::try_from(column).unwrap_or(u16::MAX),
        ));
    }

    pub fn move_to_end(&mut self) {
        self.textarea.move_cursor(CursorMove::Bottom);
        self.textarea.move_cursor(CursorMove::End);
    }

    pub fn insert_char(&mut self, character: char) {
        self.textarea.insert_char(character);
        self.sync_text();
    }

    pub fn insert_str(&mut self, text: &str) {
        self.textarea.insert_str(text);
        self.sync_text();
    }

    pub fn insert_newline(&mut self) {
        self.textarea.insert_newline();
        self.sync_text();
    }

    pub fn backspace(&mut self) -> bool {
        let changed = self.textarea.delete_char();
        self.sync_text();
        changed
    }

    pub fn delete(&mut self) -> bool {
        let changed = self.textarea.delete_str(1);
        self.sync_text();
        changed
    }

    pub fn move_left(&mut self) -> bool {
        self.move_cursor(CursorMove::Back)
    }

    pub fn move_right(&mut self) -> bool {
        self.move_cursor(CursorMove::Forward)
    }

    pub fn move_line_start(&mut self) {
        self.textarea.move_cursor(CursorMove::Head);
    }

    pub fn move_line_end(&mut self) {
        self.textarea.move_cursor(CursorMove::End);
    }

    pub fn move_up(&mut self) -> bool {
        self.move_cursor(CursorMove::Up)
    }

    pub fn move_down(&mut self) -> bool {
        self.move_cursor(CursorMove::Down)
    }

    pub fn replace_range(&mut self, range: std::ops::Range<usize>, replacement: &str) {
        let mut text = self.text.clone();
        text.replace_range(range.clone(), replacement);
        self.replace_text(text, range.start + replacement.len());
    }

    pub fn clear(&mut self) {
        self.replace_text(String::new(), 0);
    }

    pub fn take(&mut self) -> String {
        let text = std::mem::take(&mut self.text);
        *self.textarea = TextArea::default();
        text
    }

    pub fn line_start(&self) -> usize {
        self.text[..self.cursor()].rfind('\n').map_or(0, |index| index + 1)
    }

    pub fn textarea(&self) -> &TextArea<'static> {
        &self.textarea
    }

    fn move_cursor(&mut self, direction: CursorMove) -> bool {
        let before = self.textarea.cursor();
        self.textarea.move_cursor(direction);
        self.textarea.cursor() != before
    }

    fn replace_text(&mut self, text: String, cursor: usize) {
        *self.textarea = TextArea::from(text.lines());
        self.text = text;
        if self.text.ends_with('\n') {
            self.textarea.insert_newline();
        }
        self.set_cursor(cursor);
    }

    fn sync_text(&mut self) {
        self.text = self.textarea.lines().join("\n");
    }
}

impl From<String> for EditBuffer {
    fn from(text: String) -> Self {
        Self::new(text)
    }
}

impl From<&str> for EditBuffer {
    fn from(text: &str) -> Self {
        Self::new(text)
    }
}

impl AsRef<str> for EditBuffer {
    fn as_ref(&self) -> &str {
        self.text()
    }
}

impl std::ops::Deref for EditBuffer {
    type Target = str;

    fn deref(&self) -> &Self::Target {
        self.text()
    }
}

impl std::fmt::Display for EditBuffer {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.text())
    }
}

impl PartialEq<str> for EditBuffer {
    fn eq(&self, other: &str) -> bool {
        self.text() == other
    }
}

#[cfg(test)]
mod tests {
    use super::EditBuffer;

    #[test]
    fn edits_graphemes_through_native_textarea() {
        let mut buffer = EditBuffer::new("a界");
        assert!(buffer.move_left());
        buffer.insert_char('🙂');
        assert_eq!(buffer.text(), "a🙂界");
        assert!(buffer.backspace());
        assert_eq!(buffer.text(), "a界");
    }
}
