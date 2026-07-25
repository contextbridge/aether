use acp_utils::{
    ConstTitle, ElicitationSchema, EnumSchema, MultiSelectEnumSchema, PrimitiveSchema, SingleSelectEnumSchema,
};
use crossterm::event::{KeyCode, KeyEvent};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Paragraph, Widget, Wrap};
use serde_json::{Map, Value};

use crate::edit_buffer::{EditBuffer, apply_edit_key};
use crate::selection::Direction;
use crate::theme::Theme;
use crate::wrap::rows;

pub(super) struct FormModal {
    server_name: String,
    message: String,
    pub(super) fields: Vec<FormField>,
    pub(super) selected: usize,
    validation_error: Option<String>,
    /// Terminal row the first field was last drawn on, so a click lands on the
    /// field the user actually pointed at wherever the modal was centred.
    first_field_row: u16,
}

pub(super) struct FormField {
    name: String,
    label: String,
    pub(super) description: Option<String>,
    required: bool,
    pub(super) kind: FormFieldKind,
}

pub(super) enum FormFieldKind {
    Text(EditBuffer),
    Number(EditBuffer),
    Boolean(bool),
    Single { options: Vec<SelectOption>, selected: usize },
    Multi { options: Vec<SelectOption>, selected: Vec<bool>, cursor: usize },
}

pub(super) struct SelectOption {
    value: String,
    title: String,
}

pub(super) enum FormAction {
    None,
    Cancel,
    Accept(Value),
}

impl FormModal {
    pub(super) fn new(server_name: String, message: String, schema: &ElicitationSchema) -> Self {
        let required: Vec<&str> = schema.required.as_deref().unwrap_or(&[]).iter().map(String::as_str).collect();
        let fields =
            schema.properties.iter().map(|(name, prop)| FormField::from_primitive(name, prop, &required)).collect();
        Self { server_name, message, fields, selected: 0, validation_error: None, first_field_row: 0 }
    }

    pub(super) fn on_key(&mut self, key: KeyEvent) -> FormAction {
        // Inside a multi-select, arrows and space move within its options rather
        // than between fields.
        if self.multi_select_key(key) {
            return FormAction::None;
        }

        match key.code {
            KeyCode::Esc => return FormAction::Cancel,
            KeyCode::Enter => return self.submit(),
            KeyCode::Up | KeyCode::BackTab => self.scroll(Direction::Backward),
            KeyCode::Down | KeyCode::Tab => self.scroll(Direction::Forward),
            KeyCode::Left => self.cycle_focused(Direction::Backward),
            KeyCode::Right | KeyCode::Char(' ') => self.cycle_focused(Direction::Forward),
            _ => {
                if let Some(FormFieldKind::Text(value) | FormFieldKind::Number(value)) = self.focused_kind() {
                    apply_edit_key(value, key);
                }
            }
        }
        FormAction::None
    }

    /// Moves between fields, or within the focused multi-select's options.
    pub(super) fn scroll(&mut self, direction: Direction) {
        if let Some(FormFieldKind::Multi { options, cursor, .. }) = self.focused_kind() {
            *cursor = match direction {
                Direction::Backward => cursor.saturating_sub(1),
                Direction::Forward => (*cursor + 1).min(options.len().saturating_sub(1)),
            };
            return;
        }
        self.selected = match direction {
            Direction::Backward => self.selected.saturating_sub(1),
            Direction::Forward => (self.selected + 1).min(self.fields.len().saturating_sub(1)),
        };
    }

    /// Focuses the field drawn at terminal `row` and activates it, so a click
    /// both selects and toggles in one gesture.
    pub(super) fn click(&mut self, row: u16) {
        let Some(field_row) = row.checked_sub(self.first_field_row).map(usize::from) else {
            return;
        };
        let mut current = 0usize;
        for index in 0..self.fields.len() {
            if current == field_row {
                self.selected = index;
                match self.focused_kind() {
                    Some(FormFieldKind::Boolean(_) | FormFieldKind::Single { .. }) => {
                        self.cycle_focused(Direction::Forward);
                    }
                    Some(FormFieldKind::Multi { .. }) => self.toggle_multi_select(),
                    _ => {}
                }
                return;
            }
            current += self.fields[index].rendered_rows();
        }
    }

