use acp_utils::notifications::{
    CreateElicitationRequestParams, ElicitationAction, ElicitationParams, ElicitationResponse, McpNotification,
    UrlElicitationCompleteParams,
};
use agent_client_protocol::Responder;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Wrap};
use serde_json::{Map, Value};

use crate::theme::Theme;

pub struct ElicitationModal {
    kind: ModalKind,
    responder: Option<Responder<ElicitationResponse>>,
}

enum ModalKind {
    Form(FormModal),
    Url(UrlModal),
}

struct FormModal {
    server_name: String,
    message: String,
    fields: Vec<FormField>,
    selected: usize,
    validation_error: Option<String>,
}

struct FormField {
    name: String,
    label: String,
    description: Option<String>,
    required: bool,
    kind: FormFieldKind,
}

enum FormFieldKind {
    Text(String),
    Number(String),
    Boolean(bool),
    Single { options: Vec<String>, selected: usize },
    Multi { options: Vec<String>, selected: Vec<bool> },
}

struct UrlModal {
    server_name: String,
    elicitation_id: String,
    message: String,
    url: String,
}

pub enum ModalOutcome {
    None,
    Close,
}

impl ElicitationModal {
    pub fn new(params: ElicitationParams, responder: Responder<ElicitationResponse>) -> Self {
        let kind = match params.request {
            CreateElicitationRequestParams::FormElicitationParams { message, requested_schema, .. } => {
                ModalKind::Form(FormModal::new(params.server_name, message, &requested_schema))
            }
            CreateElicitationRequestParams::UrlElicitationParams { message, url, elicitation_id, .. } => {
                ModalKind::Url(UrlModal { server_name: params.server_name, elicitation_id, message, url })
            }
        };
        Self { kind, responder: Some(responder) }
    }

    pub fn on_key(&mut self, key: KeyEvent) -> ModalOutcome {
        if !matches!(key.kind, crossterm::event::KeyEventKind::Press | crossterm::event::KeyEventKind::Repeat) {
            return ModalOutcome::None;
        }
        match &mut self.kind {
            ModalKind::Form(form) => match form.on_key(key) {
                FormAction::None => ModalOutcome::None,
                FormAction::Cancel => self.respond(ElicitationAction::Cancel, None),
                FormAction::Accept(content) => self.respond(ElicitationAction::Accept, Some(content)),
            },
            ModalKind::Url(url) => match key.code {
                KeyCode::Esc => self.respond(ElicitationAction::Cancel, None),
                KeyCode::Enter => {
                    if let Err(error) = open_url(&url.url) {
                        tracing::warn!(%error, "failed to open elicitation URL");
                    }
                    ModalOutcome::None
                }
                _ => ModalOutcome::None,
            },
        }
    }

    pub fn on_notification(&mut self, notification: &McpNotification) -> ModalOutcome {
        let McpNotification::UrlElicitationComplete(params) = notification else {
            return ModalOutcome::None;
        };
        if self.matches_url_completion(params) {
            self.respond(ElicitationAction::Accept, None)
        } else {
            ModalOutcome::None
        }
    }

    pub fn cancel(&mut self) {
        let _ = self.respond(ElicitationAction::Cancel, None);
    }

    pub fn render(&self, frame: &mut Frame, theme: &Theme) {
        let area = centered_rect(frame.area(), 80, 80);
        frame.render_widget(Clear, area);
        match &self.kind {
            ModalKind::Form(form) => form.render(frame, area, theme),
            ModalKind::Url(url) => url.render(frame, area, theme),
        }
    }

    fn matches_url_completion(&self, params: &UrlElicitationCompleteParams) -> bool {
        matches!(
            &self.kind,
            ModalKind::Url(url) if url.server_name == params.server_name && url.elicitation_id == params.elicitation_id
        )
    }

    fn respond(&mut self, action: ElicitationAction, content: Option<Value>) -> ModalOutcome {
        if let Some(responder) = self.responder.take() {
            let _ = responder.respond(ElicitationResponse { action, content });
        }
        ModalOutcome::Close
    }
}

enum FormAction {
    None,
    Cancel,
    Accept(Value),
}

