use crossterm::event::{KeyCode, KeyModifiers};

use crate::components::{Component, Event, ViewContext};
use crate::line::Line;
use crate::rendering::frame::Frame;
use crate::rendering::soft_wrap::{soft_wrap_text_byte_offset, soft_wrap_text_position};

/// Single-line text input with cursor tracking and navigation.
pub struct TextField {
    pub value: String,
    cursor_pos: usize,
    content_width: usize,
}

impl TextField {
    pub fn new(value: String) -> Self {
        let cursor_pos = value.len();
        Self { value, cursor_pos, content_width: usize::MAX }
    }

    pub fn set_content_width(&mut self, width: usize) {
        self.content_width = width.max(1);
    }

    pub fn cursor_pos(&self) -> usize {
        self.cursor_pos
    }

    pub fn set_cursor_pos(&mut self, pos: usize) {
        self.cursor_pos = pos.min(self.value.len());
    }

    pub fn insert_at_cursor(&mut self, c: char) {
        self.value.insert(self.cursor_pos, c);
        self.cursor_pos += c.len_utf8();
    }

    pub fn insert_str_at_cursor(&mut self, s: &str) {
        self.value.insert_str(self.cursor_pos, s);
        self.cursor_pos += s.len();
    }

    pub fn delete_before_cursor(&mut self) -> bool {
        let Some((prev, _)) = self.value[..self.cursor_pos].char_indices().next_back() else {
            return false;
        };
        self.value.drain(prev..self.cursor_pos);
        self.cursor_pos = prev;
        true
    }

    pub fn set_value(&mut self, value: String) {
        self.cursor_pos = value.len();
        self.value = value;
    }

    pub fn clear(&mut self) {
        self.value.clear();
        self.cursor_pos = 0;
    }

    pub fn to_json(&self) -> serde_json::Value {
        serde_json::Value::String(self.value.clone())
    }

    pub fn render_field(&self, context: &ViewContext, focused: bool) -> Vec<Line> {
        let mut line = Line::new(&self.value);
        if focused {
            line.push_styled("▏", context.theme.primary());
        }
        vec![line]
    }

    fn delete_after_cursor(&mut self) {
        if let Some(c) = self.value[self.cursor_pos..].chars().next() {
            self.value.drain(self.cursor_pos..self.cursor_pos + c.len_utf8());
        }
    }

    fn delete_word_backward(&mut self) {
        let end = self.cursor_pos;
        let start = self.word_start_backward();
        self.cursor_pos = start;
        self.value.drain(start..end);
    }

    fn word_end_forward(&mut self) {
        let len = self.value.len();
        while self.cursor_pos < len {
            let ch = self.value[self.cursor_pos..].chars().next().unwrap();
            if ch.is_whitespace() {
                break;
            }
            self.cursor_pos += ch.len_utf8();
        }
        while self.cursor_pos < len {
            let ch = self.value[self.cursor_pos..].chars().next().unwrap();
            if !ch.is_whitespace() {
                break;
            }
            self.cursor_pos += ch.len_utf8();
        }
    }

    fn move_cursor_up(&mut self, content_width: usize) {
        if content_width == 0 {
            return;
        }
        let (row, col) = self.visual_position(self.cursor_pos, content_width);
        if row == 0 {
            self.cursor_pos = 0;
        } else {
            self.cursor_pos = self.byte_pos_at_visual(row - 1, col, content_width);
        }
    }

    fn move_cursor_down(&mut self, content_width: usize) {
        if content_width == 0 {
            return;
        }
        let (row, col) = self.visual_position(self.cursor_pos, content_width);
        let max_row = self.visual_position(self.value.len(), content_width).0;
        if row >= max_row {
            self.cursor_pos = self.value.len();
        } else {
            self.cursor_pos = self.byte_pos_at_visual(row + 1, col, content_width);
        }
    }

