use acp_utils::notifications::{
    CreateElicitationRequestParams, ElicitationAction, ElicitationParams, ElicitationResponse,
    UrlElicitationCompleteParams,
};
use acp_utils::{
    ConstTitle, ElicitationSchema, EnumSchema, MultiSelectEnumSchema, PrimitiveSchema, SingleSelectEnumSchema,
};
use agent_client_protocol::Responder;
use std::io::Write;
use std::process::{Command, Stdio};
use std::sync::Arc;
use tui::{
    Checkbox, Component, Event, Form, FormField, FormFieldKind, FormMessage, Frame, KeyCode, KeyEvent, KeyModifiers,
    MultiSelect, NumberField, RadioSelect, SelectOption, TextField, ViewContext,
};

pub enum ElicitationMessage {
    Responded,
    /// Emitted when a URL modal successfully opens the browser.
    UrlOpened {
        elicitation_id: String,
        server_name: String,
    },
}

pub enum ElicitationUi {
    Form(Form),
    Url(UrlPrompt),
}

pub struct UrlPrompt {
    pub server_name: String,
    pub elicitation_id: String,
    pub message: String,
    pub url: String,
    pub host: Option<String>,
    pub warnings: Vec<String>,
    pub launch_error: Option<String>,
    pub copy_message: Option<String>,
}

pub enum UrlPromptOutcome {
    Opened,
    Copied,
    Cancelled,
}

pub type BrowserOpener = Arc<dyn Fn(&str) -> Result<(), String> + Send + Sync>;
pub type ClipboardWriter = Arc<dyn Fn(&str) -> Result<(), String> + Send + Sync>;

pub struct ElicitationForm {
    pub ui: ElicitationUi,
    browser_opener: BrowserOpener,
    clipboard_writer: ClipboardWriter,
    responder: Option<Responder<ElicitationResponse>>,
}

