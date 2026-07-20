use ratatui::style::{Modifier, Style};
use ratatui::text::Line;

use crate::theme::Theme;
use crate::view::widgets::KeyHint;
use crate::view::wrap::wrap_line;
use std::borrow::Cow;

pub(super) struct UrlModal {
    pub(super) server_name: String,
    message: String,
    pub(super) url: String,
    host: Option<String>,
    warnings: Vec<String>,
    pub(super) launch_error: Option<String>,
    pub(super) copy_message: Option<String>,
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

        Self { server_name, message, url, host, warnings, launch_error: None, copy_message: None }
    }

    /// What is being authorized, where the browser would go, and anything about
    /// the URL worth a second look — everything but the key hints, which a host
    /// drawing this inline puts in its own footer.
    pub(super) fn body_lines(&self, theme: &Theme, width: u16) -> Vec<Line<'static>> {
        let mut lines = wrap_line(
            Line::styled(
                format!("Request from {}", self.server_name),
                Style::new().fg(theme.accent).add_modifier(Modifier::BOLD),
            ),
            width,
        );
        lines.extend(wrap_line(Line::raw(self.message.clone()), width));

        if let Some(ref host) = self.host {
            lines.extend(wrap_line(Line::styled(format!("Host: {host}"), Style::new().fg(theme.muted)), width));
        }

        if !self.warnings.is_empty() {
            lines.push(Line::raw(""));
            for warning in &self.warnings {
                lines.extend(wrap_line(Line::styled(warning.clone(), Style::new().fg(theme.warning)), width));
            }
        }

        if let Some(ref message) = self.copy_message {
            lines.push(Line::raw(""));
            lines.extend(wrap_line(Line::styled(message.clone(), Style::new().fg(theme.muted)), width));
        }

        if let Some(ref error) = self.launch_error {
            lines.push(Line::raw(""));
            lines.extend(wrap_line(Line::styled(error.clone(), Style::new().fg(theme.error)), width));
        }

        lines
    }
}

/// The keys a URL request answers, for whichever footer is drawing them.
pub(super) const HINTS: [KeyHint; 3] =
    [("Enter", Cow::Borrowed("open browser")), ("c", Cow::Borrowed("copy URL")), ("Esc", Cow::Borrowed("cancel"))];

#[cfg(test)]
#[allow(clippy::absolute_paths, clippy::similar_names)]
mod tests {
    use super::*;
    #[test]
    fn url_modal_parses_host() {
        let url = UrlModal::new("github".into(), "Auth".into(), "https://github.com/login".into());
        assert_eq!(url.host.as_deref(), Some("github.com"));
        assert!(url.warnings.is_empty());
    }

    #[test]
    fn url_modal_warns_on_non_https() {
        let url = UrlModal::new("test".into(), "Open".into(), "http://example.com/form".into());
        assert_eq!(url.warnings.len(), 1);
        assert!(url.warnings[0].contains("HTTPS"));
    }

    #[test]
    fn url_modal_does_not_warn_on_localhost() {
        let url = UrlModal::new("test".into(), "Local".into(), "http://localhost:3000/auth".into());
        assert!(url.warnings.is_empty());
    }

    #[test]
    fn url_modal_allows_127_0_0_1_as_localhost() {
        let url = UrlModal::new("test".into(), "Local".into(), "http://127.0.0.1:8000/api".into());
        assert!(url.warnings.is_empty());
    }

    #[test]
    fn url_modal_warns_on_invalid_url() {
        let url = UrlModal::new("test".into(), "Check".into(), "not a valid url".into());
        assert!(url.host.is_none());
        assert!(url.warnings.iter().any(|w| w.contains("could not be parsed")));
    }

    #[test]
    fn url_modal_warns_on_punycode() {
        let url = UrlModal::new("test".into(), "Phish".into(), "https://xn--e1afmkfd.xn--p1ai/".into());
        assert_eq!(url.warnings.len(), 1);
        assert!(url.warnings[0].contains("punycode"));
    }

    #[test]
    fn url_modal_warns_on_punycode_and_non_https() {
        let url = UrlModal::new("test".into(), "Both".into(), "http://xn--e1afmkfd.xn--p1ai/".into());
        assert_eq!(url.warnings.len(), 2);
        assert!(url.warnings.iter().any(|w| w.contains("punycode")));
        assert!(url.warnings.iter().any(|w| w.contains("HTTPS")));
    }

    #[test]
    fn url_modal_warns_on_embedded_credentials() {
        let url = UrlModal::new("test".into(), "Auth".into(), "https://user:pass@example.com/path".into());
        assert!(url.warnings.iter().any(|w| w.contains("credentials")), "warnings: {:?}", url.warnings);
    }

    #[test]
    fn url_modal_no_credential_warning_for_clean_url() {
        let url = UrlModal::new("test".into(), "Auth".into(), "https://example.com/path".into());
        assert!(!url.warnings.iter().any(|w| w.contains("credentials")), "warnings: {:?}", url.warnings);
    }
}
