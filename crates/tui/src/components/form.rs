use crate::components::{Component, Event, ViewContext};
use crate::focus::FocusRing;
use crate::line::Line;
use crate::rendering::frame::Frame;
use crate::style::Style;

use super::checkbox::Checkbox;
use super::multi_select::MultiSelect;
use super::number_field::NumberField;
use super::radio_select::RadioSelect;
use super::text_field::TextField;
use crossterm::event::KeyCode;

/// Messages emitted by [`Form`] input handling.
pub enum FormMessage {
    Close,
    Submit,
}

enum FormMode {
    SingleStep,
    MultiStep,
}

impl FormMode {
    fn from_field_count(count: usize) -> Self {
        if count <= 1 { Self::SingleStep } else { Self::MultiStep }
    }
}

#[doc = include_str!("../docs/form.md")]
pub struct Form {
    pub message: String,
    pub fields: Vec<FormField>,
    mode: FormMode,
    focus: FocusRing,
}

/// A single field within a [`Form`].
pub struct FormField {
    pub name: String,
    pub label: String,
    pub description: Option<String>,
    pub required: bool,
    pub kind: FormFieldKind,
}

/// The widget type backing a [`FormField`].
pub enum FormFieldKind {
    Text(TextField),
    Number(NumberField),
    Boolean(Checkbox),
    SingleSelect(RadioSelect),
    MultiSelect(MultiSelect),
}

impl Form {
    pub fn new(message: String, fields: Vec<FormField>) -> Self {
        let mode = FormMode::from_field_count(fields.len());
        let focus_len = match mode {
            FormMode::SingleStep => fields.len(),
            FormMode::MultiStep => fields.len() + 1,
        };
        Self { message, fields, mode, focus: FocusRing::new(focus_len) }
    }

    pub fn to_json(&self) -> serde_json::Value {
        let mut map = serde_json::Map::new();
        for field in &self.fields {
            map.insert(field.name.clone(), field.kind.to_json());
        }
        serde_json::Value::Object(map)
    }

    fn is_single_step(&self) -> bool {
        matches!(self.mode, FormMode::SingleStep)
    }

    fn is_multi_step(&self) -> bool {
        matches!(self.mode, FormMode::MultiStep)
    }

    fn is_on_submit_tab(&self) -> bool {
        self.is_multi_step() && self.focus.focused() == self.fields.len()
    }

    fn active_field_uses_horizontal_arrows(&self) -> bool {
        self.fields
            .get(self.focus.focused())
            .is_some_and(|f| matches!(f.kind, FormFieldKind::Text(_) | FormFieldKind::Number(_)))
    }

    fn render_tab_bar(&self, context: &ViewContext) -> Line {
        let mut line = Line::default();
        let muted = context.theme.text_secondary();
        let primary = context.theme.primary();
        let success = context.theme.success();

        for (i, field) in self.fields.iter().enumerate() {
            if i > 0 {
                line.push_styled(" · ", muted);
            }

            let is_active = self.focus.is_focused(i);
            let indicator = if field.kind.is_answered() { "✓ " } else { "□ " };

            let style = if is_active { Style::fg(primary).bold() } else { Style::fg(muted) };
            line.push_with_style(format!("{indicator}{}", field.label), style);
        }

        // Submit tab
        if !self.fields.is_empty() {
            line.push_styled(" · ", muted);
        }
        let submit_style = if self.is_on_submit_tab() { Style::fg(success).bold() } else { Style::fg(muted) };
        line.push_with_style("Submit", submit_style);

        line
    }

    fn render_active_field(&self, context: &ViewContext) -> Vec<Line> {
        if self.is_on_submit_tab() {
            return self.render_submit_summary(context);
        }

        let Some(field) = self.fields.get(self.focus.focused()) else {
            return vec![];
        };

        let mut lines = Vec::new();
        let required_marker = if field.required { "*" } else { "" };
        let label_line = Line::with_style(
            format!("{}{required_marker}: ", field.label),
            Style::fg(context.theme.text_primary()).bold(),
        );

        let field_lines = field.kind.render_field(context, true);
        let inline = field.kind.is_inline();
        if inline {
            let mut combined = label_line;
            if let Some((first, rest)) = field_lines.split_first() {
                combined.append_line(first);
                lines.push(combined);
                lines.extend_from_slice(rest);
            } else {
                lines.push(combined);
            }
        } else {
            // Multi-line widgets (radio, multi-select): label on its own line.
            lines.push(label_line);
            lines.extend(field_lines);
        }

        if let Some(desc) = &field.description {
            lines.push(Line::styled(desc, context.theme.muted()));
        }

        lines
    }

