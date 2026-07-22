use acp_utils::notifications::{
    CreateElicitationRequestParams, ElicitationAction, ElicitationParams, ElicitationResponse, McpNotification,
    UrlElicitationCompleteParams,
};
use acp_utils::{
    ConstTitle, ElicitationSchema, EnumSchema, MultiSelectEnumSchema, PrimitiveSchema, SingleSelectEnumSchema,
};
use agent_client_protocol::Responder;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Wrap};
use serde_json::{Map, Value};
use std::sync::Arc;

use crate::theme::Theme;

pub type BrowserOpener = Arc<dyn Fn(&str) -> Result<(), String> + Send + Sync>;
pub type ClipboardWriter = Arc<dyn Fn(&str) -> Result<(), String> + Send + Sync>;

pub struct ElicitationModal {
    kind: ModalKind,
    responder: Option<Responder<ElicitationResponse>>,
    browser_opener: BrowserOpener,
    clipboard_writer: ClipboardWriter,
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
    Single { options: Vec<SelectOption>, selected: usize },
    Multi { options: Vec<SelectOption>, selected: Vec<bool>, cursor: usize },
}

struct SelectOption {
    value: String,
    title: String,
}

struct UrlModal {
    server_name: String,
    elicitation_id: String,
    message: String,
    url: String,
    host: Option<String>,
    warnings: Vec<String>,
    launch_error: Option<String>,
    copy_message: Option<String>,
}

pub enum ModalOutcome {
    None,
    Close,
}

impl ElicitationModal {
    pub fn new(params: ElicitationParams, responder: Responder<ElicitationResponse>) -> Self {
        Self::with_url_handlers(params, responder, default_browser_opener(), default_clipboard_writer())
    }