    fn word_start_backward(&self) -> usize {
        let mut pos = self.cursor_pos;
        while pos > 0 {
            let (i, ch) = self.value[..pos].char_indices().next_back().unwrap();
            if !ch.is_whitespace() {
                break;
            }
            pos = i;
        }
        while pos > 0 {
            let (i, ch) = self.value[..pos].char_indices().next_back().unwrap();
            if ch.is_whitespace() {
                break;
            }
            pos = i;
        }
        pos
    }

    fn hard_line_start(&self) -> usize {
        self.value[..self.cursor_pos].rfind('\n').map_or(0, |pos| pos + '\n'.len_utf8())
    }

    fn hard_line_end(&self) -> usize {
        self.value[self.cursor_pos..].find('\n').map_or(self.value.len(), |pos| self.cursor_pos + pos)
    }

    pub fn is_cursor_on_first_visual_line(&self) -> bool {
        self.visual_position(self.cursor_pos, self.content_width).0 == 0
    }

    pub fn is_cursor_on_last_visual_line(&self) -> bool {
        let cursor_row = self.visual_position(self.cursor_pos, self.content_width).0;
        let max_row = self.visual_position(self.value.len(), self.content_width).0;
        cursor_row >= max_row
    }

    fn visual_position(&self, byte_pos: usize, content_width: usize) -> (usize, usize) {
        soft_wrap_text_position(&self.value, byte_pos, content_width)
    }

    fn byte_pos_at_visual(&self, target_row: usize, target_col: usize, content_width: usize) -> usize {
        soft_wrap_text_byte_offset(&self.value, target_row, target_col, content_width)
    }
}

impl Component for TextField {
    type Message = ();

    async fn on_event(&mut self, event: &Event) -> Option<Vec<Self::Message>> {
        match event {
            Event::Key(key) => {
                let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
                let alt = key.modifiers.contains(KeyModifiers::ALT);
                match key.code {
                    KeyCode::Char('a') if ctrl => {
                        self.cursor_pos = self.hard_line_start();
                        Some(vec![])
                    }
                    KeyCode::Char('e') if ctrl => {
                        self.cursor_pos = self.hard_line_end();
                        Some(vec![])
                    }
                    KeyCode::Char('w') if ctrl => {
                        self.delete_word_backward();
                        Some(vec![])
                    }
                    KeyCode::Char('u') if ctrl => {
                        self.value.drain(..self.cursor_pos);
                        self.cursor_pos = 0;
                        Some(vec![])
                    }
                    KeyCode::Char('k') if ctrl => {
                        self.value.truncate(self.cursor_pos);
                        Some(vec![])
                    }
                    KeyCode::Backspace if alt => {
                        self.delete_word_backward();
                        Some(vec![])
                    }
                    KeyCode::Left if alt || ctrl => {
                        self.cursor_pos = self.word_start_backward();
                        Some(vec![])
                    }
                    KeyCode::Char('b') if alt => {
                        self.cursor_pos = self.word_start_backward();
                        Some(vec![])
                    }
                    KeyCode::Right if alt || ctrl => {
                        self.word_end_forward();
                        Some(vec![])
                    }
                    KeyCode::Char('f') if alt => {
                        self.word_end_forward();
                        Some(vec![])
                    }
                    KeyCode::Delete => {
                        self.delete_after_cursor();
                        Some(vec![])
                    }
                    KeyCode::Char(c) if !ctrl => {
                        self.insert_at_cursor(c);
                        Some(vec![])
                    }
                    KeyCode::Backspace => {
                        self.delete_before_cursor();
                        Some(vec![])
                    }
                    KeyCode::Left => {
                        self.cursor_pos =
                            self.value[..self.cursor_pos].char_indices().next_back().map_or(0, |(i, _)| i);
                        Some(vec![])
                    }
                    KeyCode::Right => {
                        if let Some(c) = self.value[self.cursor_pos..].chars().next() {
                            self.cursor_pos += c.len_utf8();
                        }
                        Some(vec![])
                    }
                    KeyCode::Home => {
                        self.cursor_pos = 0;
                        Some(vec![])
                    }
                    KeyCode::End => {
                        self.cursor_pos = self.value.len();
                        Some(vec![])
                    }
                    KeyCode::Up => {
                        self.move_cursor_up(self.content_width);
                        Some(vec![])
                    }
                    KeyCode::Down => {
                        self.move_cursor_down(self.content_width);
                        Some(vec![])
                    }
                    _ => None,
                }
            }
            Event::Paste(text) => {
                self.insert_str_at_cursor(text);
                Some(vec![])
            }
            _ => None,
        }
    }