    fn render_submit_summary(&self, context: &ViewContext) -> Vec<Line> {
        let mut lines = vec![Line::with_style("Review & Submit", Style::fg(context.theme.text_primary()).bold())];
        lines.push(Line::default());

        for field in &self.fields {
            let mut line = Line::with_style(format!("{}: ", field.label), Style::fg(context.theme.text_secondary()));
            let value_lines = field.kind.render_field(context, false);
            if let Some(first) = value_lines.first() {
                line.append_line(first);
            }
            lines.push(line);
        }

        lines
    }

    fn render_footer(&self, context: &ViewContext) -> Line {
        let muted = context.theme.muted();

        if self.is_multi_step() && self.is_on_submit_tab() {
            return Line::styled("Enter to submit · Esc to cancel", muted);
        }

        let Some(field) = self.fields.get(self.focus.focused()) else {
            return if self.is_single_step() {
                Line::styled("Enter to submit · Esc to cancel", muted)
            } else {
                Line::default()
            };
        };

        let hints = if self.is_single_step() {
            match &field.kind {
                FormFieldKind::Text(_) | FormFieldKind::Number(_) => {
                    "Type your answer · Enter to submit · Esc to cancel"
                }
                FormFieldKind::Boolean(_) => "Space to toggle · Enter to submit · Esc to cancel",
                FormFieldKind::SingleSelect(_) => "↑↓ to select · Enter to submit · Esc to cancel",
                FormFieldKind::MultiSelect(_) => "Space to toggle · ↑↓ to move · Enter to submit · Esc to cancel",
            }
        } else {
            match &field.kind {
                FormFieldKind::Text(_) | FormFieldKind::Number(_) => {
                    "Type your answer · Tab to navigate · Esc to cancel"
                }
                FormFieldKind::Boolean(_) => "Space to toggle · Tab to navigate · Esc to cancel",
                FormFieldKind::SingleSelect(_) => "↑↓ to select · Tab to navigate · Enter to confirm · Esc to cancel",
                FormFieldKind::MultiSelect(_) => "Space to toggle · ↑↓ to move · Tab to navigate · Esc to cancel",
            }
        };

        Line::styled(hints, muted)
    }
}

impl FormFieldKind {
    /// Returns `true` for widgets that render on a single line (text, number, checkbox)
    /// and `false` for multi-line widgets (radio select, multi-select).
    pub fn is_inline(&self) -> bool {
        matches!(self, Self::Text(_) | Self::Number(_) | Self::Boolean(_))
    }

    pub fn is_answered(&self) -> bool {
        match self {
            Self::Text(w) => !w.value.is_empty(),
            Self::Number(w) => !w.value.is_empty(),
            Self::Boolean(_) | Self::SingleSelect(_) => true,
            Self::MultiSelect(w) => w.selected.iter().any(|&s| s),
        }
    }

    fn to_json(&self) -> serde_json::Value {
        match self {
            Self::Text(w) => w.to_json(),
            Self::Number(w) => w.to_json(),
            Self::Boolean(w) => w.to_json(),
            Self::SingleSelect(w) => w.to_json(),
            Self::MultiSelect(w) => w.to_json(),
        }
    }

    fn render_field(&self, context: &ViewContext, focused: bool) -> Vec<Line> {
        match self {
            Self::Text(w) => w.render_field(context, focused),
            Self::Number(w) => w.render_field(context, focused),
            Self::Boolean(w) => w.render_field(context, focused),
            Self::SingleSelect(w) => w.render_field(context, focused),
            Self::MultiSelect(w) => w.render_field(context, focused),
        }
    }