    fn multi_select_key(&mut self, key: KeyEvent) -> bool {
        if !matches!(self.focused_kind(), Some(FormFieldKind::Multi { .. })) {
            return false;
        }
        match key.code {
            KeyCode::Up => self.scroll(Direction::Backward),
            KeyCode::Down => self.scroll(Direction::Forward),
            KeyCode::Char(' ') => self.toggle_multi_select(),
            _ => return false,
        }
        true
    }

    fn toggle_multi_select(&mut self) {
        if let Some(FormFieldKind::Multi { selected, cursor, .. }) = self.focused_kind()
            && let Some(flag) = selected.get_mut(*cursor)
        {
            *flag = !*flag;
        }
    }

    /// Advances a boolean or single-select field to its next value.
    fn cycle_focused(&mut self, direction: Direction) {
        let step = match direction {
            Direction::Backward => -1,
            Direction::Forward => 1,
        };
        match self.focused_kind() {
            Some(FormFieldKind::Boolean(value)) => *value = !*value,
            Some(FormFieldKind::Single { options, selected }) if !options.is_empty() => {
                *selected = selected.saturating_add_signed(step).min(options.len() - 1);
            }
            _ => {}
        }
    }

    fn focused_kind(&mut self) -> Option<&mut FormFieldKind> {
        self.fields.get_mut(self.selected).map(|field| &mut field.kind)
    }

    fn submit(&mut self) -> FormAction {
        let mut content = Map::new();
        for field in &self.fields {
            let value = match field.value() {
                Ok(value) => value,
                Err(error) => {
                    self.validation_error = Some(error);
                    return FormAction::None;
                }
            };
            if !value.is_null() {
                content.insert(field.name.clone(), value);
            }
        }
        FormAction::Accept(Value::Object(content))
    }

    pub(super) fn render(&mut self, area: Rect, buf: &mut Buffer, theme: &Theme) {
        let mut lines = vec![Line::styled(
            format!("Request from {}", self.server_name),
            Style::new().fg(theme.accent).add_modifier(Modifier::BOLD),
        )];
        if !self.message.is_empty() {
            lines.push(Line::raw(self.message.clone()));
        }
        let block = Block::bordered().title(" Elicitation ").border_style(Style::new().fg(theme.accent));
        self.first_field_row = block.inner(area).y.saturating_add(rows(lines.len()));

        for (index, field) in self.fields.iter().enumerate() {
            let marker = if index == self.selected { "›" } else { " " };
            let required = if field.required { " *" } else { "" };
            let styled_label =
                Span::styled(format!("{marker} {}{required}: ", field.label), Style::new().fg(theme.heading));
            match &field.kind {
                FormFieldKind::Multi { options, selected, cursor } => {
                    lines.push(Line::from(styled_label));
                    for (opt_index, option) in options.iter().enumerate() {
                        let prefix = if opt_index == *cursor { "›" } else { " " };
                        let checkbox = if selected[opt_index] { "[x]" } else { "[ ]" };
                        let opt_style = if opt_index == *cursor {
                            Style::new().fg(theme.accent)
                        } else {
                            Style::new().fg(theme.text_primary)
                        };
                        lines.push(Line::styled(format!("  {prefix} {checkbox} {}", option.title), opt_style));
                    }
                }
                _ => {
                    lines.push(Line::from(vec![styled_label, Span::raw(field.display_value())]));
                }
            }
            if let Some(description) = &field.description {
                lines.push(Line::styled(format!("    {description}"), Style::new().fg(theme.muted)));
            }
        }
        if let Some(error) = &self.validation_error {
            lines.push(Line::styled(error.clone(), Style::new().fg(theme.error)));
        }
        if self.fields.iter().any(|f| matches!(&f.kind, FormFieldKind::Multi { .. })) {
            lines.push(Line::styled(
                "↑↓ option · Space toggle · Enter submit · Esc cancel",
                Style::new().fg(theme.muted),
            ));
        } else {
            lines.push(Line::styled("Enter submit · Esc cancel", Style::new().fg(theme.muted)));
        }
        Paragraph::new(Text::from(lines)).block(block).wrap(Wrap { trim: false }).render(area, buf);
    }
}