    pub fn with_url_handlers(
        params: ElicitationParams,
        responder: Responder<ElicitationResponse>,
        browser_opener: BrowserOpener,
        clipboard_writer: ClipboardWriter,
    ) -> Self {
        let kind = match params.request {
            CreateElicitationRequestParams::FormElicitationParams { message, requested_schema, .. } => {
                ModalKind::Form(FormModal::new(params.server_name, message, &requested_schema))
            }
            CreateElicitationRequestParams::UrlElicitationParams { message, url, elicitation_id, .. } => {
                ModalKind::Url(UrlModal::new(params.server_name, elicitation_id, message, url))
            }
        };
        Self { kind, responder: Some(responder), browser_opener, clipboard_writer }
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
            ModalKind::Url(url) => {
                let plain_key = key.modifiers == KeyModifiers::NONE || key.modifiers == KeyModifiers::SHIFT;
                match key.code {
                    KeyCode::Esc => self.respond(ElicitationAction::Cancel, None),
                    KeyCode::Enter => {
                        match (self.browser_opener)(&url.url) {
                            Ok(()) => {}
                            Err(e) => {
                                url.launch_error = Some(format!("Failed to open browser: {e}"));
                            }
                        }
                        ModalOutcome::None
                    }
                    KeyCode::Char('c' | 'C') if plain_key => {
                        url.copy_message = Some(match (self.clipboard_writer)(&url.url) {
                            Ok(()) => "Copied URL to clipboard.".to_string(),
                            Err(e) => format!("Failed to copy URL: {e}"),
                        });
                        ModalOutcome::None
                    }
                    _ => ModalOutcome::None,
                }
            }
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

    pub fn needs_mouse_capture(&self) -> bool {
        matches!(self.kind, ModalKind::Form(_))
    }

    pub fn on_mouse_scroll_up(&mut self, _local_y: u16) {
        if let ModalKind::Form(form) = &mut self.kind {
            if let Some(field) = form.fields.get_mut(form.selected)
                && matches!(&field.kind, FormFieldKind::Multi { .. })
            {
                form.handle_multi_select_key(KeyEvent::new(KeyCode::Up, crossterm::event::KeyModifiers::NONE));
            } else {
                form.selected = form.selected.saturating_sub(1);
            }
        }
    }

    pub fn on_mouse_scroll_down(&mut self, _local_y: u16) {
        if let ModalKind::Form(form) = &mut self.kind {
            if let Some(field) = form.fields.get_mut(form.selected)
                && matches!(&field.kind, FormFieldKind::Multi { .. })
            {
                form.handle_multi_select_key(KeyEvent::new(KeyCode::Down, crossterm::event::KeyModifiers::NONE));
            } else {
                form.selected = (form.selected + 1).min(form.fields.len().saturating_sub(1));
            }
        }
    }

    pub fn on_mouse_click(&mut self, local_y: u16) {
        if let ModalKind::Form(form) = &mut self.kind {
            if form.fields.is_empty() {
                return;
            }
            if local_y < 3 {
                return;
            }
            let field_y = local_y.saturating_sub(3);
            let mut row = 0usize;
            for (index, field) in form.fields.iter().enumerate() {
                if row == field_y as usize {
                    form.selected = index;
                    if matches!(&field.kind, FormFieldKind::Boolean(_) | FormFieldKind::Single { .. }) {
                        form.change_selection(1);
                    } else if matches!(&field.kind, FormFieldKind::Multi { .. }) {
                        form.handle_multi_select_key(KeyEvent::new(
                            KeyCode::Char(' '),
                            crossterm::event::KeyModifiers::NONE,
                        ));
                    }
                    return;
                }
                row += 1;
                if let FormFieldKind::Multi { options, .. } = &field.kind {
                    row += options.len();
                }
                if let Some(ref desc) = field.description
                    && !desc.is_empty()
                {
                    row += 1;
                }
            }
        }
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

impl Drop for ElicitationModal {
    fn drop(&mut self) {
        if let Some(responder) = self.responder.take() {
            let _ = responder.respond(ElicitationResponse { action: ElicitationAction::Cancel, content: None });
        }
    }
}

enum FormAction {
    None,
    Cancel,
    Accept(Value),
}

impl FormModal {
    fn new(server_name: String, message: String, schema: &ElicitationSchema) -> Self {
        let required: Vec<&str> = schema.required.as_deref().unwrap_or(&[]).iter().map(String::as_str).collect();
        let fields =
            schema.properties.iter().map(|(name, prop)| FormField::from_primitive(name, prop, &required)).collect();
        Self { server_name, message, fields, selected: 0, validation_error: None }
    }

    fn on_key(&mut self, key: KeyEvent) -> FormAction {
        // If inside a multi-select's options, handle navigation within that field
        if let Some(field) = self.fields.get_mut(self.selected)
            && matches!(&field.kind, FormFieldKind::Multi { .. })
            && self.handle_multi_select_key(key)
        {
            return FormAction::None;
        }

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

    fn handle_multi_select_key(&mut self, key: KeyEvent) -> bool {
        let Some(field) = self.fields.get_mut(self.selected) else { return false };
        let FormFieldKind::Multi { options, selected, cursor } = &mut field.kind else { return false };

        match key.code {
            KeyCode::Up => {
                *cursor = cursor.saturating_sub(1);
                true
            }
            KeyCode::Down => {
                *cursor = (*cursor + 1).min(options.len().saturating_sub(1));
                true
            }
            KeyCode::Char(' ') => {
                if let Some(sel) = selected.get_mut(*cursor) {
                    *sel = !*sel;
                }
                true
            }
            _ => false,
        }
    }

    fn change_selection(&mut self, direction: isize) {
        let Some(field) = self.fields.get_mut(self.selected) else { return };
        match &mut field.kind {
            FormFieldKind::Boolean(value) => *value = !*value,
            FormFieldKind::Single { options, selected } if !options.is_empty() => {
                *selected = selected.saturating_add_signed(direction).min(options.len() - 1);
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
        let block =
            Block::default().borders(Borders::ALL).title(" Elicitation ").border_style(Style::new().fg(theme.accent));
        frame.render_widget(Paragraph::new(Text::from(lines)).block(block).wrap(Wrap { trim: false }), area);
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
                (label, description, FormFieldKind::Number(default_str))
            }
            PrimitiveSchema::Number(n) => {
                let label = n.title.as_deref().unwrap_or(name).to_string();
                let description = n.description.as_deref().map(str::to_string);
                let default_str = n.default.map(|d| d.to_string()).unwrap_or_default();
                (label, description, FormFieldKind::Number(default_str))
            }
            PrimitiveSchema::String(s) => {
                let label = s.title.as_deref().unwrap_or(name).to_string();
                let description = s.description.as_deref().map(str::to_string);
                let default_str = s.default.clone().unwrap_or_default();
                (label, description, FormFieldKind::Text(default_str))
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

    fn display_value(&self) -> String {
        match &self.kind {
            FormFieldKind::Text(value) | FormFieldKind::Number(value) => value.clone(),
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

impl UrlModal {
    fn new(server_name: String, elicitation_id: String, message: String, url: String) -> Self {
        let parsed_url = url::Url::parse(&url);
        let host = parsed_url.as_ref().ok().and_then(|parsed| parsed.host_str().map(std::string::ToString::to_string));

        let mut warnings = Vec::new();
        match parsed_url {
            Ok(parsed_url) => {
                if !parsed_url.username().is_empty() || parsed_url.password().is_some() {
                    warnings.push(
                        "Warning: URL contains embedded credentials. These may be visible to the server.".to_string(),
                    );
                }
                if let Some(ref h) = host
                    && h.contains("xn--")
                {
                    warnings.push(
                        "Warning: URL contains punycode (internationalized domain). Verify the domain before proceeding."
                            .to_string(),
                    );
                }
                if parsed_url.scheme() != "https" && !is_local_http_url(&parsed_url) {
                    warnings.push("Warning: URL does not use HTTPS.".to_string());
                }
            }
            Err(_) => {
                warnings.push("Warning: URL could not be parsed. Verify it carefully before proceeding.".to_string());
            }
        }

        Self { server_name, elicitation_id, message, url, host, warnings, launch_error: None, copy_message: None }
    }

    fn render(&self, frame: &mut Frame, area: Rect, theme: &Theme) {
        let mut lines = vec![
            Line::styled(
                format!("Request from {}", self.server_name),
                Style::new().fg(theme.accent).add_modifier(Modifier::BOLD),
            ),
            Line::raw(self.message.clone()),
        ];

        if let Some(ref host) = self.host {
            lines.push(Line::styled(format!("Host: {host}"), Style::new().fg(theme.muted)));
        }

        if !self.warnings.is_empty() {
            lines.push(Line::raw(""));
            for warning in &self.warnings {
                lines.push(Line::styled(warning.clone(), Style::new().fg(theme.warning)));
            }
        }

        if let Some(ref message) = self.copy_message {
            lines.push(Line::raw(""));
            lines.push(Line::styled(message.clone(), Style::new().fg(theme.muted)));
        }

        if let Some(ref error) = self.launch_error {
            lines.push(Line::raw(""));
            lines.push(Line::styled(error.clone(), Style::new().fg(theme.error)));
        }

        lines.push(Line::styled("Enter open browser · c copy URL · Esc cancel", Style::new().fg(theme.muted)));
        let block = Block::default()
            .borders(Borders::ALL)
            .title(" URL authorization ")
            .border_style(Style::new().fg(theme.accent));
        frame.render_widget(Paragraph::new(Text::from(lines)).block(block).wrap(Wrap { trim: false }), area);
    }
}

fn is_local_http_url(url: &url::Url) -> bool {
    if url.scheme() != "http" {
        return false;
    }
    matches!(url.host_str(), Some("localhost" | "127.0.0.1" | "::1"))
}

fn centered_rect(area: Rect, horizontal_percent: u16, vertical_percent: u16) -> Rect {
    let [vertical] =
        Layout::vertical([Constraint::Percentage(vertical_percent)]).flex(ratatui::layout::Flex::Center).areas(area);
    let [horizontal] = Layout::horizontal([Constraint::Percentage(horizontal_percent)])
        .flex(ratatui::layout::Flex::Center)
        .areas(vertical);
    horizontal
}

pub fn default_browser_opener() -> BrowserOpener {
    Arc::new(|url: &str| -> Result<(), String> {
        #[cfg(target_os = "macos")]
        {
            let status = std::process::Command::new("open")
                .arg(url)
                .status()
                .map_err(|e| format!("Failed to spawn 'open': {e}"))?;
            status.success().then_some(()).ok_or_else(|| format!("'open' exited with status {status}"))
        }
        #[cfg(target_os = "linux")]
        {
            let status = std::process::Command::new("xdg-open")
                .arg(url)
                .status()
                .map_err(|e| format!("Failed to spawn 'xdg-open': {e}"))?;
            status.success().then_some(()).ok_or_else(|| format!("'xdg-open' exited with status {status}"))
        }
        #[cfg(target_os = "windows")]
        {
            let status = std::process::Command::new("cmd")
                .args(["/C", "start", url])
                .status()
                .map_err(|e| format!("Failed to spawn 'start': {e}"))?;
            status.success().then_some(()).ok_or_else(|| format!("'start' exited with status {status}"))
        }
        #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
        Err("Unsupported platform for opening URLs".to_string())
    })
}

pub fn default_clipboard_writer() -> ClipboardWriter {
    Arc::new(|text: &str| -> Result<(), String> {
        #[cfg(target_os = "macos")]
        {
            cmd_clipboard("pbcopy", &[], text)
        }
        #[cfg(target_os = "linux")]
        {
            cmd_clipboard("wl-copy", &[], text)
                .or_else(|_| cmd_clipboard("xclip", &["-selection", "clipboard"], text))
                .or_else(|_| cmd_clipboard("xsel", &["--clipboard", "--input"], text))
                .or_else(|_| Err("No clipboard tool found (wl-copy, xclip, or xsel)".to_string()))
        }
        #[cfg(target_os = "windows")]
        {
            cmd_clipboard("clip", &[], text)
        }
        #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
        Err("Unsupported platform for copying URLs".to_string())
    })
}

fn cmd_clipboard(command: &str, args: &[&str], text: &str) -> Result<(), String> {
    use std::io::Write;
    let mut child = std::process::Command::new(command)
        .args(args)
        .stdin(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| format!("Failed to spawn '{command}': {e}"))?;
    child
        .stdin
        .as_mut()
        .ok_or_else(|| format!("'{command}' has no stdin"))?
        .write_all(text.as_bytes())
        .map_err(|e| format!("Failed to write to '{command}': {e}"))?;
    let status = child.wait().map_err(|e| format!("Failed to wait for '{command}': {e}"))?;
    status.success().then_some(()).ok_or_else(|| format!("'{command}' exited with status {status}"))
}

#[cfg(test)]
#[allow(clippy::absolute_paths, clippy::similar_names)]
mod tests {
    use super::*;
    use acp_utils::testing::test_connection;
    use std::sync::atomic::{AtomicBool, Ordering};
    use tokio::task::LocalSet;

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

    fn noop_handlers() -> (BrowserOpener, ClipboardWriter) {
        (Arc::new(|_| Ok(())), Arc::new(|_| Ok(())))
    }

    fn failing_handlers() -> (BrowserOpener, ClipboardWriter) {
        (
            Arc::new(|_| Err("simulated open failure".to_string())),
            Arc::new(|_| Err("simulated copy failure".to_string())),
        )
    }

    async fn make_modal_for_schema(
        schema: ElicitationSchema,
    ) -> (ElicitationModal, tokio::sync::oneshot::Receiver<ElicitationResponse>) {
        let (cx, mut peer) = test_connection().await;
        let (responder, rx) = peer.fake_elicitation(&cx).await;
        let params = ElicitationParams {
            server_name: "test".into(),
            request: CreateElicitationRequestParams::FormElicitationParams {
                meta: None,
                message: String::new(),
                requested_schema: schema,
            },
        };
        let (opener, writer) = noop_handlers();
        (ElicitationModal::with_url_handlers(params, responder, opener, writer), rx)
    }

    async fn make_url_modal(url: &str) -> (ElicitationModal, tokio::sync::oneshot::Receiver<ElicitationResponse>) {
        let (cx, mut peer) = test_connection().await;
        let (responder, rx) = peer.fake_elicitation(&cx).await;
        let params = ElicitationParams {
            server_name: "github".into(),
            request: CreateElicitationRequestParams::UrlElicitationParams {
                meta: None,
                message: "Authorize GitHub".into(),
                url: url.into(),
                elicitation_id: "el-1".into(),
            },
        };
        let (opener, writer) = noop_handlers();
        (ElicitationModal::with_url_handlers(params, responder, opener, writer), rx)
    }

    async fn make_url_modal_with_handlers(
        url: &str,
        opener: BrowserOpener,
        writer: ClipboardWriter,
    ) -> ElicitationModal {
        let (cx, mut peer) = test_connection().await;
        let (responder, _rx) = peer.fake_elicitation(&cx).await;
        let params = ElicitationParams {
            server_name: "github".into(),
            request: CreateElicitationRequestParams::UrlElicitationParams {
                meta: None,
                message: "Authorize GitHub".into(),
                url: url.into(),
                elicitation_id: "el-1".into(),
            },
        };
        ElicitationModal::with_url_handlers(params, responder, opener, writer)
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
                assert!((parsed - 2.5).abs() < 0.001, "expected ~2.5, got {value}");
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
            kind: FormFieldKind::Text(String::new()),
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
            kind: FormFieldKind::Text(String::new()),
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

    // ── Permission-like single-field ──

    #[tokio::test(flavor = "current_thread")]
    async fn permission_like_form_returns_default_on_enter() {
        LocalSet::new()
            .run_until(async {
                let schema = permission_like_schema();
                let (mut modal, rx) = make_modal_for_schema(schema).await;
                assert!(matches!(modal.on_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)), ModalOutcome::Close));
                let response = rx.await.unwrap();
                assert_eq!(response.action, ElicitationAction::Accept);
                assert_eq!(response.content.unwrap()["decision"], "deny");
            })
            .await;
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

    // ── Cancel ──

    #[tokio::test(flavor = "current_thread")]
    async fn esc_returns_cancel() {
        LocalSet::new()
            .run_until(async {
                let schema = ElicitationSchema::builder().build().unwrap();
                let (mut modal, rx) = make_modal_for_schema(schema).await;
                assert!(matches!(modal.on_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)), ModalOutcome::Close));
                let response = rx.await.unwrap();
                assert_eq!(response.action, ElicitationAction::Cancel);
            })
            .await;
    }

    // ── Drop cancellation ──

    #[tokio::test(flavor = "current_thread")]
    async fn dropping_modal_responds_cancel() {
        LocalSet::new()
            .run_until(async {
                let schema = ElicitationSchema::builder().build().unwrap();
                let (modal, rx) = make_modal_for_schema(schema).await;
                drop(modal);
                let response = rx.await.unwrap();
                assert_eq!(response.action, ElicitationAction::Cancel);
            })
            .await;
    }

    // ── URL modal ──

    #[test]
    fn url_modal_parses_host() {
        let url = UrlModal::new("github".into(), "el-1".into(), "Auth".into(), "https://github.com/login".into());
        assert_eq!(url.host.as_deref(), Some("github.com"));
        assert!(url.warnings.is_empty());
    }

    #[test]
    fn url_modal_warns_on_non_https() {
        let url = UrlModal::new("test".into(), "el-1".into(), "Open".into(), "http://example.com/form".into());
        assert_eq!(url.warnings.len(), 1);
        assert!(url.warnings[0].contains("HTTPS"));
    }

    #[test]
    fn url_modal_does_not_warn_on_localhost() {
        let url = UrlModal::new("test".into(), "el-1".into(), "Local".into(), "http://localhost:3000/auth".into());
        assert!(url.warnings.is_empty());
    }

    #[test]
    fn url_modal_allows_127_0_0_1_as_localhost() {
        let url = UrlModal::new("test".into(), "el-1".into(), "Local".into(), "http://127.0.0.1:8000/api".into());
        assert!(url.warnings.is_empty());
    }

    #[test]
    fn url_modal_warns_on_invalid_url() {
        let url = UrlModal::new("test".into(), "el-invalid".into(), "Check".into(), "not a valid url".into());
        assert!(url.host.is_none());
        assert!(url.warnings.iter().any(|w| w.contains("could not be parsed")));
    }

    #[test]
    fn url_modal_warns_on_punycode() {
        let url = UrlModal::new("test".into(), "el-1".into(), "Phish".into(), "https://xn--e1afmkfd.xn--p1ai/".into());
        assert_eq!(url.warnings.len(), 1);
        assert!(url.warnings[0].contains("punycode"));
    }

    #[test]
    fn url_modal_warns_on_punycode_and_non_https() {
        let url = UrlModal::new("test".into(), "el-1".into(), "Both".into(), "http://xn--e1afmkfd.xn--p1ai/".into());
        assert_eq!(url.warnings.len(), 2);
        assert!(url.warnings.iter().any(|w| w.contains("punycode")));
        assert!(url.warnings.iter().any(|w| w.contains("HTTPS")));
    }

    #[test]
    fn url_modal_warns_on_embedded_credentials() {
        let url =
            UrlModal::new("test".into(), "el-1".into(), "Auth".into(), "https://user:pass@example.com/path".into());
        assert!(url.warnings.iter().any(|w| w.contains("credentials")), "warnings: {:?}", url.warnings);
    }

    #[test]
    fn url_modal_no_credential_warning_for_clean_url() {
        let url = UrlModal::new("test".into(), "el-1".into(), "Auth".into(), "https://example.com/path".into());
        assert!(!url.warnings.iter().any(|w| w.contains("credentials")), "warnings: {:?}", url.warnings);
    }

    // ── URL completion correlation ──

    #[tokio::test(flavor = "current_thread")]
    async fn url_enter_opens_browser_and_keeps_modal_open() {
        LocalSet::new()
            .run_until(async {
                let opened = Arc::new(AtomicBool::new(false));
                let url_opener: BrowserOpener = {
                    let opened = opened.clone();
                    Arc::new(move |_| {
                        opened.store(true, Ordering::SeqCst);
                        Ok(())
                    })
                };
                let mut modal =
                    make_url_modal_with_handlers("https://github.com/login", url_opener, noop_handlers().1).await;
                let outcome = modal.on_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
                assert!(matches!(outcome, ModalOutcome::None));
            })
            .await;
    }

    #[tokio::test(flavor = "current_thread")]
    async fn url_enter_shows_error_on_open_failure() {
        LocalSet::new()
            .run_until(async {
                let mut modal =
                    make_url_modal_with_handlers("https://github.com/login", failing_handlers().0, noop_handlers().1)
                        .await;
                modal.on_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
                match &modal.kind {
                    ModalKind::Url(url) => {
                        assert!(url.launch_error.as_deref().unwrap().contains("simulated open failure"));
                    }
                    ModalKind::Form(_) => panic!("expected Url"),
                }
            })
            .await;
    }

    #[tokio::test(flavor = "current_thread")]
    async fn url_c_copies_to_clipboard() {
        LocalSet::new()
            .run_until(async {
                let copied: Arc<std::sync::Mutex<String>> = Arc::new(std::sync::Mutex::new(String::new()));
                let writer: ClipboardWriter = {
                    let copied = copied.clone();
                    Arc::new(move |text: &str| {
                        *copied.lock().unwrap() = text.to_string();
                        Ok(())
                    })
                };
                let mut modal =
                    make_url_modal_with_handlers("https://github.com/login", noop_handlers().0, writer).await;
                modal.on_key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::NONE));
                match &modal.kind {
                    ModalKind::Url(url) => {
                        assert_eq!(url.copy_message.as_deref(), Some("Copied URL to clipboard."));
                    }
                    ModalKind::Form(_) => panic!("expected Url"),
                }
            })
            .await;
    }

    #[tokio::test(flavor = "current_thread")]
    async fn url_copy_shows_error_on_failure() {
        LocalSet::new()
            .run_until(async {
                let mut modal =
                    make_url_modal_with_handlers("https://github.com/login", noop_handlers().0, failing_handlers().1)
                        .await;
                modal.on_key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::NONE));
                match &modal.kind {
                    ModalKind::Url(url) => {
                        assert!(url.copy_message.as_deref().unwrap().contains("simulated copy failure"));
                    }
                    ModalKind::Form(_) => panic!("expected Url"),
                }
            })
            .await;
    }

    // ── URL completion correlation ──

    #[tokio::test(flavor = "current_thread")]
    async fn url_completion_matches_on_server_name_and_elicitation_id() {
        LocalSet::new()
            .run_until(async {
                let (mut modal, _rx) = make_url_modal("https://github.com").await;
                let matched =
                    modal.on_notification(&McpNotification::UrlElicitationComplete(UrlElicitationCompleteParams {
                        server_name: "github".into(),
                        elicitation_id: "el-1".into(),
                    }));
                assert!(matches!(matched, ModalOutcome::Close));
            })
            .await;
    }

    #[tokio::test(flavor = "current_thread")]
    async fn url_completion_ignores_unrelated_server() {
        LocalSet::new()
            .run_until(async {
                let (mut modal, _rx) = make_url_modal("https://github.com").await;
                let matched =
                    modal.on_notification(&McpNotification::UrlElicitationComplete(UrlElicitationCompleteParams {
                        server_name: "other".into(),
                        elicitation_id: "el-1".into(),
                    }));
                assert!(matches!(matched, ModalOutcome::None));
            })
            .await;
    }

    #[tokio::test(flavor = "current_thread")]
    async fn url_completion_ignores_unrelated_elicitation_id() {
        LocalSet::new()
            .run_until(async {
                let (mut modal, _rx) = make_url_modal("https://github.com").await;
                let matched =
                    modal.on_notification(&McpNotification::UrlElicitationComplete(UrlElicitationCompleteParams {
                        server_name: "github".into(),
                        elicitation_id: "el-other".into(),
                    }));
                assert!(matches!(matched, ModalOutcome::None));
            })
            .await;
    }

    #[tokio::test(flavor = "current_thread")]
    async fn url_completion_ignored_for_form_modal() {
        LocalSet::new()
            .run_until(async {
                let schema = ElicitationSchema::builder().build().unwrap();
                let (mut modal, _rx) = make_modal_for_schema(schema).await;
                let matched =
                    modal.on_notification(&McpNotification::UrlElicitationComplete(UrlElicitationCompleteParams {
                        server_name: "test".into(),
                        elicitation_id: "ignored".into(),
                    }));
                assert!(matches!(matched, ModalOutcome::None));
            })
            .await;
    }

    // ── URL cancel ──

    #[tokio::test(flavor = "current_thread")]
    async fn url_esc_cancels() {
        LocalSet::new()
            .run_until(async {
                let (mut modal, rx) = make_url_modal("https://github.com/login").await;
                assert!(matches!(modal.on_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)), ModalOutcome::Close));
                let response = rx.await.unwrap();
                assert_eq!(response.action, ElicitationAction::Cancel);
            })
            .await;
    }

    #[tokio::test(flavor = "current_thread")]
    async fn url_drop_sends_cancel() {
        LocalSet::new()
            .run_until(async {
                let (modal, rx) = make_url_modal("https://github.com/login").await;
                drop(modal);
                let response = rx.await.unwrap();
                assert_eq!(response.action, ElicitationAction::Cancel);
            })
            .await;
    }
}
