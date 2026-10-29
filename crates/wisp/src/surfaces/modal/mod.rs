mod form;
pub(crate) mod frame;
mod url;

use crate::renderer::DrawContext;
use client_core::elicitation::{ElicitationAnswer, ElicitationId, FormField};
use crossterm::event::KeyEvent;
use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Position, Rect};
use ratatui::text::Text;
use ratatui::widgets::{Paragraph, Widget};

use self::form::FormModal;
use self::frame::{MODAL_HORIZONTAL_PADDING, MODAL_VERTICAL_CHROME, ModalFrame};
use self::url::UrlModal;
use crate::surfaces::input::UiOutput;
use crate::theme::Theme;
use crate::view::input::{MouseAction, UiEvent};
use crate::view::widgets::{KeyHint, SCROLLBAR_WIDTH, key_hints};
use crate::view::wrap::as_u16;

pub struct ElicitationModal {
    id: ElicitationId,
    kind: ModalKind,
}

enum ModalKind {
    Form(FormModal),
    Url(UrlModal),
}

impl ElicitationModal {
    pub fn form(id: ElicitationId, source: String, message: String, fields: Vec<FormField>) -> Self {
        Self { id, kind: ModalKind::Form(FormModal::new(source, message, fields)) }
    }

    pub fn url(id: ElicitationId, source: String, message: String, url: String) -> Self {
        Self { id, kind: ModalKind::Url(UrlModal::new(source, message, url)) }
    }

    pub fn id(&self) -> ElicitationId {
        self.id
    }

    pub fn render_inline(&mut self, area: Rect, buf: &mut Buffer, theme: &Theme) -> Option<Position> {
        match &mut self.kind {
            ModalKind::Form(form) => form.render(area, buf, theme),
            ModalKind::Url(url) => {
                Paragraph::new(Text::from(url.body_lines(theme, area.width))).render(area, buf);
                None
            }
        }
    }

    /// The rows [`Self::render_inline`] wants at `width`. A form lays out its
    /// own pages, so it takes whatever the host can spare.
    pub fn inline_height(&self, theme: &Theme, width: u16) -> u16 {
        match &self.kind {
            ModalKind::Form(_) => u16::MAX,
            ModalKind::Url(url) => as_u16(url.body_lines(theme, width).len()),
        }
    }

    /// The keys this request answers, for a host that draws its own footer.
    pub fn key_hints(&self) -> Vec<KeyHint> {
        match &self.kind {
            ModalKind::Form(form) => form.hints(),
            ModalKind::Url(_) => url::HINTS.to_vec(),
        }
    }

    pub(crate) fn on_ui_event(&mut self, event: &UiEvent) -> Option<UiOutput> {
        match event {
            UiEvent::Key(key) => self.on_key(*key),
            UiEvent::Paste(text) => {
                if let ModalKind::Form(form) = &mut self.kind {
                    form.paste(text);
                }
                None
            }
            UiEvent::Mouse(MouseAction::Moved, ..) => None,
            UiEvent::Mouse(action, (column, row), _) => {
                if let Some(direction) = action.direction() {
                    if let ModalKind::Form(form) = &mut self.kind {
                        form.vertical(direction);
                    }
                } else if let ModalKind::Form(form) = &mut self.kind {
                    form.click(*column, *row);
                }
                None
            }
        }
    }

    /// The host's answer to [`HostCommand::OpenUrl`](crate::command::HostCommand::OpenUrl).
    pub(crate) fn on_url_opened(&mut self, result: Result<(), String>) -> Option<ElicitationAnswer> {
        match &mut self.kind {
            ModalKind::Url(url) => url.on_opened(result),
            ModalKind::Form(_) => None,
        }
    }

    pub(crate) fn on_url_copied(&mut self, result: Result<(), String>) {
        if let ModalKind::Url(url) = &mut self.kind {
            url.on_copied(result);
        }
    }

    pub(crate) fn needs_mouse_capture(&self) -> bool {
        matches!(self.kind, ModalKind::Form(_))
    }

    pub(crate) fn render(&mut self, area: Rect, buf: &mut Buffer, cx: &mut DrawContext<'_>) -> Option<Position> {
        // The modal hugs its content: wide enough to read, never more than a
        // fraction of the screen, and only as tall as the page it is asking
        // about (plus a scroll bar when a long page cannot fit).
        let width = area.width.min(76).min(area.width.saturating_mul(92) / 100);
        let content_width = width.saturating_sub(MODAL_HORIZONTAL_PADDING * 2);
        let (title, footer, body_rows) = match &self.kind {
            ModalKind::Form(form) => (
                "Request",
                key_hints(&form.hints(), cx.theme),
                form.content_height(cx.theme, content_width.saturating_sub(SCROLLBAR_WIDTH + 1)),
            ),
            ModalKind::Url(url) => {
                ("Authorization", key_hints(&url::HINTS, cx.theme), url.body_lines(cx.theme, content_width).len())
            }
        };
        let height = as_u16(body_rows + usize::from(MODAL_VERTICAL_CHROME))
            .min(area.height.saturating_mul(70) / 100)
            .clamp(3.min(area.height), area.height);
        let server_name = match &self.kind {
            ModalKind::Form(form) => form.server_name(),
            ModalKind::Url(url) => url.server_name.as_str(),
        };
        let frame =
            ModalFrame::new(title, Some(footer), Constraint::Length(width), Constraint::Length(height), cx.theme)
                .title_right(server_name);
        let inner = frame.inner(area);
        (&frame).render(area, buf);
        self.render_inline(inner, buf, cx.theme)
    }
}

