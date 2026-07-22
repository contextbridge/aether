use unicode_width::UnicodeWidthStr;

use crate::wrap::byte_at_display_column;

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct EditBuffer {
    text: String,
    cursor: usize,
}

impl EditBuffer {
    pub fn new(text: impl Into<String>) -> Self {
        let text = text.into();
        let cursor = text.len();
        Self { text, cursor }
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn cursor(&self) -> usize {
        self.cursor
    }

    pub fn is_empty(&self) -> bool {
        self.text.is_empty()
    }

    pub fn set_text(&mut self, text: impl Into<String>) {
        self.text = text.into();
        self.cursor = self.cursor.min(self.text.len());
        while !self.text.is_char_boundary(self.cursor) {
            self.cursor -= 1;
        }
    }

    pub fn set_cursor(&mut self, cursor: usize) {
        self.cursor = cursor.min(self.text.len());
        while !self.text.is_char_boundary(self.cursor) {
            self.cursor -= 1;
        }
    }

    pub fn move_to_end(&mut self) {
        self.cursor = self.text.len();
    }

    pub fn insert_char(&mut self, character: char) {
        self.text.insert(self.cursor, character);
        self.cursor += character.len_utf8();
    }

    pub fn insert_str(&mut self, text: &str) {
        self.text.insert_str(self.cursor, text);
        self.cursor += text.len();
    }

    pub fn backspace(&mut self) -> bool {
        let Some((index, _)) = self.text[..self.cursor].char_indices().next_back() else {
            return false;
        };
        self.text.drain(index..self.cursor);
        self.cursor = index;
        true
    }

    pub fn delete(&mut self) -> bool {
        let Some(character) = self.text[self.cursor..].chars().next() else {
            return false;
        };
        self.text.drain(self.cursor..self.cursor + character.len_utf8());
        true
    }

    pub fn move_left(&mut self) -> bool {
        let Some((index, _)) = self.text[..self.cursor].char_indices().next_back() else {
            return false;
        };
        self.cursor = index;
        true
    }

    pub fn move_right(&mut self) -> bool {
        let Some(character) = self.text[self.cursor..].chars().next() else {
            return false;
        };
        self.cursor += character.len_utf8();
        true
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
        let current_end = self.text[self.cursor..].find('\n').map(|offset| self.cursor + offset);
        let Some(current_end) = current_end else {
            return false;
        };
        let target_start = current_end + 1;
        let target_end = self.text[target_start..].find('\n').map_or(self.text.len(), |offset| target_start + offset);
        let column = self.text[self.line_start()..self.cursor].width();
        self.cursor = target_start + byte_at_display_column(&self.text[target_start..target_end], column);
        true
    }

    pub fn replace_range(&mut self, range: std::ops::Range<usize>, replacement: &str) {
        self.text.replace_range(range.clone(), replacement);
        self.cursor = range.start + replacement.len();
    }

    pub fn clear(&mut self) {
        self.text.clear();
        self.cursor = 0;
    }

    pub fn take(&mut self) -> String {
        self.cursor = 0;
        std::mem::take(&mut self.text)
    }

    pub fn line_start(&self) -> usize {
        self.text[..self.cursor].rfind('\n').map_or(0, |index| index + 1)
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
    fn edits_unicode_only_at_character_boundaries() {
        let mut buffer = EditBuffer::new("a界");
        assert!(buffer.move_left());
        assert_eq!(buffer.cursor(), 1);
        buffer.insert_char('🙂');
        assert_eq!(buffer.text(), "a🙂界");
        assert!(buffer.backspace());
        assert_eq!(buffer.text(), "a界");
        assert_eq!(buffer.cursor(), 1);
        assert!(buffer.delete());
        assert_eq!(buffer.text(), "a");
    }

    #[test]
    fn vertical_movement_preserves_display_column() {
        let mut buffer = EditBuffer::new("a界x\n12345");
        buffer.set_cursor(4);
        assert!(buffer.move_down());
        assert_eq!(&buffer.text()[buffer.cursor()..], "45");
        assert!(buffer.move_up());
        assert_eq!(buffer.cursor(), 4);
    }
}
