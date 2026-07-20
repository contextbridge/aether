use unicode_width::UnicodeWidthChar;

/// Multi-line prompt input with a byte-offset cursor.
#[derive(Debug, Default)]
pub struct Composer {
    text: String,
    cursor: usize,
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

    /// Take the current text for submission, leaving the composer empty.
    pub fn take_submission(&mut self) -> String {
        self.cursor = 0;
        std::mem::take(&mut self.text)
    }

    pub fn clear(&mut self) {
        self.text.clear();
        self.cursor = 0;
    }

    pub fn insert_char(&mut self, c: char) {
        self.text.insert(self.cursor, c);
        self.cursor += c.len_utf8();
    }

    pub fn insert_str(&mut self, s: &str) {
        self.text.insert_str(self.cursor, s);
        self.cursor += s.len();
    }

    pub fn insert_newline(&mut self) {
        self.insert_char('\n');
    }

    pub fn backspace(&mut self) {
        if let Some(c) = self.text[..self.cursor].chars().next_back() {
            self.cursor -= c.len_utf8();
            self.text.remove(self.cursor);
        }
    }

    pub fn delete(&mut self) {
        if self.cursor < self.text.len() {
            self.text.remove(self.cursor);
        }
    }

    pub fn move_left(&mut self) {
        if let Some(c) = self.text[..self.cursor].chars().next_back() {
            self.cursor -= c.len_utf8();
        }
    }

    pub fn move_right(&mut self) {
        if let Some(c) = self.text[self.cursor..].chars().next() {
            self.cursor += c.len_utf8();
        }
    }

    pub fn move_line_start(&mut self) {
        self.cursor = self.line_start();
    }

    pub fn move_line_end(&mut self) {
        self.cursor = self.text[self.cursor..].find('\n').map_or(self.text.len(), |offset| self.cursor + offset);
    }

    pub fn lines(&self) -> impl Iterator<Item = &str> {
        self.text.split('\n')
    }

    pub fn line_count(&self) -> usize {
        self.text.split('\n').count()
    }

    /// Cursor position as (row, display column) for terminal cursor placement.
    pub fn cursor_position(&self) -> (usize, usize) {
        let before = &self.text[..self.cursor];
        let row = before.matches('\n').count();
        let column = before[self.line_start()..].chars().map(|c| c.width().unwrap_or(0)).sum();
        (row, column)
    }

    fn line_start(&self) -> usize {
        self.text[..self.cursor].rfind('\n').map_or(0, |index| index + 1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn composer_with(text: &str) -> Composer {
        let mut composer = Composer::new();
        composer.insert_str(text);
        composer
    }

    #[test]
    fn take_submission_returns_text_and_resets() {
        let mut composer = composer_with("hello");

        assert_eq!(composer.take_submission(), "hello");
        assert!(composer.is_empty());
        assert_eq!(composer.cursor_position(), (0, 0));
    }

    #[test]
    fn backspace_handles_multibyte_chars() {
        let mut composer = composer_with("héllo");
        for _ in 0..4 {
            composer.backspace();
        }

        assert_eq!(composer.text(), "h");
    }

    #[test]
    fn cursor_moves_across_multibyte_boundaries() {
        let mut composer = composer_with("日本");
        composer.move_left();
        composer.insert_char('x');

        assert_eq!(composer.text(), "日x本");
    }

    #[test]
    fn cursor_position_tracks_rows_and_display_columns() {
        let mut composer = composer_with("first");
        composer.insert_newline();
        composer.insert_str("日本");

        assert_eq!(composer.cursor_position(), (1, 4));
        assert_eq!(composer.line_count(), 2);
    }

    #[test]
    fn home_and_end_move_within_current_line() {
        let mut composer = composer_with("one\ntwo");
        composer.move_line_start();
        assert_eq!(composer.cursor_position(), (1, 0));

        composer.move_line_end();
        composer.insert_char('!');
        assert_eq!(composer.text(), "one\ntwo!");
    }

    #[test]
    fn delete_removes_char_under_cursor() {
        let mut composer = composer_with("ab");
        composer.move_left();
        composer.move_left();
        composer.delete();

        assert_eq!(composer.text(), "b");
    }
}