impl UrlPrompt {
    pub fn new(server_name: String, elicitation_id: String, message: String, url: String) -> Self {
        let parsed_url = url::Url::parse(&url);
        let host = parsed_url.as_ref().ok().and_then(|parsed| parsed.host_str().map(std::string::ToString::to_string));

        let mut warnings = Vec::new();
        match parsed_url {
            Ok(parsed_url) => {
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

    pub fn on_key(
        &mut self,
        key: &KeyEvent,
        browser_opener: &BrowserOpener,
        clipboard_writer: &ClipboardWriter,
    ) -> Option<UrlPromptOutcome> {
        let plain_key = key.modifiers == KeyModifiers::NONE || key.modifiers == KeyModifiers::SHIFT;
        match key.code {
            KeyCode::Enter => match browser_opener(&self.url) {
                Ok(()) => Some(UrlPromptOutcome::Opened),
                Err(e) => {
                    self.launch_error = Some(format!("Failed to open browser: {e}"));
                    None
                }
            },
            KeyCode::Char('c' | 'C') if plain_key => {
                self.copy_message = Some(match clipboard_writer(&self.url) {
                    Ok(()) => "Copied URL to clipboard.".to_string(),
                    Err(e) => format!("Failed to copy URL: {e}"),
                });
                Some(UrlPromptOutcome::Copied)
            }
            KeyCode::Esc => Some(UrlPromptOutcome::Cancelled),
            _ => None,
        }
    }
}

impl Component for ElicitationForm {
    type Message = ElicitationMessage;

    async fn on_event(&mut self, event: &Event) -> Option<Vec<Self::Message>> {
        match &mut self.ui {
            ElicitationUi::Form(form) => {
                let outcome = form.on_event(event).await?;
                if let Some(msg) = outcome.into_iter().next() {
                    match msg {
                        FormMessage::Close => {
                            let _ = self.responder.take().map(|r| r.respond(Self::cancel()));
                            return Some(vec![ElicitationMessage::Responded]);
                        }
                        FormMessage::Submit => {
                            let response = self.confirm();
                            let _ = self.responder.take().map(|r| r.respond(response));
                            return Some(vec![ElicitationMessage::Responded]);
                        }
                    }
                }
                Some(vec![])
            }
            ElicitationUi::Url(prompt) => {
                let Event::Key(key) = event else {
                    return Some(vec![]);
                };
                let Some(outcome) = prompt.on_key(key, &self.browser_opener, &self.clipboard_writer) else {
                    return Some(vec![]);
                };
                match outcome {
                    UrlPromptOutcome::Opened => Some(vec![ElicitationMessage::UrlOpened {
                        elicitation_id: prompt.elicitation_id.clone(),
                        server_name: prompt.server_name.clone(),
                    }]),
                    UrlPromptOutcome::Copied => Some(vec![]),
                    UrlPromptOutcome::Cancelled => {
                        let _ = self.responder.take().map(|r| r.respond(Self::cancel()));
                        Some(vec![ElicitationMessage::Responded])
                    }
                }
            }
        }
    }

    fn render(&mut self, ctx: &ViewContext) -> Frame {
        match &mut self.ui {
            ElicitationUi::Form(form) => form.render(ctx),
            ElicitationUi::Url(prompt) => render_url_prompt(prompt, ctx),
        }
    }
}

impl ElicitationForm {
    pub fn from_params(params: ElicitationParams, responder: Responder<ElicitationResponse>) -> Self {
        Self::with_url_handlers(params, responder, default_browser_opener, default_clipboard_writer)
    }

    pub fn with_browser_opener<T>(
        params: ElicitationParams,
        responder: Responder<ElicitationResponse>,
        browser_opener: T,
    ) -> Self
    where
        T: Fn(&str) -> Result<(), String> + Send + Sync + 'static,
    {
        Self::with_url_handlers(params, responder, browser_opener, default_clipboard_writer)
    }

    pub fn with_url_handlers<T, U>(
        params: ElicitationParams,
        responder: Responder<ElicitationResponse>,
        browser_opener: T,
        clipboard_writer: U,
    ) -> Self
    where
        T: Fn(&str) -> Result<(), String> + Send + Sync + 'static,
        U: Fn(&str) -> Result<(), String> + Send + Sync + 'static,
    {
        let ui = match params.request {
            CreateElicitationRequestParams::FormElicitationParams { message, requested_schema, .. } => {
                let fields = parse_schema(&requested_schema);
                ElicitationUi::Form(Form::new(message, fields))
            }
            CreateElicitationRequestParams::UrlElicitationParams { message, url, elicitation_id, .. } => {
                ElicitationUi::Url(UrlPrompt::new(params.server_name, elicitation_id, message, url))
            }
        };
        Self {
            ui,
            browser_opener: Arc::new(browser_opener),
            clipboard_writer: Arc::new(clipboard_writer),
            responder: Some(responder),
        }
    }

    pub fn confirm(&self) -> ElicitationResponse {
        match &self.ui {
            ElicitationUi::Form(form) => {
                ElicitationResponse { action: ElicitationAction::Accept, content: Some(form.to_json()) }
            }
            ElicitationUi::Url(_) => ElicitationResponse { action: ElicitationAction::Accept, content: None },
        }
    }

    pub fn cancel() -> ElicitationResponse {
        ElicitationResponse { action: ElicitationAction::Cancel, content: None }
    }

    /// Send `Cancel` to the responder if one is still attached. Used when the
    /// owning container is displacing this form before the user responded.
    pub fn cancel_pending(&mut self) {
        if let Some(responder) = self.responder.take() {
            let _ = responder.respond(Self::cancel());
        }
    }

    /// If this form is showing the URL prompt that `params` refers to, accept
    /// it and consume the responder. Returns true iff the form was answered.
    pub fn accept_url_complete(&mut self, params: &UrlElicitationCompleteParams) -> bool {
        let ElicitationUi::Url(prompt) = &self.ui else {
            return false;
        };
        if prompt.server_name != params.server_name || prompt.elicitation_id != params.elicitation_id {
            return false;
        }
        let response = self.confirm();
        if let Some(responder) = self.responder.take() {
            let _ = responder.respond(response);
        }
        true
    }
}

pub fn render_url_prompt(prompt: &UrlPrompt, ctx: &ViewContext) -> Frame {
    use tui::{Line, Style};

    let mut lines = Vec::new();
    let text_primary = ctx.theme.text_primary();
    let text_secondary = ctx.theme.text_secondary();
    let warning_color = ctx.theme.warning();
    lines.push(Line::default());
    lines.push(Line::with_style(&prompt.message, Style::fg(text_primary)));

    if let Some(ref host) = prompt.host {
        lines.push(Line::with_style(format!("Host: {host}"), Style::fg(text_secondary)));
    }

    if !prompt.warnings.is_empty() {
        lines.push(Line::default());
        for warning in &prompt.warnings {
            lines.push(Line::styled(warning, warning_color));
        }
    }

    if let Some(ref message) = prompt.copy_message {
        lines.push(Line::default());
        lines.push(Line::with_style(message, Style::fg(text_secondary)));
    }

    if let Some(ref error) = prompt.launch_error {
        lines.push(Line::default());
        lines.push(Line::styled(error, ctx.theme.error()));
    }

    Frame::new(lines)
}

fn is_local_http_url(url: &url::Url) -> bool {
    if url.scheme() != "http" {
        return false;
    }

    matches!(url.host_str(), Some("localhost" | "127.0.0.1" | "::1"))
}

fn default_browser_opener(url: &str) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        let status = Command::new("open").arg(url).status().map_err(|e| e.to_string())?;
        return status.success().then_some(()).ok_or_else(|| format!("open exited with status {status}"));
    }

    #[cfg(target_os = "linux")]
    {
        let status = Command::new("xdg-open").arg(url).status().map_err(|e| e.to_string())?;
        return status.success().then_some(()).ok_or_else(|| format!("xdg-open exited with status {status}"));
    }

    #[cfg(target_os = "windows")]
    {
        let status = Command::new("cmd").args(["/C", "start", url]).status().map_err(|e| e.to_string())?;
        return status.success().then_some(()).ok_or_else(|| format!("start exited with status {status}"));
    }

    #[allow(unreachable_code)]
    Err("Unsupported platform for opening URLs".to_string())
}

fn default_clipboard_writer(text: &str) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        return cmd("pbcopy", &[], text);
    }

    #[cfg(target_os = "linux")]
    {
        return cmd("wl-copy", &[], text)
            .or_else(|_| cmd("xclip", &["-selection", "clipboard"], text))
            .or_else(|_| cmd("xsel", &["--clipboard", "--input"], text));
    }

    #[cfg(target_os = "windows")]
    {
        return cmd("clip", &[], text);
    }

    #[allow(unreachable_code)]
    Err("Unsupported platform for copying URLs".to_string())
}