impl FormField {
    fn from_primitive(name: &str, prop: &PrimitiveSchema, required: &[&str]) -> Self {
        let (label, description, kind) = match prop {
            PrimitiveSchema::Boolean(b) => {
                let label = b.title.as_deref().unwrap_or(name).to_string();
                let description = b.description.as_deref().map(str::to_string);
                let kind = FormFieldKind::Boolean(b.default.unwrap_or(false));
                (label, description, kind)
            }
            PrimitiveSchema::Integer(i) => {
                let label = i.title.as_deref().unwrap_or(name).to_string();
                let description = i.description.as_deref().map(str::to_string);
                let default_str = i.default.map(|d| d.to_string()).unwrap_or_default();
                (label, description, FormFieldKind::Number(default_str.into()))
            }
            PrimitiveSchema::Number(n) => {
                let label = n.title.as_deref().unwrap_or(name).to_string();
                let description = n.description.as_deref().map(str::to_string);
                let default_str = n.default.map(|d| d.to_string()).unwrap_or_default();
                (label, description, FormFieldKind::Number(default_str.into()))
            }
            PrimitiveSchema::String(s) => {
                let label = s.title.as_deref().unwrap_or(name).to_string();
                let description = s.description.as_deref().map(str::to_string);
                let default_str = s.default.clone().unwrap_or_default();
                (label, description, FormFieldKind::Text(default_str.into()))
            }
            PrimitiveSchema::Enum(e) => {
                let (label, description) = extract_enum_metadata(e, name);
                let kind = parse_enum_kind(e);
                (label, description, kind)
            }
        };
        Self { name: name.to_string(), label, description, required: required.contains(&name), kind }
    }

    fn value(&self) -> Result<Value, String> {
        let missing = || format!("{} is required", self.label);
        match &self.kind {
            FormFieldKind::Text(value) => {
                if self.required && value.is_empty() {
                    Err(missing())
                } else {
                    Ok(if value.is_empty() { Value::Null } else { Value::String(value.text().to_string()) })
                }
            }
            FormFieldKind::Number(value) => {
                if value.is_empty() {
                    return if self.required { Err(missing()) } else { Ok(Value::Null) };
                }
                serde_json::from_str(value.text()).map_err(|_| format!("{} must be a number", self.label))
            }
            FormFieldKind::Boolean(value) => Ok(Value::Bool(*value)),
            FormFieldKind::Single { options, selected } => {
                let value = options.get(*selected).map(|o| o.value.clone());
                if self.required && value.is_none() {
                    Err(missing())
                } else {
                    Ok(value.map_or(Value::Null, Value::String))
                }
            }
            FormFieldKind::Multi { options, selected, .. } => Ok(Value::Array(
                options
                    .iter()
                    .zip(selected)
                    .filter(|(_, selected)| **selected)
                    .map(|(opt, _)| Value::String(opt.value.clone()))
                    .collect(),
            )),
        }
    }

    /// Lines this field occupies, so click targets line up with what is drawn.
    fn rendered_rows(&self) -> usize {
        let options = match &self.kind {
            FormFieldKind::Multi { options, .. } => options.len(),
            _ => 0,
        };
        let description = usize::from(self.description.as_ref().is_some_and(|text| !text.is_empty()));
        1 + options + description
    }

    fn display_value(&self) -> String {
        match &self.kind {
            FormFieldKind::Text(value) | FormFieldKind::Number(value) => value.text().to_string(),
            FormFieldKind::Boolean(value) => if *value { "[x]" } else { "[ ]" }.to_string(),
            FormFieldKind::Single { options, selected } => {
                options.get(*selected).map(|o| o.title.clone()).unwrap_or_default()
            }
            FormFieldKind::Multi { options, selected, .. } => options
                .iter()
                .zip(selected)
                .filter(|(_, selected)| **selected)
                .map(|(opt, _)| opt.title.as_str())
                .collect::<Vec<_>>()
                .join(", "),
        }
    }
}