impl FormModal {
    fn new(server_name: String, message: String, schema: &impl serde::Serialize) -> Self {
        let value = serde_json::to_value(schema).unwrap_or_default();
        let required = value
            .get("required")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .collect::<Vec<_>>();
        let fields = value
            .get("properties")
            .and_then(Value::as_object)
            .map(|properties| {
                properties.iter().map(|(name, schema)| FormField::from_schema(name, schema, &required)).collect()
            })
            .unwrap_or_default();
        Self { server_name, message, fields, selected: 0, validation_error: None }
    }

    fn on_key(&mut self, key: KeyEvent) -> FormAction {
        match key.code {
            KeyCode::Esc => FormAction::Cancel,
            KeyCode::Up | KeyCode::BackTab => {
                self.selected = self.selected.saturating_sub(1);
                FormAction::None
            }
            KeyCode::Down | KeyCode::Tab => {
                self.selected = (self.selected + 1).min(self.fields.len().saturating_sub(1));
                FormAction::None
            }
            KeyCode::Enter => self.submit(),
            KeyCode::Left => {
                self.change_selection(-1);
                FormAction::None
            }
            KeyCode::Right | KeyCode::Char(' ') => {
                self.change_selection(1);
                FormAction::None
            }
            KeyCode::Backspace => {
                if let Some(field) = self.fields.get_mut(self.selected) {
                    match &mut field.kind {
                        FormFieldKind::Text(value) | FormFieldKind::Number(value) => {
                            value.pop();
                        }
                        _ => {}
                    }
                }
                FormAction::None
            }
            KeyCode::Char(character) if !key.modifiers.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) => {
                if let Some(field) = self.fields.get_mut(self.selected) {
                    match &mut field.kind {
                        FormFieldKind::Text(value) | FormFieldKind::Number(value) => value.push(character),
                        _ => {}
                    }
                }
                FormAction::None
            }
            _ => FormAction::None,
        }
    }

    fn change_selection(&mut self, direction: isize) {
        let Some(field) = self.fields.get_mut(self.selected) else { return };
        match &mut field.kind {
            FormFieldKind::Boolean(value) => *value = !*value,
            FormFieldKind::Single { options, selected } if !options.is_empty() => {
                *selected = selected.saturating_add_signed(direction).min(options.len() - 1);
            }
            FormFieldKind::Multi { selected, .. } => {
                if let Some(value) = selected.first_mut() {
                    *value = !*value;
                }
            }
            _ => {}
        }
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

    fn render(&self, frame: &mut Frame, area: Rect, theme: &Theme) {
        let mut lines = vec![Line::styled(
            format!("Request from {}", self.server_name),
            Style::new().fg(theme.accent).add_modifier(Modifier::BOLD),
        )];
        if !self.message.is_empty() {
            lines.push(Line::raw(self.message.clone()));
        }
        for (index, field) in self.fields.iter().enumerate() {
            let marker = if index == self.selected { "›" } else { " " };
            let required = if field.required { " *" } else { "" };
            lines.push(Line::from(vec![
                Span::styled(format!("{marker} {}{required}: ", field.label), Style::new().fg(theme.heading)),
                Span::raw(field.display_value()),
            ]));
            if let Some(description) = &field.description {
                lines.push(Line::styled(format!("    {description}"), Style::new().fg(theme.muted)));
            }
        }
        if let Some(error) = &self.validation_error {
            lines.push(Line::styled(error.clone(), Style::new().fg(theme.error)));
        }
        lines.push(Line::styled("Enter submit · Esc cancel", Style::new().fg(theme.muted)));
        let block =
            Block::default().borders(Borders::ALL).title(" Elicitation ").border_style(Style::new().fg(theme.accent));
        frame.render_widget(Paragraph::new(Text::from(lines)).block(block).wrap(Wrap { trim: false }), area);
    }
}

