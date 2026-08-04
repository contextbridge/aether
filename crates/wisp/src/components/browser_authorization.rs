//! Private Aether browser-authorization prompt.
//!
//! The local OAuth browser flow is driven by the private
//! `_aether/browser_authorization` request rather than an MCP URL elicitation:
//! this prompt reuses the `UrlPrompt` safeguards but carries a dedicated
//! responder and no protocol elicitation id semantics.

use crate::components::elicitation_form::{
    BrowserOpener, ClipboardWriter, UrlHandlerError, UrlPrompt, UrlPromptOutcome, default_browser_opener,
    default_clipboard_writer, render_url_prompt,
};
use acp_utils::notifications::{BrowserAuthorizationParams, BrowserAuthorizationResponseParams};
use agent_client_protocol::Responder;
use std::sync::Arc;
use tui::{Component, Event, Frame, ViewContext};

pub enum BrowserAuthorizationMessage {
    /// The prompt was answered or dismissed; the modal should close.
    Responded,
    /// The browser was opened for `server_name`; the modal stays open until
    /// the flow completes.
    Opened { server_name: String },
}

pub struct BrowserAuthorizationPrompt {
    pub prompt: UrlPrompt,
    browser_opener: BrowserOpener,
    clipboard_writer: ClipboardWriter,
    responder: Option<Responder<BrowserAuthorizationResponseParams>>,
}

impl BrowserAuthorizationPrompt {
    pub fn from_params(
        params: BrowserAuthorizationParams,
        responder: Responder<BrowserAuthorizationResponseParams>,
    ) -> Self {
        Self::with_url_handlers(params, responder, default_browser_opener, default_clipboard_writer)
    }

    pub fn with_url_handlers<T, U>(
        params: BrowserAuthorizationParams,
        responder: Responder<BrowserAuthorizationResponseParams>,
        browser_opener: T,
        clipboard_writer: U,
    ) -> Self
    where
        T: Fn(&str) -> Result<(), UrlHandlerError> + Send + Sync + 'static,
        U: Fn(&str) -> Result<(), UrlHandlerError> + Send + Sync + 'static,
    {
        let prompt = UrlPrompt::new(params.server_name, params.message, params.url);
        Self {
            prompt,
            browser_opener: Arc::new(browser_opener),
            clipboard_writer: Arc::new(clipboard_writer),
            responder: Some(responder),
        }
    }

    fn proceed() -> BrowserAuthorizationResponseParams {
        BrowserAuthorizationResponseParams { proceed: true }
    }

    fn cancel() -> BrowserAuthorizationResponseParams {
        BrowserAuthorizationResponseParams { proceed: false }
    }

    /// The browser flow for `server_name` finished. Answer any still-pending
    /// responder with `Proceed` and report whether this prompt matched.
    pub fn accept_completed(&mut self, server_name: &str) -> bool {
        if self.prompt.server_name != server_name {
            return false;
        }
        if let Some(responder) = self.responder.take() {
            let _ = responder.respond(Self::proceed());
        }
        true
    }
}

impl Component for BrowserAuthorizationPrompt {
    type Message = BrowserAuthorizationMessage;

    async fn on_event(&mut self, event: &Event) -> Option<Vec<Self::Message>> {
        let Event::Key(key) = event else {
            return Some(vec![]);
        };
        let Some(outcome) = self.prompt.on_key(key, &self.browser_opener, &self.clipboard_writer) else {
            return Some(vec![]);
        };
        match outcome {
            UrlPromptOutcome::Opened => {
                let _ = self.responder.take().map(|r| r.respond(Self::proceed()));
                Some(vec![BrowserAuthorizationMessage::Opened { server_name: self.prompt.server_name.clone() }])
            }
            UrlPromptOutcome::Copied => Some(vec![]),
            UrlPromptOutcome::Cancelled => {
                let _ = self.responder.take().map(|r| r.respond(Self::cancel()));
                Some(vec![BrowserAuthorizationMessage::Responded])
            }
        }
    }

    fn render(&mut self, ctx: &ViewContext) -> Frame {
        render_url_prompt(&self.prompt, ctx)
    }
}

