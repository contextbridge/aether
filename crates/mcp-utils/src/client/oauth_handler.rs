use crate::client::manager::{BrowserAuthorizationResponse, McpClientEvent, OAuthHandlerContext};
use aether_auth::{OAuthCallback, OAuthError, OAuthHandler, accept_oauth_callback};
use futures::future::BoxFuture;
use std::num::NonZeroU16;
use tokio::net::TcpListener;
use tokio::sync::{mpsc, oneshot};

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
            self.event_sender
                .send(McpClientEvent::BrowserAuthorizationRequested {
                    server_name: self.server_name.clone(),
                    message: "Open this URL to authorize MCP server access.".to_string(),
                    url: auth_url,
                    response_sender,
                })
                .await
                .map_err(|_| OAuthError::Rmcp("OAuth prompt channel closed".to_string()))?;

            let callback = tokio::select! {
                callback = accept_oauth_callback(&self.listener) => callback,
                response = response_rx => match response {
                    Ok(BrowserAuthorizationResponse::Cancel) => Err(OAuthError::UserCancelled),
                    Ok(BrowserAuthorizationResponse::Proceed) | Err(_) => {
                        accept_oauth_callback(&self.listener).await
                    }
                },
            }?;

            let complete = McpClientEvent::BrowserAuthorizationCompleted { server_name: self.server_name.clone() };

            if self.event_sender.send(complete).await.is_err() {
                tracing::warn!("Failed to send browser authorization completion: receiver dropped");
            }

            Ok(callback)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
