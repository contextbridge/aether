use crate::keybindings::Keybindings;
use std::path::PathBuf;
use tui::{Component, Event, Frame, KeyCode, KeyEvent, KeyModifiers, Line, TextField, ViewContext};

#[doc = include_str!("../docs/text_input.md")]
pub struct TextInput {
    field: TextField,
    mentions: Vec<SelectedFileMention>,
    keybindings: Keybindings,
    history: PromptHistory,
}

pub enum TextInputMessage {
    Submit,
    OpenCommandPicker,
    OpenFilePicker,
}

#[derive(Debug, Clone)]
pub struct SelectedFileMention {
    pub mention: String,
    pub path: PathBuf,
    pub display_name: String,
}

const MAX_HISTORY_ENTRIES: usize = 500;

struct PromptHistory {
    prompts: Vec<String>,
    index: Option<usize>,
    draft: Option<String>,
}

impl PromptHistory {
    fn new() -> Self {
        Self { prompts: Vec::new(), index: None, draft: None }
    }

    fn record(&mut self, prompt: &str) {
        if prompt.is_empty() {
            return;
        }

        self.prompts.push(prompt.to_string());
        if self.prompts.len() > MAX_HISTORY_ENTRIES {
            self.prompts.remove(0);
        }
    }

    fn previous(&mut self, current_text: &str) -> Option<String> {
        if self.prompts.is_empty() {
            return None;
        }

        let next_index = match self.index {
            None => {
                self.draft = Some(current_text.to_string());
                self.prompts.len() - 1
            }
            Some(index) if index > 0 => index - 1,
            Some(_) => return None,
        };

        self.index = Some(next_index);
        Some(self.prompts[next_index].clone())
    }

    fn next(&mut self) -> Option<String> {
        let index = self.index?;

        if index + 1 < self.prompts.len() {
            let next_index = index + 1;
            self.index = Some(next_index);
            Some(self.prompts[next_index].clone())
        } else {
            let value = self.draft.take().unwrap_or_default();
            self.index = None;
            Some(value)
        }
    }

    fn reset(&mut self) {
        self.index = None;
        self.draft = None;
    }

    fn is_navigating(&self) -> bool {
        self.index.is_some()
    }
}

impl Default for TextInput {
    fn default() -> Self {
        Self::new(Keybindings::default())
    }
}

impl TextInput {
    pub fn new(keybindings: Keybindings) -> Self {
        Self { field: TextField::new(String::new()), mentions: Vec::new(), keybindings, history: PromptHistory::new() }
    }

    pub fn set_content_width(&mut self, width: usize) {
        self.field.set_content_width(width);
    }

    pub fn buffer(&self) -> &str {
        &self.field.value
    }

    /// Returns the visual cursor index, accounting for an active file picker
    /// whose query extends beyond the `@` trigger character.
    pub fn cursor_index(&self, picker_query_len: Option<usize>) -> usize {
        if let Some(query_len) = picker_query_len {
            let at_pos = self.active_mention_start().unwrap_or(self.field.value.len());
            at_pos + 1 + query_len
        } else {
            self.field.cursor_pos()
        }
    }

    #[cfg(test)]
    pub fn mentions(&self) -> &[SelectedFileMention] {
        &self.mentions
    }

    pub fn take_mentions(&mut self) -> Vec<SelectedFileMention> {
        std::mem::take(&mut self.mentions)
    }

    pub fn set_input(&mut self, s: String) {
        self.history.reset();
        self.field.set_value(s);
    }

    pub fn set_cursor_pos(&mut self, pos: usize) {
        self.field.set_cursor_pos(pos);
    }

    pub fn clear(&mut self) {
        self.history.reset();
        self.field.clear();
    }

    pub fn insert_char_at_cursor(&mut self, c: char) {
        self.history.reset();
        self.field.insert_at_cursor(c);
    }

    pub fn delete_char_before_cursor(&mut self) -> bool {
        self.history.reset();
        self.field.delete_before_cursor()
    }

    pub fn insert_paste(&mut self, text: &str) {
        self.history.reset();
        let filtered: String = text.chars().filter(|c| !c.is_control()).collect();
        self.field.insert_str_at_cursor(&filtered);
    }