fn cmd(command: &str, args: &[&str], text: &str) -> Result<(), String> {
    let mut child = Command::new(command).args(args).stdin(Stdio::piped()).spawn().map_err(|e| e.to_string())?;
    child
        .stdin
        .as_mut()
        .ok_or_else(|| format!("{command} stdin unavailable"))?
        .write_all(text.as_bytes())
        .map_err(|e| e.to_string())?;
    let status = child.wait().map_err(|e| e.to_string())?;
    status.success().then_some(()).ok_or_else(|| format!("{command} exited with status {status}"))
}

fn parse_schema(schema: &ElicitationSchema) -> Vec<FormField> {
    let required = schema.required.as_deref().unwrap_or(&[]);
    schema
        .properties
        .iter()
        .map(|(name, prop)| {
            let (title, description) = extract_metadata(prop);
            FormField {
                name: name.clone(),
                label: title.unwrap_or_else(|| name.clone()),
                description,
                required: required.iter().any(|r| r == name),
                kind: parse_field_kind(prop),
            }
        })
        .collect()
}

fn parse_field_kind(prop: &PrimitiveSchema) -> FormFieldKind {
    match prop {
        PrimitiveSchema::Boolean(b) => FormFieldKind::Boolean(Checkbox::new(b.default.unwrap_or(false))),
        PrimitiveSchema::Integer(_) => FormFieldKind::Number(NumberField::new(String::new(), true)),
        PrimitiveSchema::Number(_) => FormFieldKind::Number(NumberField::new(String::new(), false)),
        PrimitiveSchema::String(_) => FormFieldKind::Text(TextField::new(String::new())),
        PrimitiveSchema::Enum(e) => parse_enum_field(e),
    }
}