fn extract_enum_metadata(e: &EnumSchema, name: &str) -> (String, Option<String>) {
    match e {
        EnumSchema::Single(s) => match s {
            SingleSelectEnumSchema::Untitled(u) => {
                (u.title.as_deref().unwrap_or(name).to_string(), u.description.as_deref().map(str::to_string))
            }
            SingleSelectEnumSchema::Titled(t) => {
                (t.title.as_deref().unwrap_or(name).to_string(), t.description.as_deref().map(str::to_string))
            }
        },
        EnumSchema::Multi(m) => match m {
            MultiSelectEnumSchema::Untitled(u) => {
                (u.title.as_deref().unwrap_or(name).to_string(), u.description.as_deref().map(str::to_string))
            }
            MultiSelectEnumSchema::Titled(t) => {
                (t.title.as_deref().unwrap_or(name).to_string(), t.description.as_deref().map(str::to_string))
            }
        },
        EnumSchema::Legacy(l) => {
            (l.title.as_deref().unwrap_or(name).to_string(), l.description.as_deref().map(str::to_string))
        }
    }
}

fn parse_enum_kind(e: &EnumSchema) -> FormFieldKind {
    match e {
        EnumSchema::Single(s) => match s {
            SingleSelectEnumSchema::Untitled(u) => {
                let options = options_from_strings(&u.enum_);
                let default_idx =
                    u.default.as_ref().and_then(|d| options.iter().position(|o| o.value == *d)).unwrap_or(0);
                FormFieldKind::Single { options, selected: default_idx }
            }
            SingleSelectEnumSchema::Titled(t) => {
                let options = options_from_const_titles(&t.one_of);
                let default_idx =
                    t.default.as_ref().and_then(|d| options.iter().position(|o| o.value == *d)).unwrap_or(0);
                FormFieldKind::Single { options, selected: default_idx }
            }
        },
        EnumSchema::Multi(m) => match m {
            MultiSelectEnumSchema::Untitled(u) => {
                let options = options_from_strings(&u.items.enum_);
                let defaults = u.default.as_deref().unwrap_or(&[]);
                let selected: Vec<bool> = options.iter().map(|o| defaults.contains(&o.value)).collect();
                FormFieldKind::Multi { options, selected, cursor: 0 }
            }
            MultiSelectEnumSchema::Titled(t) => {
                let options = options_from_const_titles(&t.items.any_of);
                let defaults = t.default.as_deref().unwrap_or(&[]);
                let selected: Vec<bool> = options.iter().map(|o| defaults.contains(&o.value)).collect();
                FormFieldKind::Multi { options, selected, cursor: 0 }
            }
        },
        EnumSchema::Legacy(l) => {
            let options = options_from_strings(&l.enum_);
            FormFieldKind::Single { options, selected: 0 }
        }
    }
}

fn options_from_strings(values: &[String]) -> Vec<SelectOption> {
    values.iter().map(|s| SelectOption { value: s.clone(), title: s.clone() }).collect()
}

fn options_from_const_titles(items: &[ConstTitle]) -> Vec<SelectOption> {
    items.iter().map(|ct| SelectOption { value: ct.const_.clone(), title: ct.title.clone() }).collect()
}

#[cfg(test)]
#[allow(clippy::absolute_paths, clippy::similar_names)]
mod tests {
    use super::*;
    use crossterm::event::KeyModifiers;

    fn permission_like_schema() -> ElicitationSchema {
        ElicitationSchema::builder()
            .required_enum_schema(
                "decision",
                EnumSchema::builder(vec!["allow".into(), "deny".into()])
                    .untitled()
                    .with_default(String::from("deny"))
                    .unwrap()
                    .build(),
            )
            .build()
            .unwrap()
    }
    // ── Form schema parsing ──

    #[test]
    fn parses_string_field_with_title_and_description() {
        let schema = ElicitationSchema::builder()
            .required_string_with("name", |s| s.title("Your Name").description("Enter your full name"))
            .build()
            .unwrap();
        let form = FormModal::new("test".into(), String::new(), &schema);
        assert_eq!(form.fields.len(), 1);
        assert_eq!(form.fields[0].label, "Your Name");
        assert_eq!(form.fields[0].description.as_deref(), Some("Enter your full name"));
        assert!(form.fields[0].required);
        assert!(matches!(form.fields[0].kind, FormFieldKind::Text(_)));
    }

    #[test]
    fn parses_boolean_field_with_default() {
        let schema = ElicitationSchema::builder().optional_bool("approved", true).build().unwrap();
        let form = FormModal::new("test".into(), String::new(), &schema);
        assert_eq!(form.fields.len(), 1);
        assert!(matches!(form.fields[0].kind, FormFieldKind::Boolean(true)));
    }