    async fn handle_event(&mut self, event: &Event) -> Option<Vec<()>> {
        match self {
            Self::Text(w) => w.on_event(event).await,
            Self::Number(w) => w.on_event(event).await,
            Self::Boolean(w) => w.on_event(event).await,
            Self::SingleSelect(w) => w.on_event(event).await,
            Self::MultiSelect(w) => w.on_event(event).await,
        }
    }
}

impl Component for Form {
    type Message = FormMessage;

    async fn on_event(&mut self, event: &Event) -> Option<Vec<Self::Message>> {
        let Event::Key(key) = event else {
            return None;
        };
        match key.code {
            KeyCode::Esc => return Some(vec![FormMessage::Close]),
            KeyCode::Enter if self.is_single_step() => return Some(vec![FormMessage::Submit]),
            KeyCode::Enter if self.is_on_submit_tab() => return Some(vec![FormMessage::Submit]),
            KeyCode::Enter | KeyCode::Tab => {
                self.focus.focus_next();
                return Some(vec![]);
            }
            KeyCode::BackTab => {
                self.focus.focus_prev();
                return Some(vec![]);
            }
            KeyCode::Left if !self.active_field_uses_horizontal_arrows() => {
                self.focus.focus_prev();
                return Some(vec![]);
            }
            KeyCode::Right if !self.active_field_uses_horizontal_arrows() => {
                self.focus.focus_next();
                return Some(vec![]);
            }
            _ => {}
        }

        if let Some(field) = self.fields.get_mut(self.focus.focused()) {
            field.kind.handle_event(event).await;
        }
        Some(vec![])
    }

    fn render(&mut self, context: &ViewContext) -> Frame {
        let mut lines = vec![Line::with_style(&self.message, Style::fg(context.theme.text_primary()).bold())];
        lines.push(Line::default());

        if self.is_multi_step() {
            lines.push(self.render_tab_bar(context));
            lines.push(Line::default());
        }

        lines.extend(self.render_active_field(context));
        lines.push(Line::default());
        lines.push(self.render_footer(context));
        Frame::new(lines)
    }
}

#[cfg(test)]
mod tests {
    use super::super::select_option::SelectOption;
    use super::*;
    use crate::rendering::line::Line;
    use crossterm::event::{KeyEvent, KeyModifiers};

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn sample_fields() -> Vec<FormField> {
        vec![
            FormField {
                name: "lang".to_string(),
                label: "Language".to_string(),
                description: Some("Pick a language".to_string()),
                required: true,
                kind: FormFieldKind::SingleSelect(RadioSelect::new(
                    vec![
                        SelectOption { value: "rust".into(), title: "Rust".into(), description: None },
                        SelectOption { value: "ts".into(), title: "TypeScript".into(), description: None },
                    ],
                    0,
                )),
            },
            FormField {
                name: "name".to_string(),
                label: "Name".to_string(),
                description: None,
                required: false,
                kind: FormFieldKind::Text(TextField::new(String::new())),
            },
            FormField {
                name: "features".to_string(),
                label: "Features".to_string(),
                description: None,
                required: false,
                kind: FormFieldKind::MultiSelect(MultiSelect::new(
                    vec![
                        SelectOption { value: "a".into(), title: "Alpha".into(), description: None },
                        SelectOption { value: "b".into(), title: "Beta".into(), description: None },
                    ],
                    vec![false, false],
                )),
            },
        ]
    }

    fn single_select_field() -> Vec<FormField> {
        vec![FormField {
            name: "decision".to_string(),
            label: "Decision".to_string(),
            description: None,
            required: true,
            kind: FormFieldKind::SingleSelect(RadioSelect::new(
                vec![
                    SelectOption { value: "allow".into(), title: "allow".into(), description: None },
                    SelectOption { value: "deny".into(), title: "deny".into(), description: None },
                ],
                0,
            )),
        }]
    }

    #[test]
    fn render_does_not_panic_when_title_wider_than_terminal() {
        let mut form = Form::new(
            "This is a very long message that exceeds the terminal width".to_string(),
            vec![FormField {
                name: "name".to_string(),
                label: "Name".to_string(),
                description: None,
                required: false,
                kind: FormFieldKind::Text(TextField::new(String::new())),
            }],
        );
        let context = ViewContext::new((10, 10));

        // Should not panic
        let frame = form.render(&context);
        assert!(!frame.lines().is_empty());
    }