    pub fn apply_file_selection(&mut self, path: PathBuf, display_name: String) {
        let mention = format!("@{display_name}");
        self.mentions.push(SelectedFileMention { mention: mention.clone(), path, display_name });

        if let Some(at_pos) = self.active_mention_start() {
            let mut s = self.field.value[..at_pos].to_string();
            s.push_str(&mention);
            s.push(' ');
            self.set_input(s);
        }
    }

    fn active_mention_start(&self) -> Option<usize> {
        mention_start(&self.field.value)
    }

    pub fn record_submission(&mut self, prompt: &str) {
        self.history.record(prompt);
    }

    fn recall_older(&mut self) -> bool {
        let Some(value) = self.history.previous(&self.field.value) else {
            return false;
        };
        self.mentions.clear();
        self.field.value = value;
        self.field.set_cursor_pos(0);
        true
    }

    fn recall_newer(&mut self) -> bool {
        let Some(value) = self.history.next() else {
            return false;
        };
        self.mentions.clear();
        self.field.value = value;
        self.field.set_cursor_pos(self.field.value.len());
        true
    }
}

impl Component for TextInput {
    type Message = TextInputMessage;

    async fn on_event(&mut self, event: &Event) -> Option<Vec<Self::Message>> {
        match event {
            Event::Paste(text) => {
                self.insert_paste(text);
                Some(vec![])
            }
            Event::Key(key_event) => self.handle_key(key_event).await,
            _ => None,
        }
    }

    fn render(&mut self, _context: &ViewContext) -> Frame {
        Frame::new(vec![Line::new(self.field.value.clone())])
    }
}

impl TextInput {
    async fn handle_key(&mut self, key_event: &KeyEvent) -> Option<Vec<TextInputMessage>> {
        if key_event.code == KeyCode::Enter && key_event.modifiers.contains(KeyModifiers::SHIFT) {
            self.history.reset();
            self.field.insert_at_cursor('\n');
            return Some(vec![]);
        }

        if self.keybindings.submit.matches(*key_event) {
            return Some(vec![TextInputMessage::Submit]);
        }

        if self.keybindings.open_command_picker.matches(*key_event) && self.field.value.is_empty() {
            self.history.reset();
            if let Some(c) = self.keybindings.open_command_picker.char() {
                self.field.insert_at_cursor(c);
            }
            return Some(vec![TextInputMessage::OpenCommandPicker]);
        }

        if self.keybindings.open_file_picker.matches(*key_event) {
            self.history.reset();
            if let Some(c) = self.keybindings.open_file_picker.char() {
                self.field.insert_at_cursor(c);
            }
            return Some(vec![TextInputMessage::OpenFilePicker]);
        }

        match key_event.code {
            KeyCode::Up if self.field.is_cursor_on_first_visual_line() && self.recall_older() => {
                return Some(vec![]);
            }
            KeyCode::Down if self.field.is_cursor_on_last_visual_line() && self.recall_newer() => {
                return Some(vec![]);
            }
            _ => {}
        }

        let before_len = self.field.value.len();
        let result = self.field.on_event(&Event::Key(*key_event)).await;
        if self.history.is_navigating() && self.field.value.len() != before_len {
            self.history.reset();
        }
        result.map(|_| vec![])
    }
}