    #[test]
    fn parses_integer_and_number_fields() {
        let schema = ElicitationSchema::builder()
            .required_integer("age", 0, 150)
            .required_number("rating", 0.0, 5.0)
            .build()
            .unwrap();
        let form = FormModal::new("test".into(), String::new(), &schema);
        assert_eq!(form.fields.len(), 2);
        assert!(matches!(form.fields[0].kind, FormFieldKind::Number(_)));
        assert!(matches!(form.fields[1].kind, FormFieldKind::Number(_)));
    }

    #[test]
    fn integer_field_respects_default() {
        let schema = ElicitationSchema::builder()
            .required_integer_with("count", |i| i.range(0, 100).with_default(42))
            .build()
            .unwrap();
        let form = FormModal::new("test".into(), String::new(), &schema);
        match &form.fields[0].kind {
            FormFieldKind::Number(value) => assert_eq!(value, "42"),
            _ => panic!("expected Number"),
        }
    }

    #[test]
    fn number_field_respects_default() {
        let schema = ElicitationSchema::builder()
            .required_number_with("score", |n| n.range(0.0, 100.0).with_default(2.5))
            .build()
            .unwrap();
        let form = FormModal::new("test".into(), String::new(), &schema);
        match &form.fields[0].kind {
            FormFieldKind::Number(value) => {
                let parsed: f64 = value.parse().unwrap();
                assert!((parsed - 2.5).abs() < 0.001, "expected ~2.5, got {}", value.text());
            }
            _ => panic!("expected Number"),
        }
    }

    #[test]
    fn string_field_respects_default() {
        let schema =
            ElicitationSchema::builder().required_string_with("greeting", |s| s.with_default("hello")).build().unwrap();
        let form = FormModal::new("test".into(), String::new(), &schema);
        match &form.fields[0].kind {
            FormFieldKind::Text(value) => assert_eq!(value, "hello"),
            _ => panic!("expected Text"),
        }
    }

    #[test]
    fn parses_single_select_enum_from_schema() {
        let schema = ElicitationSchema::builder()
            .required_enum_schema(
                "color",
                EnumSchema::builder(vec!["red".into(), "green".into(), "blue".into()]).untitled().build(),
            )
            .build()
            .unwrap();
        let form = FormModal::new("test".into(), String::new(), &schema);
        match &form.fields[0].kind {
            FormFieldKind::Single { options, selected } => {
                assert_eq!(options.len(), 3);
                assert_eq!(options[0].value, "red");
                assert_eq!(*selected, 0);
            }
            _ => panic!("expected Single"),
        }
    }

    #[test]
    fn parses_single_select_with_default() {
        let schema = ElicitationSchema::builder()
            .required_enum_schema(
                "color",
                EnumSchema::builder(vec!["red".into(), "green".into()])
                    .untitled()
                    .with_default("green".to_string())
                    .unwrap()
                    .build(),
            )
            .build()
            .unwrap();
        let form = FormModal::new("test".into(), String::new(), &schema);
        match &form.fields[0].kind {
            FormFieldKind::Single { options, selected } => {
                assert_eq!(*selected, 1);
                assert_eq!(options[1].value, "green");
            }
            _ => panic!("expected Single"),
        }
    }

    #[test]
    fn parses_titled_single_select_with_const_titles() {
        let schema = ElicitationSchema::builder()
            .required_enum_schema(
                "size",
                EnumSchema::builder(vec!["s".into(), "m".into(), "l".into()])
                    .enum_titles(vec!["Small".into(), "Medium".into(), "Large".into()])
                    .unwrap()
                    .build(),
            )
            .build()
            .unwrap();
        let form = FormModal::new("test".into(), String::new(), &schema);
        match &form.fields[0].kind {
            FormFieldKind::Single { options, .. } => {
                assert_eq!(options.len(), 3);
                assert_eq!(options[0].title, "Small");
                assert_eq!(options[0].value, "s");
            }
            _ => panic!("expected Single"),
        }
    }