fn parse_enum_field(e: &EnumSchema) -> FormFieldKind {
    match e {
        EnumSchema::Single(s) => match s {
            SingleSelectEnumSchema::Untitled(u) => {
                let options = options_from_strings(&u.enum_);
                let default_idx =
                    u.default.as_ref().and_then(|d| options.iter().position(|o| o.value == *d)).unwrap_or(0);
                FormFieldKind::SingleSelect(RadioSelect::new(options, default_idx))
            }
            SingleSelectEnumSchema::Titled(t) => {
                let options = options_from_const_titles(&t.one_of);
                let default_idx =
                    t.default.as_ref().and_then(|d| options.iter().position(|o| o.value == *d)).unwrap_or(0);
                FormFieldKind::SingleSelect(RadioSelect::new(options, default_idx))
            }
        },
        EnumSchema::Multi(m) => match m {
            MultiSelectEnumSchema::Untitled(u) => {
                let options = options_from_strings(&u.items.enum_);
                let defaults = u.default.as_deref().unwrap_or(&[]);
                let selected: Vec<bool> = options.iter().map(|o| defaults.contains(&o.value)).collect();
                FormFieldKind::MultiSelect(MultiSelect::new(options, selected))
            }
            MultiSelectEnumSchema::Titled(t) => {
                let options = options_from_const_titles(&t.items.any_of);
                let defaults = t.default.as_deref().unwrap_or(&[]);
                let selected: Vec<bool> = options.iter().map(|o| defaults.contains(&o.value)).collect();
                FormFieldKind::MultiSelect(MultiSelect::new(options, selected))
            }
        },
        EnumSchema::Legacy(l) => {
            let options = options_from_strings(&l.enum_);
            FormFieldKind::SingleSelect(RadioSelect::new(options, 0))
        }
    }
}

fn extract_metadata(prop: &PrimitiveSchema) -> (Option<String>, Option<String>) {
    match prop {
        PrimitiveSchema::String(s) => {
            (s.title.as_ref().map(ToString::to_string), s.description.as_ref().map(ToString::to_string))
        }
        PrimitiveSchema::Number(n) => {
            (n.title.as_ref().map(ToString::to_string), n.description.as_ref().map(ToString::to_string))
        }
        PrimitiveSchema::Integer(i) => {
            (i.title.as_ref().map(ToString::to_string), i.description.as_ref().map(ToString::to_string))
        }
        PrimitiveSchema::Boolean(b) => {
            (b.title.as_ref().map(ToString::to_string), b.description.as_ref().map(ToString::to_string))
        }
        PrimitiveSchema::Enum(e) => extract_enum_metadata(e),
    }
}

fn extract_enum_metadata(e: &EnumSchema) -> (Option<String>, Option<String>) {
    match e {
        EnumSchema::Single(s) => match s {
            SingleSelectEnumSchema::Untitled(u) => {
                (u.title.as_ref().map(ToString::to_string), u.description.as_ref().map(ToString::to_string))
            }
            SingleSelectEnumSchema::Titled(t) => {
                (t.title.as_ref().map(ToString::to_string), t.description.as_ref().map(ToString::to_string))
            }
        },
        EnumSchema::Multi(m) => match m {
            MultiSelectEnumSchema::Untitled(u) => {
                (u.title.as_ref().map(ToString::to_string), u.description.as_ref().map(ToString::to_string))
            }
            MultiSelectEnumSchema::Titled(t) => {
                (t.title.as_ref().map(ToString::to_string), t.description.as_ref().map(ToString::to_string))
            }
        },
        EnumSchema::Legacy(l) => {
            (l.title.as_ref().map(ToString::to_string), l.description.as_ref().map(ToString::to_string))
        }
    }
}

