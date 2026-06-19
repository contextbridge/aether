use tui::{
    Combobox, Component, Event, Frame, Line, PickerMessage, Searchable, Style, ViewContext, display_width_text,
    pad_text_to_width, truncate_text,
};

#[derive(Debug, Clone)]
pub struct CommandEntry {
    pub name: String,
    pub description: String,
    pub has_input: bool,
    pub hint: Option<String>,
    pub builtin: bool,
}

impl Searchable for CommandEntry {
    fn search_text(&self) -> String {
        format!("{} {}", self.name, self.description)
    }
}

#[doc = include_str!("../docs/command_picker.md")]
pub struct CommandPicker {
    combobox: Combobox<CommandEntry>,
}

pub type CommandPickerMessage = PickerMessage<CommandEntry>;

impl CommandPicker {
    pub fn new(commands: Vec<CommandEntry>) -> Self {
        Self { combobox: Combobox::new(commands) }
    }

    #[cfg(test)]
    pub fn query(&self) -> &str {
        self.combobox.query()
    }
}

impl Component for CommandPicker {
    type Message = CommandPickerMessage;

    async fn on_event(&mut self, event: &Event) -> Option<Vec<Self::Message>> {
        self.combobox.handle_picker_event(event)
    }

    fn render(&mut self, context: &ViewContext) -> Frame {
        let mut lines = Vec::new();

        if self.combobox.is_empty() {
            lines.push(Line::new("  (no matching commands)".to_string()));
            return Frame::new(lines);
        }

        let max_name_width =
            self.combobox.matches().iter().map(|cmd| display_width_text(&format!("/{}", cmd.name))).max().unwrap_or(0);

        let item_lines = self.combobox.render_items(context, |command, is_selected, ctx| {
            let hint_suffix = match &command.hint {
                Some(hint) => format!("  [{hint}]"),
                None => String::new(),
            };

            let name_part = format!("/{}", command.name);
            let padded_name = pad_text_to_width(&name_part, max_name_width);
            let line_text = format!("{padded_name}  {}{}", command.description, hint_suffix);

            let max_width = ctx.size.width as usize;
            let truncated = truncate_text(&line_text, max_width);

            if is_selected {
                ctx.theme.selected_row_line(truncated)
            } else {
                build_styled_command_line(&truncated, padded_name.len(), ctx.theme.muted())
            }
        });
        lines.extend(item_lines);

        Frame::new(lines)
    }
}

fn build_styled_command_line(truncated: &str, name_byte_len: usize, muted: tui::Color) -> Line {
    if truncated.len() <= name_byte_len {
        Line::new(truncated)
    } else {
        let mut line = Line::new(&truncated[..name_byte_len]);
        line.push_with_style(&truncated[name_byte_len..], Style::fg(muted));
        line
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tui::test_picker::type_query;
    use tui::{KeyCode, KeyEvent, KeyModifiers};

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn sample_commands() -> Vec<CommandEntry> {
        vec![
            CommandEntry {
                name: "settings".into(),
                description: "Open settings".into(),
                has_input: false,
                hint: None,
                builtin: true,
            },
            CommandEntry {
                name: "search".into(),
                description: "Search code in the project".into(),
                has_input: true,
                hint: Some("query pattern".into()),
                builtin: false,
            },
            CommandEntry {
                name: "web".into(),
                description: "Browse the web".into(),
                has_input: true,
                hint: Some("url".into()),
                builtin: false,
            },
        ]
    }

    #[tokio::test]
    async fn handle_key_enter_returns_selected_command() {
        let mut picker = CommandPicker::new(sample_commands());

        let outcome = picker.on_event(&Event::Key(key(KeyCode::Enter))).await;

        assert!(outcome.is_some());
        assert!(matches!(outcome.unwrap().as_slice(), [PickerMessage::Confirm(_)]));
    }

    #[tokio::test]
    async fn handle_key_backspace_on_empty_query_requests_close() {
        let mut picker = CommandPicker::new(sample_commands());

        let outcome = picker.on_event(&Event::Key(key(KeyCode::Backspace))).await;

        assert!(outcome.is_some());

        assert!(matches!(outcome.unwrap().as_slice(), [PickerMessage::CloseAndPopChar]));
    }

    #[tokio::test]
    async fn handle_key_char_returns_char_typed() {
        let mut picker = CommandPicker::new(sample_commands());

        let outcome = picker.on_event(&Event::Key(key(KeyCode::Char('r')))).await;

        assert!(outcome.is_some());

        assert!(matches!(outcome.unwrap().as_slice(), [PickerMessage::CharTyped('r')]));
        assert_eq!(picker.query(), "r");
    }

    #[tokio::test]
    async fn handle_key_whitespace_closes_picker() {
        let mut picker = CommandPicker::new(sample_commands());

        let outcome = picker.on_event(&Event::Key(key(KeyCode::Char(' ')))).await;

        assert!(outcome.is_some());

        assert!(matches!(outcome.unwrap().as_slice(), [PickerMessage::CloseWithChar(' ')]));
    }

    #[tokio::test]
    async fn handle_key_backspace_with_query_returns_pop_char() {
        let mut picker = CommandPicker::new(sample_commands());
        type_query(&mut picker, "co").await;

        let outcome = picker.on_event(&Event::Key(key(KeyCode::Backspace))).await;

        assert!(outcome.is_some());

        assert!(matches!(outcome.unwrap().as_slice(), [PickerMessage::PopChar]));
        assert_eq!(picker.query(), "c");
    }

    #[tokio::test]
    async fn type_and_delete_updates_query() {
        let mut picker = CommandPicker::new(sample_commands());
        type_query(&mut picker, "co").await;
        assert_eq!(picker.query(), "co");

        picker.on_event(&Event::Key(key(KeyCode::Backspace))).await;
        assert_eq!(picker.query(), "c");

        picker.on_event(&Event::Key(key(KeyCode::Backspace))).await;
        assert_eq!(picker.query(), "");
    }
}