impl ElicitationModal {
    fn on_key(&mut self, key: KeyEvent) -> Option<UiOutput> {
        match &mut self.kind {
            ModalKind::Url(url) => url.on_key(self.id, key),
            ModalKind::Form(form) => form.on_key(key).map(|answer| UiOutput::AnswerElicitation { id: self.id, answer }),
        }
    }
}

#[cfg(test)]
#[allow(clippy::absolute_paths, clippy::similar_names)]
mod tests {
    use super::*;
    use crate::command::HostCommand;
    use crate::testing::buffer_text;
    use agent_client_protocol::schema::v2::{ElicitationSchema, StringPropertySchema};
    use crossterm::event::{KeyCode, KeyModifiers};

    const LOGIN: &str = "https://github.com/login";

    #[test]
    fn enter_accepts_a_forms_defaults() {
        let schema = ElicitationSchema::new().property(
            "decision",
            StringPropertySchema::new().enum_values(vec!["allow".into(), "deny".into()]).default_value("deny"),
            true,
        );
        let mut modal = form_modal(&schema);

        let Some(UiOutput::AnswerElicitation { answer: ElicitationAnswer::Accept(Some(content)), .. }) =
            modal.on_ui_event(&key(KeyCode::Enter))
        else {
            panic!("expected accept")
        };
        assert_eq!(content["decision"], "deny".into());
    }

    #[test]
    fn enter_asks_the_host_to_open_the_url_once_and_accepts_when_it_opens() {
        let mut modal = url_modal(LOGIN);

        let output = modal.on_ui_event(&key(KeyCode::Enter));
        assert!(
            matches!(&output, Some(UiOutput::Host(HostCommand::OpenUrl { url, .. })) if url == LOGIN),
            "{output:?}"
        );
        assert!(modal.on_ui_event(&key(KeyCode::Enter)).is_none(), "the browser is already opening");
        assert_eq!(modal.on_url_opened(Ok(())), Some(ElicitationAnswer::Accept(None)));
    }

    #[test]
    fn a_failed_open_shows_the_error_and_keeps_asking() {
        let mut modal = url_modal(LOGIN);
        modal.on_ui_event(&key(KeyCode::Enter));

        assert_eq!(modal.on_url_opened(Err("simulated failure".into())), None);
        assert!(rendered(&mut modal).contains("Failed to open browser: simulated failure"));
        assert!(modal.on_ui_event(&key(KeyCode::Enter)).is_some(), "Enter retries");
    }

    #[test]
    fn c_asks_the_host_to_copy_the_url_and_shows_the_outcome() {
        let mut modal = url_modal(LOGIN);

        let output = modal.on_ui_event(&key(KeyCode::Char('c')));
        assert!(
            matches!(&output, Some(UiOutput::Host(HostCommand::CopyUrl { url, .. })) if url == LOGIN),
            "{output:?}"
        );

        modal.on_url_copied(Ok(()));
        assert!(rendered(&mut modal).contains("Copied URL to clipboard."));
        modal.on_url_copied(Err("simulated failure".into()));
        assert!(rendered(&mut modal).contains("Failed to copy URL: simulated failure"));
    }

    #[test]
    fn esc_cancels_a_url() {
        let mut modal = url_modal(LOGIN);

        let output = modal.on_ui_event(&key(KeyCode::Esc));
        assert!(
            matches!(output, Some(UiOutput::AnswerElicitation { answer: ElicitationAnswer::Cancel, .. })),
            "{output:?}"
        );
    }

    #[test]
    fn a_url_shows_its_host() {
        let mut modal = url_modal(LOGIN);

        let screen = rendered(&mut modal);
        assert!(screen.contains("Host: github.com"), "{screen}");
        assert!(!screen.contains("Warning"), "{screen}");
    }

    #[test]
    fn local_http_urls_raise_no_warning() {
        for url in ["http://localhost:3000/auth", "http://127.0.0.1:8000/api"] {
            let screen = rendered(&mut url_modal(url));
            assert!(!screen.contains("Warning"), "{url}: {screen}");
        }
    }

    #[test]
    fn suspicious_urls_raise_warnings() {
        let cases = [
            ("http://example.com/form", &["does not use HTTPS"][..]),
            ("not a valid url", &["could not be parsed"]),
            ("https://xn--e1afmkfd.xn--p1ai/", &["punycode"]),
            ("http://xn--e1afmkfd.xn--p1ai/", &["punycode", "does not use HTTPS"]),
            ("https://user:pass@example.com/path", &["embedded credentials"]),
        ];
        for (url, warnings) in cases {
            let screen = rendered(&mut url_modal(url));
            for warning in warnings {
                assert!(screen.contains(warning), "{url} should warn about {warning}: {screen}");
            }
            assert_eq!(screen.matches("Warning").count(), warnings.len(), "{url}: {screen}");
        }
    }

    fn form_modal(schema: &ElicitationSchema) -> ElicitationModal {
        let fields = client_core::elicitation::form_fields(schema).unwrap();
        ElicitationModal::form(ElicitationId::next(), "Agent".into(), String::new(), fields)
    }

    fn url_modal(url: &str) -> ElicitationModal {
        ElicitationModal::url(ElicitationId::next(), "Agent".into(), "Authorize GitHub".into(), url.into())
    }

    fn key(code: KeyCode) -> UiEvent {
        UiEvent::Key(KeyEvent::new(code, KeyModifiers::NONE))
    }

    fn rendered(modal: &mut ElicitationModal) -> String {
        let area = Rect::new(0, 0, 200, 20);
        let mut buffer = Buffer::empty(area);
        modal.render_inline(area, &mut buffer, &Theme::default());
        buffer_text(&buffer)
    }
}