    #[test]
    fn tab_bar_shows_all_field_labels() {
        let form = Form::new("Survey".to_string(), sample_fields());
        let context = ViewContext::new((80, 24));
        let tab_bar = form.render_tab_bar(&context);
        let text = tab_bar.plain_text();
        assert!(text.contains("Language"), "tab bar missing 'Language'");
        assert!(text.contains("Name"), "tab bar missing 'Name'");
        assert!(text.contains("Features"), "tab bar missing 'Features'");
        assert!(text.contains("Submit"), "tab bar missing 'Submit'");
    }

    #[test]
    fn single_field_form_does_not_render_submit_tab() {
        let mut form = Form::new("Survey".to_string(), single_select_field());
        let context = ViewContext::new((80, 24));
        let frame = form.render(&context);
        let text: String = frame.lines().iter().map(Line::plain_text).collect::<Vec<_>>().join("\n");

        assert!(!text.contains("Submit"));
    }

    #[test]
    fn renders_only_active_field() {
        let mut form = Form::new("Survey".to_string(), sample_fields());
        let context = ViewContext::new((80, 24));

        // Focus is on field 0 (Language / RadioSelect)
        let frame = form.render(&context);
        let text: String = frame.lines().iter().map(Line::plain_text).collect::<Vec<_>>().join("\n");
        // The active field's options should be visible
        assert!(text.contains("Rust"), "active field options not visible");
        assert!(text.contains("TypeScript"), "active field options not visible");
        // The non-active fields' expanded content should NOT be visible
        // (MultiSelect options Alpha/Beta should not appear as expanded options)
        // But the tab bar mentions "Features", so just check that the expanded
        // checkbox options aren't rendered
        assert!(!text.contains("Alpha"), "inactive field content should not appear");
    }

    #[tokio::test]
    async fn tab_advances_to_next_pane() {
        let mut form = Form::new("Survey".to_string(), sample_fields());
        assert_eq!(form.focus.focused(), 0);
        form.on_event(&Event::Key(key(KeyCode::Tab))).await;
        assert_eq!(form.focus.focused(), 1);
        form.on_event(&Event::Key(key(KeyCode::Tab))).await;
        assert_eq!(form.focus.focused(), 2);
        form.on_event(&Event::Key(key(KeyCode::Tab))).await;
        assert_eq!(form.focus.focused(), 3); // Submit tab
    }

    #[tokio::test]
    async fn enter_on_submit_tab_emits_submit() {
        let mut form = Form::new("Survey".to_string(), sample_fields());
        // Navigate to submit tab (index 3)
        form.focus.focus(3);
        let msgs = form.on_event(&Event::Key(key(KeyCode::Enter))).await.unwrap();
        assert!(msgs.iter().any(|m| matches!(m, FormMessage::Submit)));
    }

    #[tokio::test]
    async fn enter_on_field_advances() {
        let mut form = Form::new("Survey".to_string(), sample_fields());
        assert_eq!(form.focus.focused(), 0);
        form.on_event(&Event::Key(key(KeyCode::Enter))).await;
        assert_eq!(form.focus.focused(), 1);
    }

    #[tokio::test]
    async fn single_field_form_enter_emits_submit() {
        let mut form = Form::new("Survey".to_string(), single_select_field());
        let msgs = form.on_event(&Event::Key(key(KeyCode::Enter))).await.unwrap();

        assert!(msgs.iter().any(|m| matches!(m, FormMessage::Submit)));
    }

    #[tokio::test]
    async fn empty_form_enter_emits_submit() {
        let mut form = Form::new("Survey".to_string(), vec![]);
        let msgs = form.on_event(&Event::Key(key(KeyCode::Enter))).await.unwrap();

        assert!(msgs.iter().any(|m| matches!(m, FormMessage::Submit)));
    }

