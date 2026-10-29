use client_core::elicitation::{ElicitationAnswer, ElicitationId};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::style::Modifier;
use ratatui::text::Line;

use crate::command::HostCommand;
use crate::surfaces::input::UiOutput;
use crate::theme::Theme;
use crate::view::widgets::KeyHint;
use crate::view::wrap::wrap_line;
use std::borrow::Cow;

pub(super) struct UrlModal {
    pub(super) server_name: String,
    message: String,
    url: String,
    host: Option<String>,
    warnings: Vec<String>,
    launch_error: Option<String>,
    copy_message: Option<String>,
    /// Set while the host opens the browser, so a second Enter cannot open it twice.
    opening: bool,
}

impl UrlModal {
    pub(super) fn new(server_name: String, message: String, url: String) -> Self {
        let parsed_url = url::Url::parse(&url);
        let host = parsed_url.as_ref().ok().and_then(|parsed| parsed.host_str().map(std::string::ToString::to_string));

        let mut warnings = Vec::new();
        match parsed_url {
            Ok(parsed_url) => {
                let is_local_http = parsed_url.scheme() == "http"
                    && matches!(parsed_url.host_str(), Some("localhost" | "127.0.0.1" | "::1"));
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
                if parsed_url.scheme() != "https" && !is_local_http {
                    warnings.push("Warning: URL does not use HTTPS.".to_string());
                }
            }
            Err(_) => {
                warnings.push("Warning: URL could not be parsed. Verify it carefully before proceeding.".to_string());
            }
        }

        Self { server_name, message, url, host, warnings, launch_error: None, copy_message: None, opening: false }
    }

    pub(super) fn on_key(&mut self, id: ElicitationId, key: KeyEvent) -> Option<UiOutput> {
        let plain_key = key.modifiers == KeyModifiers::NONE || key.modifiers == KeyModifiers::SHIFT;
        match key.code {
            KeyCode::Esc => Some(UiOutput::AnswerElicitation { id, answer: ElicitationAnswer::Cancel }),
            KeyCode::Enter if !self.opening => {
                self.opening = true;
                Some(UiOutput::Host(HostCommand::OpenUrl { elicitation: id, url: self.url.clone() }))
            }
            KeyCode::Char('c' | 'C') if plain_key => {
                Some(UiOutput::Host(HostCommand::CopyUrl { elicitation: id, url: self.url.clone() }))
            }
            _ => None,
        }
    }

    /// Opening the browser answers the request; a failure keeps asking.
    pub(super) fn on_opened(&mut self, result: Result<(), String>) -> Option<ElicitationAnswer> {
        self.opening = false;
        match result {
            Ok(()) => Some(ElicitationAnswer::Accept(None)),
            Err(error) => {
                self.launch_error = Some(format!("Failed to open browser: {error}"));
                None
            }
        }
    }

    pub(super) fn on_copied(&mut self, result: Result<(), String>) {
        self.copy_message = Some(match result {
            Ok(()) => "Copied URL to clipboard.".to_string(),
            Err(error) => format!("Failed to copy URL: {error}"),
        });
    }

    /// What is being authorized, where the browser would go, and anything about
    /// the URL worth a second look — everything but the key hints, which a host
    /// drawing this inline puts in its own footer.
    pub(super) fn body_lines(&self, theme: &Theme, width: u16) -> Vec<Line<'static>> {
        let mut lines = wrap_line(
            Line::styled(format!("Request from {}", self.server_name), (theme.accent, Modifier::BOLD)),
            width,
        );
        lines.extend(wrap_line(Line::raw(self.message.clone()), width));

        if let Some(ref host) = self.host {
            lines.extend(wrap_line(Line::styled(format!("Host: {host}"), theme.muted), width));
        }

        if !self.warnings.is_empty() {
            lines.push(Line::raw(""));
            for warning in &self.warnings {
                lines.extend(wrap_line(Line::styled(warning.clone(), theme.warning), width));
            }
        }

        if let Some(ref message) = self.copy_message {
            lines.push(Line::raw(""));
            lines.extend(wrap_line(Line::styled(message.clone(), theme.muted), width));
        }

        if let Some(ref error) = self.launch_error {
            lines.push(Line::raw(""));
            lines.extend(wrap_line(Line::styled(error.clone(), theme.error), width));
        }

        lines
    }
}

/// The keys a URL request answers, for whichever footer is drawing them.
pub(super) const HINTS: [KeyHint; 3] =
    [("Enter", Cow::Borrowed("open browser")), ("c", Cow::Borrowed("copy URL")), ("Esc", Cow::Borrowed("cancel"))];