fn options_from_strings(values: &[String]) -> Vec<SelectOption> {
    values.iter().map(|s| SelectOption { value: s.clone(), title: s.clone(), description: None }).collect()
}

fn options_from_const_titles(items: &[ConstTitle]) -> Vec<SelectOption> {
    items
        .iter()
        .map(|ct| SelectOption { value: ct.const_.clone(), title: ct.title.clone(), description: None })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_helpers::{elicitation_params, key};
    use acp_utils::EnumSchema;
    use acp_utils::testing::test_connection;
    use std::collections::BTreeMap;
    use std::sync::Arc;
    use tokio::task::LocalSet;

    fn test_schema() -> ElicitationSchema {
        serde_json::from_value(serde_json::json!({
            "type": "object",
            "properties": {
                "name": {
                    "type": "string",
                    "title": "Your Name",
                    "description": "Enter your full name"
                },
                "age": {
                    "type": "integer",
                    "title": "Age",
                    "minimum": 0,
                    "maximum": 150
                },
                "rating": {
                    "type": "number",
                    "title": "Rating"
                },
                "approved": {
                    "type": "boolean",
                    "title": "Approved",
                    "default": true
                },
                "color": {
                    "type": "string",
                    "title": "Favorite Color",
                    "enum": ["red", "green", "blue"]
                },
                "tags": {
                    "type": "array",
                    "title": "Tags",
                    "items": {
                        "type": "string",
                        "enum": ["fast", "reliable", "cheap"]
                    }
                }
            },
            "required": ["name", "color"]
        }))
        .unwrap()
    }

    #[test]
    fn parse_schema_extracts_all_field_types() {
        let schema = test_schema();
        let fields = parse_schema(&schema);
        assert_eq!(fields.len(), 6);

        let name_field = fields.iter().find(|f| f.name == "name").unwrap();
        assert_eq!(name_field.label, "Your Name");
        assert!(name_field.required);
        assert!(matches!(name_field.kind, FormFieldKind::Text(_)));

        let age_field = fields.iter().find(|f| f.name == "age").unwrap();
        match &age_field.kind {
            FormFieldKind::Number(nf) => assert!(nf.integer_only),
            _ => panic!("Expected Number (integer)"),
        }

        let bool_field = fields.iter().find(|f| f.name == "approved").unwrap();
        match &bool_field.kind {
            FormFieldKind::Boolean(cb) => assert!(cb.checked),
            _ => panic!("Expected Boolean"),
        }

        let color_field = fields.iter().find(|f| f.name == "color").unwrap();
        assert!(color_field.required);
        match &color_field.kind {
            FormFieldKind::SingleSelect(rs) => {
                assert_eq!(rs.options.len(), 3);
                assert_eq!(rs.options[0].value, "red");
            }
            _ => panic!("Expected SingleSelect"),
        }

        let tags_field = fields.iter().find(|f| f.name == "tags").unwrap();
        match &tags_field.kind {
            FormFieldKind::MultiSelect(ms) => {
                assert_eq!(ms.options.len(), 3);
                assert!(ms.selected.iter().all(|&s| !s));
            }
            _ => panic!("Expected MultiSelect"),
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn confirm_produces_correct_json() {
        LocalSet::new()
            .run_until(async {
                let (cx, mut peer) = test_connection().await;
                let (responder, _rx) = peer.fake_elicitation(&cx).await;
                let schema = ElicitationSchema::builder()
                    .optional_string("name")
                    .optional_bool("approved", true)
                    .optional_enum_schema(
                        "color",
                        EnumSchema::builder(vec!["red".into(), "green".into()])
                            .untitled()
                            .with_default("green")
                            .unwrap()
                            .build(),
                    )
                    .build()
                    .unwrap();
                let params = elicitation_params("test-server", "Test", schema);

                let form = ElicitationForm::from_params(params, responder);
                let response = form.confirm();

                assert_eq!(response.action, ElicitationAction::Accept);
                let content = response.content.unwrap();
                assert_eq!(content["name"], "");
                assert_eq!(content["approved"], true);
                assert_eq!(content["color"], "green");
            })
            .await;
    }

    #[test]
    fn esc_returns_cancel() {
        let response = ElicitationForm::cancel();
        assert_eq!(response.action, ElicitationAction::Cancel);
        assert!(response.content.is_none());
    }

    #[test]
    fn url_prompt_parses_host() {
        let prompt = UrlPrompt::new(
            "github".to_string(),
            "el-1".to_string(),
            "Authorize".to_string(),
            "https://github.com/login/oauth".to_string(),
        );
        assert_eq!(prompt.host.as_deref(), Some("github.com"));
        assert!(prompt.warnings.is_empty());
        assert!(prompt.launch_error.is_none());
    }

    #[test]
    fn url_prompt_warns_on_non_https() {
        let prompt = UrlPrompt::new(
            "test".to_string(),
            "el-1".to_string(),
            "Open this".to_string(),
            "http://example.com/form".to_string(),
        );
        assert_eq!(prompt.warnings.len(), 1);
        assert!(prompt.warnings[0].contains("HTTPS"));
    }

    #[test]
    fn url_prompt_does_not_warn_on_localhost() {
        let prompt = UrlPrompt::new(
            "test".to_string(),
            "el-1".to_string(),
            "Local".to_string(),
            "http://localhost:3000/auth".to_string(),
        );
        assert!(prompt.warnings.is_empty());
    }

    #[test]
    fn url_prompt_warns_on_invalid_url() {
        let prompt = UrlPrompt::new(
            "test".to_string(),
            "el-invalid".to_string(),
            "Check this".to_string(),
            "not a valid url".to_string(),
        );
        assert!(prompt.host.is_none());
        assert!(
            prompt.warnings.iter().any(|warning| warning.contains("could not be parsed")),
            "invalid URLs should show an explicit warning"
        );
    }

    #[test]
    fn url_prompt_warns_on_punycode() {
        let prompt = UrlPrompt::new(
            "test".to_string(),
            "el-1".to_string(),
            "Phishing".to_string(),
            "https://xn--e1afmkfd.xn--p1ai/".to_string(),
        );
        assert_eq!(prompt.warnings.len(), 1);
        assert!(prompt.warnings[0].contains("punycode"));
    }

    #[test]
    fn url_prompt_warns_on_punycode_and_non_https() {
        let prompt = UrlPrompt::new(
            "test".to_string(),
            "el-1".to_string(),
            "Both".to_string(),
            "http://xn--e1afmkfd.xn--p1ai/".to_string(),
        );
        assert_eq!(prompt.warnings.len(), 2, "both warnings should be present");
        assert!(prompt.warnings.iter().any(|w| w.contains("punycode")));
        assert!(prompt.warnings.iter().any(|w| w.contains("HTTPS")));
    }

    fn permission_like_params() -> ElicitationParams {
        let schema = ElicitationSchema::builder()
            .required_enum_schema(
                "decision",
                EnumSchema::builder(vec!["allow".into(), "deny".into()])
                    .untitled()
                    .with_default("deny")
                    .unwrap()
                    .build(),
            )
            .build()
            .unwrap();
        elicitation_params("coding", "Allow bash: rm -rf /tmp?", schema)
    }

    #[tokio::test(flavor = "current_thread")]
    async fn single_field_permission_like_form_submits_on_first_enter() {
        LocalSet::new()
            .run_until(async {
                let (cx, mut peer) = test_connection().await;
                let (responder, rx) = peer.fake_elicitation(&cx).await;
                let mut form = ElicitationForm::from_params(permission_like_params(), responder);

                let outcome = form.on_event(&key(tui::KeyCode::Enter)).await;
                let messages = outcome.expect("enter should be handled");

                assert!(messages.iter().any(|m| matches!(m, ElicitationMessage::Responded)));

                let response = rx.await.expect("first enter should produce a response");
                assert_eq!(response.action, ElicitationAction::Accept);
                assert_eq!(response.content.unwrap()["decision"], "deny");
            })
            .await;
    }

    #[tokio::test(flavor = "current_thread")]
    async fn single_field_permission_like_form_respects_default_deny() {
        LocalSet::new()
            .run_until(async {
                let (cx, mut peer) = test_connection().await;
                let (responder, _rx) = peer.fake_elicitation(&cx).await;
                let form = ElicitationForm::from_params(permission_like_params(), responder);

                let response = form.confirm();
                assert_eq!(response.action, ElicitationAction::Accept);
                assert_eq!(response.content.unwrap()["decision"], "deny");
            })
            .await;
    }

    #[tokio::test(flavor = "current_thread")]
    async fn form_modal_esc_returns_cancel() {
        LocalSet::new()
            .run_until(async {
                let (cx, mut peer) = test_connection().await;
                let (responder, rx) = peer.fake_elicitation(&cx).await;
                let params = elicitation_params("test", "Test", ElicitationSchema::builder().build().unwrap());
                let mut form = ElicitationForm::from_params(params, responder);
                let outcome = form.on_event(&key(tui::KeyCode::Esc)).await;
                let messages = outcome.unwrap();

                assert!(messages.iter().any(|m| matches!(m, ElicitationMessage::Responded)));

                let response = rx.await.unwrap();
                assert_eq!(response.action, ElicitationAction::Cancel);
            })
            .await;
    }

    #[test]
    fn one_of_string_produces_single_select() {
        let schema: ElicitationSchema = serde_json::from_value(serde_json::json!({
            "type": "object",
            "properties": {
                "size": {
                    "type": "string",
                    "oneOf": [
                        { "const": "s", "title": "Small" },
                        { "const": "m", "title": "Medium" },
                        { "const": "l", "title": "Large" }
                    ]
                }
            }
        }))
        .unwrap();
        let fields = parse_schema(&schema);
        assert_eq!(fields.len(), 1);
        match &fields[0].kind {
            FormFieldKind::SingleSelect(rs) => {
                assert_eq!(rs.options.len(), 3);
                assert_eq!(rs.options[0].title, "Small");
                assert_eq!(rs.options[0].value, "s");
            }
            _ => panic!("Expected SingleSelect"),
        }
    }

    #[test]
    fn empty_schema_produces_no_fields() {
        let schema = ElicitationSchema::new(BTreeMap::new());
        let fields = parse_schema(&schema);
        assert!(fields.is_empty());
    }

    #[test]
    fn url_modal_renders_server_name_without_url_or_controls() {
        use tui::testing::render_component;

        let prompt = UrlPrompt::new(
            "github".to_string(),
            "el-1".to_string(),
            "Authorize GitHub".to_string(),
            "https://github.com/login/oauth".to_string(),
        );
        let ui = ElicitationUi::Url(prompt);
        let mut form = ElicitationForm {
            ui,
            browser_opener: Arc::new(default_browser_opener),
            clipboard_writer: Arc::new(default_clipboard_writer),
            responder: None,
        };

        let lines = render_component(|ctx| form.render(ctx), 80, 20).get_lines();
        let text: String = lines.join("\n");
        assert!(text.contains("github"), "should show server name");
        assert!(text.contains("Authorize GitHub"), "should show request message");
        assert!(text.contains("github.com"), "should show host");
    }
}
