mod form;
pub(crate) mod frame;
mod url;

use crate::renderer::DrawContext;
use crate::surfaces::elicitation::ElicitationResponder;
use acp_utils::elicitation::source_mcp_server_name;
use agent_client_protocol::Responder;
use agent_client_protocol::schema::v1::{CreateElicitationRequest, CreateElicitationResponse, ElicitationMode};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Position, Rect};
use ratatui::text::Text;
use ratatui::widgets::{Paragraph, Widget};

use self::form::{FormAction, FormModal};
use self::frame::{MODAL_HORIZONTAL_PADDING, MODAL_VERTICAL_CHROME, ModalFrame};
use self::url::UrlModal;
use crate::session::platform::{BrowserOpener, ClipboardWriter};
use crate::surfaces::input::{ElicitationOutput, UiEvent, is_press};
use crate::theme::Theme;
use crate::view::widgets::{KeyHint, SCROLLBAR_WIDTH, key_hints};
use crate::view::wrap::as_u16;

pub struct ElicitationModal {
    kind: ModalKind,
    responder: ElicitationResponder,
    browser_opener: BrowserOpener,
    clipboard_writer: ClipboardWriter,
}

enum ModalKind {
    Form(FormModal),
    Url(UrlModal),
}

impl ElicitationModal {
    pub fn with_url_handlers(
        params: CreateElicitationRequest,
        responder: Responder<CreateElicitationResponse>,
        browser_opener: BrowserOpener,
        clipboard_writer: ClipboardWriter,
    ) -> Option<Self> {
        let server_name = source_mcp_server_name(params.meta.as_ref()).unwrap_or("Agent").to_string();
        let message = params.message;
        let responder = ElicitationResponder::new(responder);
        let kind = match params.mode {
            ElicitationMode::Form(form) => {
                ModalKind::Form(FormModal::new(server_name, message, &form.requested_schema)?)
            }
            ElicitationMode::Url(url) => ModalKind::Url(UrlModal::new(server_name, message, url.url)),
            _ => return None,
        };
        Some(Self { kind, responder, browser_opener, clipboard_writer })
    }

