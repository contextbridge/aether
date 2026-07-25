mod form;
mod url;

use crate::elicitation::ElicitationResponder;
use crate::render_context::RenderContext;
use acp_utils::notifications::{
    CreateElicitationRequestParams, ElicitationAction, ElicitationParams, ElicitationResponse, McpNotification,
    UrlElicitationCompleteParams,
};
use agent_client_protocol::Responder;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Position, Rect};
use ratatui::widgets::{Clear, Widget};

use self::form::{FormAction, FormModal};
use self::url::UrlModal;
use crate::platform::{BrowserOpener, ClipboardWriter, default_browser_opener, default_clipboard_writer};
use crate::selection::Direction;
use crate::surface::{Surface, SurfaceMessage};

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
        Self { kind, responder: ElicitationResponder::new(responder), browser_opener, clipboard_writer }
    }

    /// Handles a completion notification, reporting whether it closed the modal.
    pub fn on_notification(&mut self, notification: &McpNotification) -> bool {
        let McpNotification::UrlElicitationComplete(params) = notification else {
            return false;
        };
        if !self.matches_url_completion(params) {
            return false;
        }
        self.responder.respond(ElicitationAction::Accept, None);
        true
    }

    fn on_url_key(&mut self, key: KeyEvent) -> Vec<SurfaceMessage> {
        let ModalKind::Url(url) = &mut self.kind else {
            return Vec::new();
        };
        let plain_key = key.modifiers == KeyModifiers::NONE || key.modifiers == KeyModifiers::SHIFT;
        match key.code {
            KeyCode::Esc => {
                self.responder.cancel();
                return vec![SurfaceMessage::Close];
            }
            KeyCode::Enter => {
                if let Err(error) = (self.browser_opener)(&url.url) {
                    url.launch_error = Some(format!("Failed to open browser: {error}"));
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

    fn matches_url_completion(&self, params: &UrlElicitationCompleteParams) -> bool {
        matches!(
            &self.kind,
            ModalKind::Url(url) if url.server_name == params.server_name && url.elicitation_id == params.elicitation_id
        )
    }
}

impl Surface for ElicitationModal {
    /// A modal answers a request, so it owns every key: nothing falls through
    /// to the shared list navigation.
    fn on_surface_key(&mut self, key: KeyEvent) -> Option<Vec<SurfaceMessage>> {
        if !matches!(key.kind, crossterm::event::KeyEventKind::Press | crossterm::event::KeyEventKind::Repeat) {
            return Some(Vec::new());
        }
        let ModalKind::Form(form) = &mut self.kind else {
            return Some(self.on_url_key(key));
        };
        Some(match form.on_key(key) {
            FormAction::None => Vec::new(),
            FormAction::Cancel => {
                self.responder.cancel();
                vec![SurfaceMessage::Close]
            }
            FormAction::Accept(content) => {
                self.responder.respond(ElicitationAction::Accept, Some(content));
                vec![SurfaceMessage::Close]
            }
        })
    }

    fn scroll(&mut self, direction: Direction) -> Vec<SurfaceMessage> {
        if let ModalKind::Form(form) = &mut self.kind {
            form.scroll(direction);
        }
        Vec::new()
    }

    fn click(&mut self, row: u16, _column: u16) -> Vec<SurfaceMessage> {
        if let ModalKind::Form(form) = &mut self.kind {
            form.click(row);
        }
        Vec::new()
    }

    fn needs_mouse_capture(&self) -> bool {
        matches!(self.kind, ModalKind::Form(_))
    }

    /// Dismissing the modal answers the request it was asking about.
    fn cancel(&mut self) {
        self.responder.cancel();
    }

    fn render(&mut self, area: Rect, buf: &mut Buffer, cx: &mut RenderContext<'_>) -> Option<Position> {
        let area = area.centered(Constraint::Percentage(80), Constraint::Percentage(80));
        Clear.render(area, buf);
        match &mut self.kind {
            ModalKind::Form(form) => form.render(area, buf, cx.theme),
            ModalKind::Url(url) => url.render(area, buf, cx.theme),
        }
        None
    }
}

#[cfg(test)]
#[allow(clippy::absolute_paths, clippy::similar_names)]
mod tests {
    use super::*;
    use acp_utils::testing::test_connection;
    use acp_utils::{ElicitationSchema, EnumSchema};
    use std::sync::Arc;
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

    /// Whether the modal asked to be dismissed.
    fn closes(messages: &[SurfaceMessage]) -> bool {
        messages.iter().any(|message| matches!(message, SurfaceMessage::Close))
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
    // ── Permission-like single-field ──

    #[tokio::test(flavor = "current_thread")]
    async fn permission_like_form_returns_default_on_enter() {
        LocalSet::new()
            .run_until(async {
                let schema = permission_like_schema();
                let (mut modal, rx) = make_modal_for_schema(schema).await;
                assert!(closes(&modal.on_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))));
                let response = rx.await.unwrap();
                assert_eq!(response.action, ElicitationAction::Accept);
                assert_eq!(response.content.unwrap()["decision"], "deny");
            })
            .await;
    }
    // ── Cancel ──

    #[tokio::test(flavor = "current_thread")]
    async fn esc_returns_cancel() {
        LocalSet::new()
            .run_until(async {
                let schema = ElicitationSchema::builder().build().unwrap();
                let (mut modal, rx) = make_modal_for_schema(schema).await;
                assert!(closes(&modal.on_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE))));
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
                assert!(!closes(&outcome));
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
                assert!(matched);
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
                assert!(!matched);
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
                assert!(!matched);
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
                assert!(!matched);
            })
            .await;
    }

    // ── URL cancel ──

    #[tokio::test(flavor = "current_thread")]
    async fn url_esc_cancels() {
        LocalSet::new()
            .run_until(async {
                let (mut modal, rx) = make_url_modal("https://github.com/login").await;
                assert!(closes(&modal.on_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE))));
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