    fn render(&mut self, context: &ViewContext) -> Frame {
        Frame::new(self.render_field(context, true))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyEvent, KeyModifiers};

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }
    fn ctrl(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::CONTROL)
    }
    fn alt(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::ALT)
    }
    fn field(text: &str) -> TextField {
        TextField::new(text.to_string())
    }
    fn field_at(text: &str, cursor: usize) -> TextField {
        let mut f = field(text);
        f.set_cursor_pos(cursor);
        f
    }

    async fn send(f: &mut TextField, evt: Event) -> Option<Vec<()>> {
        f.on_event(&evt).await
    }
    async fn send_key(f: &mut TextField, k: KeyEvent) -> Option<Vec<()>> {
        send(f, Event::Key(k)).await
    }

    /// Assert both value and cursor position.
    fn assert_state(f: &TextField, value: &str, cursor: usize) {
        assert_eq!(f.value, value, "value mismatch");
        assert_eq!(f.cursor_pos(), cursor, "cursor mismatch");
    }

    #[tokio::test]
    async fn typing_appends_characters() {
        let mut f = field("");
        send_key(&mut f, key(KeyCode::Char('h'))).await;
        send_key(&mut f, key(KeyCode::Char('i'))).await;
        assert_eq!(f.value, "hi");
    }

    #[tokio::test]
    async fn backspace_variants() {
        // End of string
        let mut f = field("abc");
        send_key(&mut f, key(KeyCode::Backspace)).await;
        assert_eq!(f.value, "ab");

        // Empty string (no-op)
        let mut f = field("");
        send_key(&mut f, key(KeyCode::Backspace)).await;
        assert_eq!(f.value, "");

        // Middle of string
        let mut f = field_at("hello", 3);
        send_key(&mut f, key(KeyCode::Backspace)).await;
        assert_state(&f, "helo", 2);
    }

    #[test]
    fn to_json_returns_string_value() {
        assert_eq!(field("hello").to_json(), serde_json::json!("hello"));
    }

    #[tokio::test]
    async fn unhandled_keys_are_ignored() {
        let mut f = field("");
        assert!(send_key(&mut f, key(KeyCode::F(1))).await.is_none());
    }

    #[tokio::test]
    async fn paste_variants() {
        // Into empty field
        let mut f = field("");
        let outcome = send(&mut f, Event::Paste("hello".into())).await;
        assert!(outcome.is_some());
        assert_eq!(f.value, "hello");

        // At cursor position
        let mut f = field_at("hd", 1);
        send(&mut f, Event::Paste("ello worl".into())).await;
        assert_state(&f, "hello world", 10);
    }

    #[test]
    fn cursor_starts_at_end() {
        assert_eq!(field("hello").cursor_pos(), 5);
    }

    #[tokio::test]
    async fn cursor_movement_single_keys() {
        // (initial_text, initial_cursor, key_event, expected_cursor)
        let cases: Vec<(&str, Option<usize>, KeyEvent, usize)> = vec![
            ("hello", None, key(KeyCode::Left), 4),
            ("hello", None, key(KeyCode::Right), 5),
            ("", None, key(KeyCode::Left), 0),
            ("hello", None, key(KeyCode::Home), 0),
            ("hello", Some(0), key(KeyCode::End), 5),
            ("hello", None, ctrl(KeyCode::Char('a')), 0),
            ("hello", Some(0), ctrl(KeyCode::Char('e')), 5),
        ];
        for (text, cursor, k, expected) in cases {
            let mut f = cursor.map_or_else(|| field(text), |c| field_at(text, c));
            send_key(&mut f, k).await;
            assert_eq!(f.cursor_pos(), expected, "failed for key {k:?} on {text:?}");
        }
    }

    #[tokio::test]
    async fn insert_at_middle() {
        let mut f = field_at("hllo", 1);
        send_key(&mut f, key(KeyCode::Char('e'))).await;
        assert_state(&f, "hello", 2);
    }

    #[tokio::test]
    async fn multibyte_utf8_navigation() {
        let mut f = field("a中b");
        assert_eq!(f.cursor_pos(), 5);
        for expected in [4, 1, 0] {
            send_key(&mut f, key(KeyCode::Left)).await;
            assert_eq!(f.cursor_pos(), expected);
        }
        for expected in [1, 4] {
            send_key(&mut f, key(KeyCode::Right)).await;
            assert_eq!(f.cursor_pos(), expected);
        }
    }

    #[test]
    fn set_value_moves_cursor_to_end() {
        let mut f = field("");
        f.set_value("hello".to_string());
        assert_state(&f, "hello", 5);
    }

    #[test]
    fn clear_resets_cursor() {
        let mut f = field("hello");
        f.clear();
        assert_state(&f, "", 0);
    }

    #[tokio::test]
    async fn delete_forward_variants() {
        // Middle of string
        let mut f = field_at("hello", 2);
        send_key(&mut f, key(KeyCode::Delete)).await;
        assert_state(&f, "helo", 2);

        // At end (no-op)
        let mut f = field("hello");
        send_key(&mut f, key(KeyCode::Delete)).await;
        assert_eq!(f.value, "hello");

        // Multibyte character
        let mut f = field_at("a中b", 1);
        send_key(&mut f, key(KeyCode::Delete)).await;
        assert_state(&f, "ab", 1);
    }

    #[tokio::test]
    async fn ctrl_w_variants() {
        // (text, cursor, expected_value, expected_cursor)
        let cases: Vec<(&str, Option<usize>, &str, usize)> = vec![
            ("hello world", None, "hello ", 6),
            ("hello   ", None, "", 0),
            ("hello", Some(0), "hello", 0),
            ("hello world", Some(8), "hello rld", 6),
            ("", None, "", 0),
        ];
        for (text, cursor, exp_val, exp_cur) in cases {
            let mut f = cursor.map_or_else(|| field(text), |c| field_at(text, c));
            send_key(&mut f, ctrl(KeyCode::Char('w'))).await;
            assert_state(&f, exp_val, exp_cur);
        }
    }

    #[tokio::test]
    async fn alt_backspace_deletes_word() {
        let mut f = field("hello world");
        send_key(&mut f, alt(KeyCode::Backspace)).await;
        assert_eq!(f.value, "hello ");
    }

    #[tokio::test]
    async fn ctrl_u_variants() {
        let mut f = field_at("hello world", 5);
        send_key(&mut f, ctrl(KeyCode::Char('u'))).await;
        assert_state(&f, " world", 0);

        // At start (no-op)
        let mut f = field_at("hello", 0);
        send_key(&mut f, ctrl(KeyCode::Char('u'))).await;
        assert_state(&f, "hello", 0);
    }

    #[tokio::test]
    async fn ctrl_k_variants() {
        let mut f = field_at("hello world", 5);
        send_key(&mut f, ctrl(KeyCode::Char('k'))).await;
        assert_state(&f, "hello", 5);

        // At end (no-op)
        let mut f = field("hello");
        send_key(&mut f, ctrl(KeyCode::Char('k'))).await;
        assert_eq!(f.value, "hello");
    }

    #[tokio::test]
    async fn word_navigation() {
        // (text, cursor, key_event, expected_cursor)
        let cases: Vec<(&str, Option<usize>, KeyEvent, usize)> = vec![
            ("hello world", None, alt(KeyCode::Left), 6),
            ("hello world", Some(8), alt(KeyCode::Left), 6),
            ("hello", Some(0), alt(KeyCode::Left), 0),
            ("hello world", None, ctrl(KeyCode::Left), 6),
            ("hello world", Some(0), alt(KeyCode::Right), 6),
            ("hello", None, alt(KeyCode::Right), 5),
            ("a中 b", Some(0), alt(KeyCode::Right), 5),
            ("hello world", Some(0), ctrl(KeyCode::Right), 6),
        ];
        for (text, cursor, k, expected) in cases {
            let mut f = cursor.map_or_else(|| field(text), |c| field_at(text, c));
            send_key(&mut f, k).await;
            assert_eq!(f.cursor_pos(), expected, "failed for {k:?} on {text:?} at {cursor:?}");
        }
    }

    #[test]
    fn move_cursor_up_cases() {
        // (text, cursor, width, expected)
        let cases: Vec<(&str, Option<usize>, usize, usize)> =
            vec![("hello world", Some(3), 10, 0), ("hello world", Some(8), 5, 2), ("hello", Some(3), 0, 3)];
        for (text, cursor, width, expected) in cases {
            let mut f = cursor.map_or_else(|| field(text), |c| field_at(text, c));
            f.move_cursor_up(width);
            assert_eq!(f.cursor_pos(), expected, "up failed: {text:?} cursor={cursor:?} w={width}");
        }
    }

    #[test]
    fn move_cursor_up_wide_chars() {
        let mut f = field("中中中中中");
        f.move_cursor_up(5);
        assert_eq!(f.cursor_pos(), 9);
    }

    #[test]
    fn move_cursor_down_cases() {
        // (text, cursor, width, expected)
        let cases: Vec<(&str, Option<usize>, usize, usize)> = vec![
            ("hello world", Some(0), 20, 11),
            ("hello world", Some(3), 5, 9),
            ("hello world", Some(8), 5, 11),
            ("", None, 10, 0),
        ];
        for (text, cursor, width, expected) in cases {
            let mut f = cursor.map_or_else(|| field(text), |c| field_at(text, c));
            f.move_cursor_down(width);
            assert_eq!(f.cursor_pos(), expected, "down failed: {text:?} cursor={cursor:?} w={width}");
        }
    }

    #[test]
    fn is_cursor_on_first_visual_line() {
        let mut f = field_at("hello world", 3);
        f.set_content_width(5);
        assert!(f.is_cursor_on_first_visual_line());

        let mut f = field_at("hello world", 8);
        f.set_content_width(5);
        assert!(!f.is_cursor_on_first_visual_line());
    }

    #[test]
    fn is_cursor_on_last_visual_line() {
        let mut f = field_at("hello world", 11);
        f.set_content_width(5);
        assert!(f.is_cursor_on_last_visual_line());

        let mut f = field_at("hello world", 3);
        f.set_content_width(5);
        assert!(!f.is_cursor_on_last_visual_line());

        let mut f = field_at("hello world", 8);
        f.set_content_width(5);
        assert!(f.is_cursor_on_last_visual_line());
    }

    #[test]
    fn single_line_is_both_first_and_last() {
        let mut f = field_at("hello", 3);
        f.set_content_width(20);
        assert!(f.is_cursor_on_first_visual_line());
        assert!(f.is_cursor_on_last_visual_line());
    }

    #[test]
    fn empty_field_is_both_first_and_last() {
        let mut f = field("");
        f.set_content_width(20);
        assert!(f.is_cursor_on_first_visual_line());
        assert!(f.is_cursor_on_last_visual_line());
    }
}