    /// Draws the request inside a host that keeps its own chrome, rather than as
    /// a modal over everything. The settings overlay shows an OAuth prompt this
    /// way so the server row that started it stays on screen.
    pub fn render_inline(&mut self, area: Rect, buf: &mut Buffer, theme: &Theme) {
        match &mut self.kind {
            ModalKind::Form(form) => {
                let _ = form.render(area, buf, theme);
            }
            ModalKind::Url(url) => {
                Paragraph::new(Text::from(url.body_lines(theme, area.width))).render(area, buf);
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

    fn on_url_key(&mut self, key: KeyEvent) -> Vec<ElicitationOutput> {
        let ModalKind::Url(url) = &mut self.kind else {
            return Vec::new();
        };
        let plain_key = key.modifiers == KeyModifiers::NONE || key.modifiers == KeyModifiers::SHIFT;
        match key.code {
            KeyCode::Esc => {
                self.responder.cancel();
                return vec![ElicitationOutput::Close];
            }
            KeyCode::Enter => {
                if let Err(error) = (self.browser_opener)(&url.url) {
                    url.launch_error = Some(format!("Failed to open browser: {error}"));
                } else {
                    self.responder.accept(None);
                    return vec![ElicitationOutput::Close];
                }
            }
            KeyCode::Char('c' | 'C') if plain_key => {
                url.copy_message = Some(match (self.clipboard_writer)(&url.url) {
                    Ok(()) => "Copied URL to clipboard.".to_string(),
                    Err(error) => format!("Failed to copy URL: {error}"),
                });
            }
            _ => {}
        }
        Vec::new()
    }
}

impl ElicitationModal {
    pub(crate) fn on_ui_event(&mut self, event: UiEvent) -> Vec<ElicitationOutput> {
        match event {
            UiEvent::Key(key) if is_press(key) => self.on_key(key),
            UiEvent::Key(_) => Vec::new(),
            UiEvent::Paste(text) => {
                if let ModalKind::Form(form) = &mut self.kind {
                    form.paste(&text);
                }
                Vec::new()
            }
            UiEvent::Mouse(action, (column, row)) => {
                if let Some(direction) = action.direction() {
                    if let ModalKind::Form(form) = &mut self.kind {
                        form.vertical(direction);
                    }
                } else if let ModalKind::Form(form) = &mut self.kind {
                    form.click(column, row);
                }
                Vec::new()
            }
        }
    }

    pub(crate) fn on_key(&mut self, key: KeyEvent) -> Vec<ElicitationOutput> {
        self.on_request_key(key)
    }

    /// A modal answers a request, so it owns every key: nothing falls through
    /// to the shared list navigation.
    fn on_request_key(&mut self, key: KeyEvent) -> Vec<ElicitationOutput> {
        let ModalKind::Form(form) = &mut self.kind else {
            return self.on_url_key(key);
        };
        match form.on_key(key) {
            FormAction::None => Vec::new(),
            FormAction::Cancel => {
                self.responder.cancel();
                vec![ElicitationOutput::Close]
            }
            FormAction::Accept(content) => {
                self.responder.accept(Some(content));
                vec![ElicitationOutput::Close]
            }
        }
    }

    pub(crate) fn needs_mouse_capture(&self) -> bool {
        matches!(self.kind, ModalKind::Form(_))
    }

    /// Dismissing the modal answers the request it was asking about.
    pub(crate) fn cancel(&mut self) {
        self.responder.cancel();
    }
}

impl ElicitationModal {
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
        match &mut self.kind {
            ModalKind::Form(form) => form.render(inner, buf, cx.theme),
            ModalKind::Url(url) => {
                Paragraph::new(Text::from(url.body_lines(cx.theme, inner.width))).render(inner, buf);
                None
            }
        }
    }
}

#[cfg(test)]
#[allow(clippy::absolute_paths, clippy::similar_names)]
mod tests {
    use super::form::permission_like_schema;
    use super::*;
    use acp_utils::testing::test_connection;
    use agent_client_protocol::schema::v1::{
        ElicitationAction, ElicitationFormMode, ElicitationSchema, ElicitationSessionScope, ElicitationUrlMode,
    };
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use tokio::task::LocalSet;

    /// Whether the modal asked to be dismissed.
    fn closes(messages: &[ElicitationOutput]) -> bool {
        messages.iter().any(|message| matches!(message, ElicitationOutput::Close))
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

    fn form_request(schema: ElicitationSchema) -> CreateElicitationRequest {
        CreateElicitationRequest::new(
            ElicitationFormMode::new(ElicitationSessionScope::new("session-1"), schema),
            String::new(),
        )
    }

    fn url_request(url: &str) -> CreateElicitationRequest {
        CreateElicitationRequest::new(
            ElicitationUrlMode::new(ElicitationSessionScope::new("session-1"), "el-1", url),
            "Authorize GitHub",
        )
    }

    async fn make_modal_for_schema(
        schema: ElicitationSchema,
    ) -> (ElicitationModal, tokio::sync::oneshot::Receiver<CreateElicitationResponse>) {
        let (cx, mut peer) = test_connection().await;
        let (responder, rx) = peer.fake_elicitation(&cx).await;
        let (opener, writer) = noop_handlers();
        (ElicitationModal::with_url_handlers(form_request(schema), responder, opener, writer).unwrap(), rx)
    }

    async fn make_url_modal(
        url: &str,
    ) -> (ElicitationModal, tokio::sync::oneshot::Receiver<CreateElicitationResponse>) {
        let (cx, mut peer) = test_connection().await;
        let (responder, rx) = peer.fake_elicitation(&cx).await;
        let (opener, writer) = noop_handlers();
        (ElicitationModal::with_url_handlers(url_request(url), responder, opener, writer).unwrap(), rx)
    }

    async fn make_url_modal_with_handlers(
        url: &str,
        opener: BrowserOpener,
        writer: ClipboardWriter,
    ) -> ElicitationModal {
        let (cx, mut peer) = test_connection().await;
        let (responder, _rx) = peer.fake_elicitation(&cx).await;
        ElicitationModal::with_url_handlers(url_request(url), responder, opener, writer).unwrap()
    }
    #[tokio::test(flavor = "current_thread")]
    async fn permission_like_form_returns_default_on_enter() {
        LocalSet::new()
            .run_until(async {
                let schema = permission_like_schema();
                let (mut modal, rx) = make_modal_for_schema(schema).await;
                assert!(closes(&modal.on_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))));
                let response = rx.await.unwrap();
                let ElicitationAction::Accept(accept) = response.action else { panic!("expected accept") };
                assert_eq!(accept.content.unwrap()["decision"], "deny".into());
            })
            .await;
    }
    #[tokio::test(flavor = "current_thread")]
    async fn esc_returns_cancel() {
        LocalSet::new()
            .run_until(async {
                let schema = ElicitationSchema::new();
                let (mut modal, rx) = make_modal_for_schema(schema).await;
                assert!(closes(&modal.on_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE))));
                let response = rx.await.unwrap();
                assert!(matches!(response.action, ElicitationAction::Cancel));
            })
            .await;
    }

    #[tokio::test(flavor = "current_thread")]
    async fn dropping_modal_responds_cancel() {
        LocalSet::new()
            .run_until(async {
                let schema = ElicitationSchema::new();
                let (modal, rx) = make_modal_for_schema(schema).await;
                drop(modal);
                let response = rx.await.unwrap();
                assert!(matches!(response.action, ElicitationAction::Cancel));
            })
            .await;
    }
    #[tokio::test(flavor = "current_thread")]
    async fn url_enter_opens_browser_and_accepts_request() {
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
                assert!(closes(&outcome));
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

    #[tokio::test(flavor = "current_thread")]
    async fn url_esc_cancels() {
        LocalSet::new()
            .run_until(async {
                let (mut modal, rx) = make_url_modal("https://github.com/login").await;
                assert!(closes(&modal.on_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE))));
                let response = rx.await.unwrap();
                assert!(matches!(response.action, ElicitationAction::Cancel));
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
                assert!(matches!(response.action, ElicitationAction::Cancel));
            })
            .await;
    }
}