fn mention_start(input: &str) -> Option<usize> {
    let at_pos = input.rfind('@')?;
    let prefix = &input[..at_pos];
    if prefix.is_empty() || prefix.chars().last().is_some_and(char::is_whitespace) { Some(at_pos) } else { None }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tui::KeyCode;
    use tui::KeyModifiers;

    fn key(code: KeyCode) -> Event {
        key_with_modifiers(code, KeyModifiers::NONE)
    }

    fn key_with_modifiers(code: KeyCode, modifiers: KeyModifiers) -> Event {
        Event::Key(KeyEvent::new(code, modifiers))
    }

    fn input_with(text: &str, cursor: Option<usize>) -> TextInput {
        let mut input = TextInput::default();
        input.set_input(text.to_string());
        if let Some(pos) = cursor {
            input.set_cursor_pos(pos);
        }
        input
    }

    fn input_with_width(text: &str, cursor: usize, width: usize) -> TextInput {
        let mut input = TextInput::default();
        input.set_content_width(width);
        input.set_input(text.to_string());
        input.set_cursor_pos(cursor);
        input
    }

    fn cursor(input: &TextInput) -> usize {
        input.cursor_index(None)
    }

    #[tokio::test]
    async fn arrow_key_cursor_movement() {
        // (initial_text, initial_cursor, key_code, expected_cursor)
        let cases = [
            ("hello", None, KeyCode::Left, 4, "left from end"),
            ("hello", Some(2), KeyCode::Right, 3, "right from middle"),
            ("hello", Some(0), KeyCode::Left, 0, "left at start stays"),
            ("hello", None, KeyCode::Right, 5, "right at end stays"),
            ("hello", Some(3), KeyCode::Home, 0, "home moves to start"),
            ("hello", Some(1), KeyCode::End, 5, "end moves to end"),
        ];
        for (text, cur, code, expected, label) in cases {
            let mut input = input_with(text, cur);
            input.on_event(&key(code)).await;
            assert_eq!(cursor(&input), expected, "{label}");
        }
    }

    #[tokio::test]
    async fn typing_inserts_at_cursor_position() {
        let mut input = input_with("hllo", Some(1));
        input.on_event(&key(KeyCode::Char('e'))).await;
        assert_eq!(input.buffer(), "hello");
        assert_eq!(cursor(&input), 2);
    }

    #[tokio::test]
    async fn backspace_at_cursor_middle_deletes_correct_char() {
        let mut input = input_with("hello", Some(3));
        input.on_event(&key(KeyCode::Backspace)).await;
        assert_eq!(input.buffer(), "helo");
        assert_eq!(cursor(&input), 2);
    }

    #[tokio::test]
    async fn backspace_at_start_does_nothing() {
        let mut input = input_with("hello", Some(0));
        let outcome = input.on_event(&key(KeyCode::Backspace)).await;
        assert!(outcome.is_some());
        assert_eq!(input.buffer(), "hello");
        assert_eq!(cursor(&input), 0);
    }

    #[tokio::test]
    async fn multibyte_utf8_cursor_navigation() {
        // "a中b" — 'a' is 1 byte, '中' is 3 bytes, 'b' is 1 byte = 5 bytes total
        let mut input = input_with("a中b", None);

        let steps: &[(KeyCode, usize)] = &[
            (KeyCode::Left, 4),  // before 'b'
            (KeyCode::Left, 1),  // before '中'
            (KeyCode::Left, 0),  // before 'a'
            (KeyCode::Right, 1), // after 'a'
            (KeyCode::Right, 4), // after '中'
        ];
        for (code, expected) in steps {
            input.on_event(&key(*code)).await;
            assert_eq!(cursor(&input), *expected);
        }
    }

    #[test]
    fn paste_inserts_at_cursor_position() {
        let mut input = input_with("hd", Some(1));
        input.insert_paste("ello worl");
        assert_eq!(input.buffer(), "hello world");
        assert_eq!(cursor(&input), 10);
    }

    #[tokio::test]
    async fn slash_on_empty_returns_open_command_picker() {
        let mut input = TextInput::default();
        let outcome = input.on_event(&key(KeyCode::Char('/'))).await;
        assert!(matches!(outcome.as_deref(), Some([TextInputMessage::OpenCommandPicker])));
        assert_eq!(input.buffer(), "/");
    }

    #[tokio::test]
    async fn at_sign_returns_open_file_picker() {
        let mut input = TextInput::default();
        let outcome = input.on_event(&key(KeyCode::Char('@'))).await;
        assert!(matches!(outcome.as_deref(), Some([TextInputMessage::OpenFilePicker])));
        assert_eq!(input.buffer(), "@");
    }

    #[tokio::test]
    async fn enter_returns_submit() {
        let mut input = input_with("hello", None);
        let outcome = input.on_event(&key(KeyCode::Enter)).await;
        assert!(matches!(outcome.as_deref(), Some([TextInputMessage::Submit])));
    }

    #[tokio::test]
    async fn shift_enter_inserts_newline() {
        let mut input = input_with("hello", None);
        let outcome = input.on_event(&key_with_modifiers(KeyCode::Enter, KeyModifiers::SHIFT)).await;

        assert!(matches!(outcome.as_deref(), Some([])));
        assert_eq!(input.buffer(), "hello\n");
        assert_eq!(cursor(&input), 6);
    }

    #[tokio::test]
    async fn shift_enter_inserts_newline_at_cursor() {
        let mut input = input_with("helloworld", Some(5));
        let outcome = input.on_event(&key_with_modifiers(KeyCode::Enter, KeyModifiers::SHIFT)).await;

        assert!(matches!(outcome.as_deref(), Some([])));
        assert_eq!(input.buffer(), "hello\nworld");
        assert_eq!(cursor(&input), 6);
    }

    #[test]
    fn file_selection_updates_mentions_and_buffer() {
        let mut input = input_with("@fo", None);
        input.apply_file_selection(PathBuf::from("foo.rs"), "foo.rs".to_string());
        assert_eq!(input.buffer(), "@foo.rs ");
        assert_eq!(input.mentions().len(), 1);
        assert_eq!(input.mentions()[0].mention, "@foo.rs");
    }

    #[test]
    fn cursor_index_with_and_without_picker() {
        let input = input_with("hello", Some(3));
        assert_eq!(input.cursor_index(None), 3);

        let input = input_with("@fo", None);
        // Picker has 2-char query ("fo"), @ is at position 0
        assert_eq!(input.cursor_index(Some(2)), 3); // 0 + 1 + 2
    }

    #[test]
    fn clear_resets_buffer_and_cursor() {
        let mut input = input_with("hello", None);
        input.clear();
        assert_eq!(input.buffer(), "");
        assert_eq!(cursor(&input), 0);
    }

    #[tokio::test]
    async fn vertical_cursor_movement_in_wrapped_text() {
        // "hello world" with width 5 → row 0: "hello", row 1: " worl", row 2: "d"
        // (cursor, key, expected, label)
        let cases = [
            (8, KeyCode::Up, 3, "up from row 1 col 3 -> row 0 col 3"),
            (3, KeyCode::Down, 8, "down from row 0 col 3 -> row 1 col 3"),
        ];
        for (cur, code, expected, label) in cases {
            let mut input = input_with_width("hello world", cur, 5);
            input.on_event(&key(code)).await;
            assert_eq!(cursor(&input), expected, "{label}");
        }
    }

    #[tokio::test]
    async fn up_on_first_row_goes_home_down_on_last_row_goes_end() {
        let cases =
            [(3, KeyCode::Up, 0, "up on single row -> home"), (0, KeyCode::Down, 5, "down on single row -> end")];
        for (cur, code, expected, label) in cases {
            let mut input = input_with_width("hello", cur, 20);
            input.on_event(&key(code)).await;
            assert_eq!(cursor(&input), expected, "{label}");
        }
    }

    fn input_with_history(history: &[&str]) -> TextInput {
        let mut input = TextInput::default();
        for entry in history {
            input.record_submission(entry);
        }
        input
    }

    #[tokio::test]
    async fn up_recalls_older_history_entry() {
        let mut input = input_with_history(&["first", "second", "third"]);
        assert_eq!(input.buffer(), "");

        input.on_event(&key(KeyCode::Up)).await;
        assert_eq!(input.buffer(), "third");
        assert_eq!(cursor(&input), 0);
    }

    #[tokio::test]
    async fn repeated_up_pages_to_older_entries() {
        let mut input = input_with_history(&["first", "second", "third"]);

        input.on_event(&key(KeyCode::Up)).await;
        assert_eq!(input.buffer(), "third");

        input.on_event(&key(KeyCode::Up)).await;
        assert_eq!(input.buffer(), "second");

        input.on_event(&key(KeyCode::Up)).await;
        assert_eq!(input.buffer(), "first");
    }

    #[tokio::test]
    async fn up_stops_at_oldest_entry() {
        let mut input = input_with_history(&["first", "second"]);

        input.on_event(&key(KeyCode::Up)).await;
        assert_eq!(input.buffer(), "second");

        input.on_event(&key(KeyCode::Up)).await;
        assert_eq!(input.buffer(), "first");

        let before = input.buffer().to_string();
        input.on_event(&key(KeyCode::Up)).await;
        assert_eq!(input.buffer(), before);
    }

    #[tokio::test]
    async fn down_recalls_newer_entry() {
        let mut input = input_with_history(&["first", "second", "third"]);

        input.on_event(&key(KeyCode::Up)).await;
        input.on_event(&key(KeyCode::Up)).await;
        input.on_event(&key(KeyCode::Up)).await;
        assert_eq!(input.buffer(), "first");

        input.on_event(&key(KeyCode::Down)).await;
        assert_eq!(input.buffer(), "second");
        assert_eq!(cursor(&input), 6);
    }

    #[tokio::test]
    async fn down_past_newest_restores_draft() {
        let mut input = input_with_history(&["first", "second"]);
        input.set_input("my draft".to_string());
        input.on_event(&key(KeyCode::Up)).await;
        assert_eq!(input.buffer(), "second");

        input.on_event(&key(KeyCode::Down)).await;
        assert_eq!(input.buffer(), "my draft");
        assert_eq!(cursor(&input), 8);
    }

    #[tokio::test]
    async fn down_on_live_prompt_does_nothing() {
        let mut input = input_with_history(&["first"]);
        let outcome = input.on_event(&key(KeyCode::Down)).await;
        assert_eq!(input.buffer(), "");
        assert!(outcome.is_some());
    }

    #[tokio::test]
    async fn empty_history_up_does_nothing_special() {
        let mut input = TextInput::default();
        input.on_event(&key(KeyCode::Up)).await;
        assert_eq!(input.buffer(), "");
        assert_eq!(cursor(&input), 0);
    }

    #[tokio::test]
    async fn typing_resets_history_navigation() {
        let mut input = input_with_history(&["first", "second"]);
        input.on_event(&key(KeyCode::Up)).await;
        assert_eq!(input.buffer(), "second");

        input.on_event(&key(KeyCode::Char('x'))).await;
        assert_eq!(input.buffer(), "xsecond");

        input.on_event(&key(KeyCode::Down)).await;
        assert_eq!(cursor(&input), 7);
    }

    #[tokio::test]
    async fn up_on_first_row_of_wrapped_text_navigates_history() {
        let mut input = TextInput::default();
        input.set_content_width(5);
        input.record_submission("old entry");
        input.set_input("hello world".to_string());
        input.set_cursor_pos(3); // on row 0
        input.on_event(&key(KeyCode::Up)).await;
        assert_eq!(input.buffer(), "old entry");
    }

    #[tokio::test]
    async fn up_on_middle_row_of_wrapped_text_keeps_normal_navigation() {
        let mut input = TextInput::default();
        input.set_content_width(5);
        input.record_submission("old entry");
        input.set_input("hello world".to_string());
        input.set_cursor_pos(8); // on row 1
        input.on_event(&key(KeyCode::Up)).await;

        assert_eq!(input.buffer(), "hello world");
        assert_eq!(cursor(&input), 3); // moved up to row 0, same column
    }

    #[tokio::test]
    async fn down_on_last_row_of_recalled_single_line_navigates_history() {
        let mut input = TextInput::default();
        input.set_content_width(20);
        input.record_submission("old entry");
        input.record_submission("newer entry");
        input.set_input("current draft".to_string());

        input.on_event(&key(KeyCode::Up)).await;
        assert_eq!(input.buffer(), "newer entry");

        input.on_event(&key(KeyCode::Up)).await;
        assert_eq!(input.buffer(), "old entry");

        input.on_event(&key(KeyCode::Down)).await;
        assert_eq!(input.buffer(), "newer entry");

        input.on_event(&key(KeyCode::Down)).await;
        assert_eq!(input.buffer(), "current draft");
    }

    #[tokio::test]
    async fn down_on_non_last_row_of_wrapped_text_keeps_normal_navigation() {
        let mut input = TextInput::default();
        input.set_content_width(5);
        input.set_input("hello world".to_string());
        input.set_cursor_pos(3); // on row 0
        input.on_event(&key(KeyCode::Down)).await;

        assert_eq!(input.buffer(), "hello world");
        assert_eq!(cursor(&input), 8); // moved down to row 1
    }

    #[tokio::test]
    async fn recalling_history_clears_mentions() {
        let mut input = TextInput::default();
        input.apply_file_selection(PathBuf::from("foo.rs"), "foo.rs".to_string());
        assert_eq!(input.mentions().len(), 1);

        input.record_submission("some prompt");
        input.set_input(String::new());

        input.on_event(&key(KeyCode::Up)).await;
        assert_eq!(input.mentions().len(), 0);
        assert_eq!(input.buffer(), "some prompt");
    }

    #[test]
    fn record_submission_adds_to_history() {
        let mut input = TextInput::default();
        input.record_submission("first");
        input.record_submission("second");
        input.record_submission("third");
        assert_eq!(input.history.prompts, vec!["first", "second", "third"]);
    }
}