    #[test]
    fn multi_field_form_still_has_submit_tab() {
        let mut form = Form::new("Survey".to_string(), sample_fields());
        let context = ViewContext::new((80, 24));
        let frame = form.render(&context);
        let text: String = frame.lines().iter().map(Line::plain_text).collect::<Vec<_>>().join("\n");

        assert!(text.contains("Submit"));
    }

    #[tokio::test]
    async fn multi_field_form_enter_before_submit_still_advances_focus() {
        let mut form = Form::new("Survey".to_string(), sample_fields());
        assert_eq!(form.focus.focused(), 0);

        let msgs = form.on_event(&Event::Key(key(KeyCode::Enter))).await.unwrap();

        assert!(msgs.is_empty());
        assert_eq!(form.focus.focused(), 1);
    }

    #[tokio::test]
    async fn left_right_navigate_tabs_for_select_fields() {
        let mut form = Form::new("Survey".to_string(), sample_fields());
        // Field 0 is a RadioSelect — Right should navigate to next tab
        assert_eq!(form.focus.focused(), 0);
        form.on_event(&Event::Key(key(KeyCode::Right))).await;
        assert_eq!(form.focus.focused(), 1);

        // Field 2 is a MultiSelect — Left should navigate to previous tab
        form.focus.focus(2);
        form.on_event(&Event::Key(key(KeyCode::Left))).await;
        assert_eq!(form.focus.focused(), 1);
    }

    #[tokio::test]
    async fn left_right_delegate_to_text_field() {
        let mut form = Form::new("Survey".to_string(), sample_fields());
        // Navigate to field 1 (Text field), type something, then use Left
        form.focus.focus(1);
        form.on_event(&Event::Key(key(KeyCode::Char('h')))).await;
        form.on_event(&Event::Key(key(KeyCode::Char('i')))).await;
        assert_eq!(form.focus.focused(), 1);

        // Left should move cursor within text field, not change tab
        form.on_event(&Event::Key(key(KeyCode::Left))).await;
        assert_eq!(form.focus.focused(), 1); // still on same tab

        // Verify the cursor moved in the text field
        if let FormFieldKind::Text(ref tf) = form.fields[1].kind {
            assert_eq!(tf.cursor_pos(), 1); // moved left from 2 to 1
        } else {
            panic!("expected Text field");
        }
    }

    #[test]
    fn is_answered_text_field() {
        assert!(!FormFieldKind::Text(TextField::new(String::new())).is_answered());
        assert!(FormFieldKind::Text(TextField::new("hello".to_string())).is_answered());
    }

    #[test]
    fn is_answered_multi_select() {
        let none_selected = FormFieldKind::MultiSelect(MultiSelect::new(
            vec![SelectOption { value: "a".into(), title: "A".into(), description: None }],
            vec![false],
        ));
        assert!(!none_selected.is_answered());

        let some_selected = FormFieldKind::MultiSelect(MultiSelect::new(
            vec![SelectOption { value: "a".into(), title: "A".into(), description: None }],
            vec![true],
        ));
        assert!(some_selected.is_answered());
    }

    #[tokio::test]
    async fn esc_emits_close() {
        let mut form = Form::new("Survey".to_string(), sample_fields());
        let msgs = form.on_event(&Event::Key(key(KeyCode::Esc))).await.unwrap();
        assert!(msgs.iter().any(|m| matches!(m, FormMessage::Close)));
    }

    #[tokio::test]
    async fn backtab_moves_backward() {
        let mut form = Form::new("Survey".to_string(), sample_fields());
        form.focus.focus(2);
        form.on_event(&Event::Key(KeyEvent::new(KeyCode::BackTab, KeyModifiers::SHIFT))).await;
        assert_eq!(form.focus.focused(), 1);
    }

    #[test]
    fn submit_tab_renders_summary() {
        let mut form = Form::new("Survey".to_string(), sample_fields());
        form.focus.focus(3); // Submit tab
        let context = ViewContext::new((80, 24));
        let frame = form.render(&context);
        let text: String = frame.lines().iter().map(Line::plain_text).collect::<Vec<_>>().join("\n");
        assert!(text.contains("Review & Submit"));
        assert!(text.contains("Language:"));
        assert!(text.contains("Name:"));
    }
}
