use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

/// A text buffer addressed by byte cursor, shared by every text input in the UI
/// (composer, commit message, review drafts, form fields).
///
/// The cursor is always held on a `char` boundary, so slicing `text()` at
/// `cursor()` is infallible.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct EditBuffer {
    text: String,
    cursor: usize,
}

impl EditBuffer {
    /// Creates a buffer holding `text` with the cursor at the end.
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

    /// Replaces the contents, keeping the cursor at the same byte offset where
    /// the new text is long enough.
    pub fn set_text(&mut self, text: impl Into<String>) {
        self.text = text.into();
        self.set_cursor(self.cursor);
    }

    pub fn set_cursor(&mut self, cursor: usize) {
        let mut cursor = cursor.min(self.text.len());
        while !self.text.is_char_boundary(cursor) {
            cursor -= 1;
        }
        self.cursor = cursor;
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
        let Some(previous) = self.previous_boundary() else {
            return false;
        };
        self.text.replace_range(previous..self.cursor, "");
        self.cursor = previous;
        true
    }

    pub fn delete(&mut self) -> bool {
        let Some(next) = self.next_boundary() else {
            return false;
        };
        self.text.replace_range(self.cursor..next, "");
        true
    }

    pub fn move_left(&mut self) -> bool {
        self.previous_boundary().is_some_and(|previous| {
            self.cursor = previous;
            true
        })
    }

    pub fn move_right(&mut self) -> bool {
        self.next_boundary().is_some_and(|next| {
            self.cursor = next;
            true
        })
    }

    pub fn move_line_start(&mut self) {
        self.cursor = self.line_start();
    }

    pub fn move_line_end(&mut self) {
        self.cursor = self.line_end();
    }

    pub fn move_up(&mut self) -> bool {
        let start = self.line_start();
        if start == 0 {
            return false;
        }
        let previous_start = self.text[..start - 1].rfind('\n').map_or(0, |index| index + 1);
        self.cursor = self.column_offset(previous_start, start - 1);
        true
    }

    pub fn move_down(&mut self) -> bool {
        let end = self.line_end();
        if end == self.text.len() {
            return false;
        }
        let next_start = end + 1;
        let next_end = self.text[next_start..].find('\n').map_or(self.text.len(), |index| next_start + index);
        self.cursor = self.column_offset(next_start, next_end);
        true
    }

    pub fn replace_range(&mut self, range: std::ops::Range<usize>, replacement: &str) {
        let cursor = range.start + replacement.len();
        self.text.replace_range(range, replacement);
        self.set_cursor(cursor);
    }

    pub fn clear(&mut self) {
        self.text.clear();
        self.cursor = 0;
    }

    /// Empties the buffer, returning what it held.
    pub fn take(&mut self) -> String {
        self.cursor = 0;
        std::mem::take(&mut self.text)
    }

    pub fn line_start(&self) -> usize {
        self.text[..self.cursor].rfind('\n').map_or(0, |index| index + 1)
    }

    fn line_end(&self) -> usize {
        self.text[self.cursor..].find('\n').map_or(self.text.len(), |index| self.cursor + index)
    }

    /// Byte offset within `start..end` at the cursor's current character column,
    /// clamped to the end of that range.
    fn column_offset(&self, start: usize, end: usize) -> usize {
        let column = self.text[self.line_start()..self.cursor].chars().count();
        self.text[start..end].char_indices().nth(column).map_or(end, |(offset, _)| start + offset)
    }

    fn previous_boundary(&self) -> Option<usize> {
        self.text[..self.cursor].chars().next_back().map(|character| self.cursor - character.len_utf8())
    }

    fn next_boundary(&self) -> Option<usize> {
        self.text[self.cursor..].chars().next().map(|character| self.cursor + character.len_utf8())
    }
}

