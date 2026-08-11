use crate::client::manager::{ElicitationRequest, McpClientEvent, OAuthHandlerContext};
use aether_auth::{OAuthCallback, OAuthError, OAuthHandler, accept_oauth_callback};
use futures::future::BoxFuture;
use rmcp::model::{ElicitRequestParams, ElicitationAction};
use std::num::NonZeroU16;
use tokio::net::TcpListener;
use tokio::sync::{mpsc, oneshot};

const AETHER_OAUTH_ELICITATION_ID: &str = "aether-oauth";

/// `OAuthHandler` that dispatches the OAuth authorization URL to the host
pub struct ElicitingOAuthHandler {
    listener: TcpListener,
    redirect_uri: String,
    server_name: String,
    event_sender: mpsc::Sender<McpClientEvent>,
}

impl ElicitingOAuthHandler {
    pub fn new(ctx: OAuthHandlerContext) -> Result<Self, std::io::Error> {
        let (port, listener) = {
            let port = ctx.callback_port.map_or(0, NonZeroU16::get);
            let std_listener = std::net::TcpListener::bind(("127.0.0.1", port))?;
            let port = std_listener.local_addr()?.port();
            std_listener.set_nonblocking(true)?;
            (port, TcpListener::from_std(std_listener)?)
        };

        Ok(Self {
            listener,
            redirect_uri: format!("http://localhost:{port}/"),
            server_name: ctx.server_name,
            event_sender: ctx.tx,
        })
    }
}

impl OAuthHandler for ElicitingOAuthHandler {
    fn redirect_uri(&self) -> &str {
        &self.redirect_uri
    }

    fn authorize(&self, auth_url: &str) -> BoxFuture<'_, Result<OAuthCallback, OAuthError>> {
        let auth_url = auth_url.to_string();
        Box::pin(async move {
            let (response_sender, response_rx) = oneshot::channel();
            let request = ElicitationRequest {
                server_name: self.server_name.clone(),
                request: ElicitRequestParams::UrlElicitationParams {
                    meta: None,
                    message: "Open this URL to authorize MCP server access.".to_string(),
                    url: auth_url,
                    elicitation_id: AETHER_OAUTH_ELICITATION_ID.to_string(),
                },
                response_sender,
            };

            self.event_sender
                .send(McpClientEvent::Elicitation(request))
                .await
                .map_err(|_| OAuthError::Rmcp("OAuth prompt channel closed".to_string()))?;

            let callback = tokio::select! {
                callback = accept_oauth_callback(&self.listener) => callback,
                response = response_rx => match response {
                    Ok(result) if matches!(result.action, ElicitationAction::Decline | ElicitationAction::Cancel) => {
                        Err(OAuthError::UserCancelled)
                    }
                    Ok(_) | Err(_) => accept_oauth_callback(&self.listener).await,
                },
            }?;

            Ok(callback)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rmcp::model::ElicitResult;
    use std::sync::Arc;
    use tokio::{io::AsyncWriteExt, task::yield_now};

    #[tokio::test]
    async fn accepting_browser_prompt_keeps_waiting_for_oauth_callback() {
        let (tx, mut rx) = mpsc::channel(1);
        let handler = Arc::new(
            ElicitingOAuthHandler::new(OAuthHandlerContext {
                server_name: "slack".to_string(),
                callback_port: None,
                tx,
            })
            .unwrap(),
        );
        let port = handler
            .redirect_uri()
            .strip_prefix("http://localhost:")
            .and_then(|value| value.strip_suffix('/'))
            .unwrap()
            .parse::<u16>()
            .unwrap();

        let authorize = {
            let handler = Arc::clone(&handler);
            tokio::spawn(async move { handler.authorize("https://example.com/oauth").await })
        };
        let McpClientEvent::Elicitation(request) = rx.recv().await.unwrap() else {
            panic!("expected OAuth elicitation");
        };
        request.response_sender.send(ElicitResult::new(ElicitationAction::Accept)).unwrap();
        yield_now().await;
        assert!(!authorize.is_finished(), "accepting the prompt must not complete OAuth");

        let mut callback = tokio::net::TcpStream::connect(("127.0.0.1", port)).await.unwrap();
        callback.write_all(b"GET /?code=test-code&state=test-state HTTP/1.1\r\nHost: localhost\r\n\r\n").await.unwrap();

        let result = authorize.await.unwrap().unwrap();
        assert_eq!(result.code, "test-code");
        assert_eq!(result.state, "test-state");
    }

    #[tokio::test]
    async fn configured_callback_port_uses_registered_localhost_redirect() {
        let probe = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = probe.local_addr().unwrap().port();
        drop(probe);
        let (tx, _) = mpsc::channel(1);

        let handler = ElicitingOAuthHandler::new(OAuthHandlerContext {
            server_name: "slack".to_string(),
            callback_port: NonZeroU16::new(port),
            tx,
        })
        .unwrap();

        assert_eq!(handler.redirect_uri(), format!("http://localhost:{port}/"));
    }

    #[tokio::test]
    async fn configured_callback_port_fails_when_already_in_use() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let (tx, _) = mpsc::channel(1);

        let error = ElicitingOAuthHandler::new(OAuthHandlerContext {
            server_name: "slack".to_string(),
            callback_port: NonZeroU16::new(port),
            tx,
        })
        .err()
        .expect("occupied callback port should fail");

        assert_eq!(error.kind(), std::io::ErrorKind::AddrInUse);
    }
}