    #[test]
    fn parses_multi_select_enum() {
        let schema = ElicitationSchema::builder()
            .required_enum_schema(
                "tags",
                EnumSchema::builder(vec!["fast".into(), "reliable".into(), "cheap".into()]).multiselect().build(),
            )
            .build()
            .unwrap();
        let form = FormModal::new("test".into(), String::new(), &schema);
        match &form.fields[0].kind {
            FormFieldKind::Multi { options, selected, cursor } => {
                assert_eq!(options.len(), 3);
                assert_eq!(selected.len(), 3);
                assert_eq!(*cursor, 0);
                assert!(selected.iter().all(|s| !*s));
            }
            _ => panic!("expected Multi"),
        }
    }

    #[test]
    fn parses_multi_select_with_defaults() {
        let schema = ElicitationSchema::builder()
            .required_enum_schema(
                "tags",
                EnumSchema::builder(vec!["fast".into(), "reliable".into(), "cheap".into()])
                    .multiselect()
                    .with_default(vec!["reliable".to_string()])
                    .unwrap()
                    .build(),
            )
            .build()
            .unwrap();
        let form = FormModal::new("test".into(), String::new(), &schema);
        match &form.fields[0].kind {
            FormFieldKind::Multi { selected, .. } => {
                assert!(!selected[0]);
                assert!(selected[1]);
                assert!(!selected[2]);
            }
            _ => panic!("expected Multi"),
        }
    }

    #[test]
    fn empty_schema_produces_no_fields() {
        let schema = ElicitationSchema::builder().build().unwrap();
        let form = FormModal::new("test".into(), String::new(), &schema);
        assert!(form.fields.is_empty());
    }

    // ── Field values / JSON output ──

    #[test]
    fn text_field_value_returns_string_or_null() {
        let field = FormField {
            name: "x".into(),
            label: "X".into(),
            description: None,
            required: false,
            kind: FormFieldKind::Text("hello".into()),
        };
        assert_eq!(field.value().unwrap(), Value::String("hello".into()));
        let empty = FormField {
            name: "x".into(),
            label: "X".into(),
            description: None,
            required: false,
            kind: FormFieldKind::Text(String::new().into()),
        };
        assert_eq!(empty.value().unwrap(), Value::Null);
    }

    #[test]
    fn required_text_field_errors_on_empty() {
        let field = FormField {
            name: "x".into(),
            label: "Name".into(),
            description: None,
            required: true,
            kind: FormFieldKind::Text(String::new().into()),
        };
        assert!(field.value().is_err());
    }

    #[test]
    fn number_field_returns_parsed_number() {
        let field = FormField {
            name: "x".into(),
            label: "Age".into(),
            description: None,
            required: false,
            kind: FormFieldKind::Number("42".into()),
        };
        assert_eq!(field.value().unwrap(), Value::Number(serde_json::Number::from(42)));
    }

    #[test]
    fn number_field_errors_on_non_numeric() {
        let field = FormField {
            name: "x".into(),
            label: "Age".into(),
            description: None,
            required: false,
            kind: FormFieldKind::Number("abc".into()),
        };
        assert!(field.value().is_err());
    }

    #[test]
    fn boolean_field_returns_bool() {
        let field = FormField {
            name: "x".into(),
            label: "Flag".into(),
            description: None,
            required: false,
            kind: FormFieldKind::Boolean(true),
        };
        assert_eq!(field.value().unwrap(), Value::Bool(true));
    }

    #[test]
    fn single_select_returns_selected_value() {
        let field = FormField {
            name: "color".into(),
            label: "Color".into(),
            description: None,
            required: false,
            kind: FormFieldKind::Single {
                options: vec![
                    SelectOption { value: "red".into(), title: "Red".into() },
                    SelectOption { value: "green".into(), title: "Green".into() },
                ],
                selected: 1,
            },
        };
        assert_eq!(field.value().unwrap(), Value::String("green".into()));
    }

    #[test]
    fn multi_select_returns_selected_values() {
        let field = FormField {
            name: "tags".into(),
            label: "Tags".into(),
            description: None,
            required: false,
            kind: FormFieldKind::Multi {
                options: vec![
                    SelectOption { value: "a".into(), title: "A".into() },
                    SelectOption { value: "b".into(), title: "B".into() },
                ],
                selected: vec![true, false],
                cursor: 0,
            },
        };
        assert_eq!(field.value().unwrap(), Value::Array(vec![Value::String("a".into())]));
    }