/// A prompt dropped without being answered (dismissed, replaced, or the app
/// shut down) still owes the requesting relay a response, or the OAuth handler
/// would wait forever.
impl Drop for BrowserAuthorizationPrompt {
    fn drop(&mut self) {
        if let Some(responder) = self.responder.take() {
            let _ = responder.respond(Self::cancel());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_helpers::key;
    use acp_utils::testing::test_connection;
    use std::sync::{Arc, Mutex};
    use tokio::task::LocalSet;

    fn params(server: &str, url: &str) -> BrowserAuthorizationParams {
        BrowserAuthorizationParams {
            server_name: server.to_string(),
            message: "Open this URL to authorize MCP server access.".to_string(),
            url: url.to_string(),
        }
    }

    type RecordingOpener = (Box<dyn Fn(&str) -> Result<(), UrlHandlerError> + Send + Sync>, Arc<Mutex<Vec<String>>>);

    fn recording_opener() -> RecordingOpener {
        let recorded_urls = Arc::new(Mutex::new(Vec::new()));
        let recorded_urls_for_assertion = recorded_urls.clone();
        (
            Box::new(move |url: &str| {
                recorded_urls.lock().unwrap().push(url.to_string());
                Ok(())
            }),
            recorded_urls_for_assertion,
        )
    }

    fn noop_clipboard() -> impl Fn(&str) -> Result<(), UrlHandlerError> {
        |_| Ok(())
    }

    #[tokio::test(flavor = "current_thread")]
    async fn enter_opens_browser_and_responds_proceed() {
        LocalSet::new()
            .run_until(async {
                let (cx, mut peer) = test_connection().await;
                let (responder, rx) = peer.fake_browser_authorization(&cx).await;
                let (opener, recorded_urls) = recording_opener();
                let mut prompt = BrowserAuthorizationPrompt::with_url_handlers(
                    params("github", "https://github.com/oauth"),
                    responder,
                    opener,
                    noop_clipboard(),
                );

                let outcome = prompt.on_event(&key(tui::KeyCode::Enter)).await;
                let messages = outcome.expect("enter should be handled");
                assert!(messages.iter().any(|m| matches!(
                    m,
                    BrowserAuthorizationMessage::Opened { server_name } if server_name == "github"
                )));

                let response = rx.await.expect("responder should be consumed");
                assert!(response.proceed, "opening the browser proceeds with the flow");
                assert_eq!(recorded_urls.lock().unwrap().as_slice(), &["https://github.com/oauth"]);
            })
            .await;
    }

    #[tokio::test(flavor = "current_thread")]
    async fn esc_cancels_and_responds_cancel() {
        LocalSet::new()
            .run_until(async {
                let (cx, mut peer) = test_connection().await;
                let (responder, rx) = peer.fake_browser_authorization(&cx).await;
                let (opener, _opened) = recording_opener();
                let mut prompt = BrowserAuthorizationPrompt::with_url_handlers(
                    params("github", "https://github.com/oauth"),
                    responder,
                    opener,
                    noop_clipboard(),
                );

                let outcome = prompt.on_event(&key(tui::KeyCode::Esc)).await;
                let messages = outcome.expect("esc should be handled");
                assert!(messages.iter().any(|m| matches!(m, BrowserAuthorizationMessage::Responded)));

                let response = rx.await.expect("responder should be consumed");
                assert!(!response.proceed, "esc cancels the browser flow");
            })
            .await;
    }

    #[tokio::test(flavor = "current_thread")]
    async fn completion_answers_pending_responder_with_proceed() {
        LocalSet::new()
            .run_until(async {
                let (cx, mut peer) = test_connection().await;
                let (responder, rx) = peer.fake_browser_authorization(&cx).await;
                let (opener, _opened) = recording_opener();
                let mut prompt = BrowserAuthorizationPrompt::with_url_handlers(
                    params("github", "https://github.com/oauth"),
                    responder,
                    opener,
                    noop_clipboard(),
                );

                assert!(prompt.accept_completed("github"), "matching server closes the prompt");
                let response = rx.await.expect("completion should answer the pending responder");
                assert!(response.proceed);
                assert!(!prompt.accept_completed("other"), "mismatched server is ignored");
            })
            .await;
    }

    #[tokio::test(flavor = "current_thread")]
    async fn dropping_unanswered_prompt_responds_cancel() {
        LocalSet::new()
            .run_until(async {
                let (cx, mut peer) = test_connection().await;
                let (responder, rx) = peer.fake_browser_authorization(&cx).await;
                let (opener, _opened) = recording_opener();

                drop(BrowserAuthorizationPrompt::with_url_handlers(
                    params("github", "https://github.com/oauth"),
                    responder,
                    opener,
                    noop_clipboard(),
                ));

                let response = rx.await.expect("dropped prompt must still answer the requester");
                assert!(!response.proceed, "dropped prompt cancels");
            })
            .await;
    }
}