impl FormField {
    fn from_schema(name: &str, schema: &Value, required: &[&str]) -> Self {
        let label = schema.get("title").and_then(Value::as_str).unwrap_or(name).to_string();
        let description = schema.get("description").and_then(Value::as_str).map(str::to_string);
        let kind = match schema.get("type").and_then(Value::as_str) {
            Some("boolean") => FormFieldKind::Boolean(schema.get("default").and_then(Value::as_bool).unwrap_or(false)),
            Some("integer" | "number") => FormFieldKind::Number(String::new()),
            Some("array") => {
                let options = schema
                    .pointer("/items/enum")
                    .and_then(Value::as_array)
                    .map(|values| string_values(values))
                    .unwrap_or_default();
                let defaults = schema
                    .get("default")
                    .and_then(Value::as_array)
                    .map(|values| string_values(values))
                    .unwrap_or_default();
                let selected = options.iter().map(|option| defaults.contains(option)).collect();
                FormFieldKind::Multi { options, selected }
            }
            _ if schema.get("enum").is_some() => {
                let options = schema
                    .get("enum")
                    .and_then(Value::as_array)
                    .map(|values| string_values(values))
                    .unwrap_or_default();
                let default = schema.get("default").and_then(Value::as_str);
                let selected = default.and_then(|value| options.iter().position(|option| option == value)).unwrap_or(0);
                FormFieldKind::Single { options, selected }
            }
            _ => FormFieldKind::Text(schema.get("default").and_then(Value::as_str).unwrap_or_default().to_string()),
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
                    Ok(if value.is_empty() { Value::Null } else { Value::String(value.clone()) })
                }
            }
            FormFieldKind::Number(value) => {
                if value.is_empty() {
                    return if self.required { Err(missing()) } else { Ok(Value::Null) };
                }
                serde_json::from_str(value).map_err(|_| format!("{} must be a number", self.label))
            }
            FormFieldKind::Boolean(value) => Ok(Value::Bool(*value)),
            FormFieldKind::Single { options, selected } => {
                let value = options.get(*selected).cloned();
                if self.required && value.is_none() {
                    Err(missing())
                } else {
                    Ok(value.map_or(Value::Null, Value::String))
                }
            }
            FormFieldKind::Multi { options, selected } => Ok(Value::Array(
                options
                    .iter()
                    .zip(selected)
                    .filter(|(_, selected)| **selected)
                    .map(|(value, _)| Value::String(value.clone()))
                    .collect(),
            )),
        }
    }

    fn display_value(&self) -> String {
        match &self.kind {
            FormFieldKind::Text(value) | FormFieldKind::Number(value) => value.clone(),
            FormFieldKind::Boolean(value) => if *value { "[x]" } else { "[ ]" }.to_string(),
            FormFieldKind::Single { options, selected } => options.get(*selected).cloned().unwrap_or_default(),
            FormFieldKind::Multi { options, selected } => options
                .iter()
                .zip(selected)
                .filter(|(_, selected)| **selected)
                .map(|(value, _)| value.as_str())
                .collect::<Vec<_>>()
                .join(", "),
        }
    }
}

impl UrlModal {
    fn render(&self, frame: &mut Frame, area: Rect, theme: &Theme) {
        let lines = vec![
            Line::styled(
                format!("Request from {}", self.server_name),
                Style::new().fg(theme.accent).add_modifier(Modifier::BOLD),
            ),
            Line::raw(self.message.clone()),
            Line::styled(self.url.clone(), Style::new().fg(theme.link)),
            Line::styled("Enter open browser · Esc cancel", Style::new().fg(theme.muted)),
        ];
        let block = Block::default()
            .borders(Borders::ALL)
            .title(" URL authorization ")
            .border_style(Style::new().fg(theme.accent));
        frame.render_widget(Paragraph::new(Text::from(lines)).block(block).wrap(Wrap { trim: false }), area);
    }
}

fn string_values(values: &[Value]) -> Vec<String> {
    values.iter().filter_map(Value::as_str).map(str::to_string).collect()
}

fn centered_rect(area: Rect, horizontal_percent: u16, vertical_percent: u16) -> Rect {
    let [vertical] =
        Layout::vertical([Constraint::Percentage(vertical_percent)]).flex(ratatui::layout::Flex::Center).areas(area);
    let [horizontal] = Layout::horizontal([Constraint::Percentage(horizontal_percent)])
        .flex(ratatui::layout::Flex::Center)
        .areas(vertical);
    horizontal
}

fn open_url(url: &str) -> Result<(), std::io::Error> {
    #[cfg(target_os = "macos")]
    let command = "open";
    #[cfg(target_os = "linux")]
    let command = "xdg-open";
    #[cfg(target_os = "windows")]
    let command = "cmd";
    #[cfg(target_os = "windows")]
    let args = ["/C", "start", url];
    #[cfg(not(target_os = "windows"))]
    let args = [url];
    std::process::Command::new(command).args(args).spawn().map(|_| ())
}