/// Applies one editing keystroke to `buffer`, returning whether it was consumed.
///
/// This is the single key table shared by every text input: the composer, the
/// git-diff commit editor and review drafts, plan-review comments, the new
/// workspace name prompt, and elicitation form fields. Callers handle their own
/// Enter/Esc (and any vertical motion) before delegating here.
pub fn apply_edit_key(buffer: &mut EditBuffer, key: KeyEvent) -> bool {
    match key.code {
        KeyCode::Char('a') if key.modifiers.contains(KeyModifiers::CONTROL) => buffer.move_line_start(),
        KeyCode::Char('e') if key.modifiers.contains(KeyModifiers::CONTROL) => buffer.move_line_end(),
        KeyCode::Char(character) if !key.modifiers.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) => {
            buffer.insert_char(character);
        }
        KeyCode::Backspace => {
            buffer.backspace();
        }
        KeyCode::Delete => {
            buffer.delete();
        }
        KeyCode::Left => {
            buffer.move_left();
        }
        KeyCode::Right => {
            buffer.move_right();
        }
        KeyCode::Home => buffer.move_line_start(),
        KeyCode::End => buffer.move_line_end(),
        _ => return false,
    }
    true
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

impl std::ops::Deref for EditBuffer {
    type Target = str;

    fn deref(&self) -> &Self::Target {
        self.text()
    }
}

impl PartialEq<str> for EditBuffer {
    fn eq(&self, other: &str) -> bool {
        self.text() == other
    }
}

#[cfg(test)]
mod tests {
    use super::{EditBuffer, KeyCode, KeyEvent, KeyModifiers, apply_edit_key};

    #[test]
    fn edit_keys_cover_the_shared_table_and_reject_the_rest() {
        let mut buffer = EditBuffer::new("ab");
        assert!(apply_edit_key(&mut buffer, KeyEvent::new(KeyCode::Home, KeyModifiers::NONE)));
        assert_eq!(buffer.cursor(), 0);
        assert!(apply_edit_key(&mut buffer, KeyEvent::new(KeyCode::Char('X'), KeyModifiers::NONE)));
        assert_eq!(buffer.text(), "Xab");
        assert!(apply_edit_key(&mut buffer, KeyEvent::new(KeyCode::Char('e'), KeyModifiers::CONTROL)));
        assert_eq!(buffer.cursor(), 3);

        assert!(
            !apply_edit_key(&mut buffer, KeyEvent::new(KeyCode::Char('x'), KeyModifiers::CONTROL)),
            "control chords that are not editing keys must not be typed into the buffer"
        );
        assert!(!apply_edit_key(&mut buffer, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)));
        assert_eq!(buffer.text(), "Xab");
    }

    #[test]
    fn edits_multi_byte_characters_on_char_boundaries() {
        let mut buffer = EditBuffer::new("a界");
        assert!(buffer.move_left());
        buffer.insert_char('🙂');
        assert_eq!(buffer.text(), "a🙂界");
        assert!(buffer.backspace());
        assert_eq!(buffer.text(), "a界");
    }

    #[test]
    fn refuses_to_move_past_either_end() {
        let mut buffer = EditBuffer::new("ab");
        assert!(!buffer.move_right());
        assert!(!buffer.delete());
        buffer.set_cursor(0);
        assert!(!buffer.move_left());
        assert!(!buffer.backspace());
    }

    #[test]
    fn vertical_motion_preserves_the_character_column() {
        let mut buffer = EditBuffer::new("hello\n界\nworld");
        buffer.set_cursor(0);
        buffer.move_line_end();
        assert_eq!(buffer.cursor(), 5);

        assert!(buffer.move_down(), "onto the shorter middle line");
        assert_eq!(&buffer.text()[..buffer.cursor()], "hello\n界", "clamps to the end of a shorter line");

        assert!(buffer.move_down());
        assert!(!buffer.move_down(), "already on the last line");

        assert!(buffer.move_up());
        assert!(buffer.move_up());
        assert!(!buffer.move_up(), "already on the first line");
    }

    #[test]
    fn take_empties_the_buffer_and_resets_the_cursor() {
        let mut buffer = EditBuffer::new("draft");
        assert_eq!(buffer.take(), "draft");
        assert!(buffer.is_empty());
        assert_eq!(buffer.cursor(), 0);
    }

    #[test]
    fn replace_range_leaves_the_cursor_after_the_replacement() {
        let mut buffer = EditBuffer::new("say @fo here");
        buffer.replace_range(4..7, "@foo.rs ");
        assert_eq!(buffer.text(), "say @foo.rs  here");
        assert_eq!(buffer.cursor(), 12);
    }
}