    // ── Form submission ──

    #[test]
    fn accept_produces_correct_json() {
        let schema = ElicitationSchema::builder()
            .optional_string_with("name", |s| s.title("Name"))
            .optional_bool("approved", true)
            .optional_enum_schema(
                "color",
                EnumSchema::builder(vec!["red".into(), "green".into()])
                    .untitled()
                    .with_default("green".to_string())
                    .unwrap()
                    .build(),
            )
            .build()
            .unwrap();
        let mut form = FormModal::new("test".into(), "Test".into(), &schema);
        match form.submit() {
            FormAction::Accept(value) => {
                let obj = value.as_object().unwrap();
                assert!(!obj.contains_key("name"), "empty optional text should be omitted");
                assert_eq!(obj["approved"], Value::Bool(true));
                assert_eq!(obj["color"], Value::String("green".into()));
            }
            _ => panic!("expected Accept"),
        }
    }

    #[test]
    fn submit_rejects_required_field() {
        let schema = ElicitationSchema::builder().required_string("name").build().unwrap();
        let mut form = FormModal::new("test".into(), String::new(), &schema);
        match form.submit() {
            FormAction::None => {}
            _ => panic!("expected None"),
        }
    }

    // ── Multi-select navigation ──

    #[test]
    fn multi_select_cursor_moves_and_toggles_independent_options() {
        let schema = ElicitationSchema::builder()
            .required_enum_schema(
                "opts",
                EnumSchema::builder(vec!["a".into(), "b".into(), "c".into()]).multiselect().build(),
            )
            .build()
            .unwrap();
        let mut form = FormModal::new("test".into(), String::new(), &schema);

        form.on_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
        form.on_key(KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE));
        form.on_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
        form.on_key(KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE));

        match &form.fields[0].kind {
            FormFieldKind::Multi { selected, cursor, .. } => {
                assert_eq!(*cursor, 2);
                assert!(!selected[0], "option 0 should remain untoggled");
                assert!(selected[1], "option 1 should be toggled");
                assert!(selected[2], "option 2 should be toggled");
            }
            _ => panic!("expected Multi"),
        }
    }

    #[test]
    fn multi_select_up_saturates_at_zero() {
        let schema = ElicitationSchema::builder()
            .required_enum_schema("opts", EnumSchema::builder(vec!["a".into(), "b".into()]).multiselect().build())
            .build()
            .unwrap();
        let mut form = FormModal::new("test".into(), String::new(), &schema);

        form.on_key(KeyEvent::new(KeyCode::Up, KeyModifiers::NONE));
        match &form.fields[0].kind {
            FormFieldKind::Multi { cursor, .. } => assert_eq!(*cursor, 0),
            _ => panic!("expected Multi"),
        }
    }

    #[test]
    fn multi_select_space_toggles_the_focused_option() {
        let schema = ElicitationSchema::builder()
            .required_enum_schema("opts", EnumSchema::builder(vec!["a".into(), "b".into()]).multiselect().build())
            .build()
            .unwrap();
        let mut form = FormModal::new("test".into(), String::new(), &schema);

        form.on_key(KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE));
        match &form.fields[0].kind {
            FormFieldKind::Multi { selected, .. } => assert!(selected[0]),
            _ => panic!("expected Multi"),
        }
    }

    #[test]
    fn multi_select_submit_returns_selected_values() {
        let schema = ElicitationSchema::builder()
            .required_enum_schema("opts", EnumSchema::builder(vec!["a".into(), "b".into()]).multiselect().build())
            .build()
            .unwrap();
        let mut form = FormModal::new("test".into(), String::new(), &schema);
        form.on_key(KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE));
        match form.submit() {
            FormAction::Accept(value) => {
                assert_eq!(value, serde_json::json!({"opts": ["a"]}));
            }
            _ => panic!("expected Accept"),
        }
    }
    #[test]
    fn permission_like_form_submit_returns_default_deny() {
        let schema = permission_like_schema();
        let mut form = FormModal::new("coding".into(), "Allow bash: rm -rf /tmp?".into(), &schema);
        match form.submit() {
            FormAction::Accept(val) => {
                assert_eq!(val["decision"], "deny");
            }
            _ => panic!("expected Accept"),
        }
    }
}
