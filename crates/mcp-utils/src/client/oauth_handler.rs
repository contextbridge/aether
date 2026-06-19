use crate::client::manager::{ElicitationRequest, McpClientEvent, OAuthHandlerContext, UrlElicitationCompleteParams};
use aether_auth::{OAuthCallback, OAuthError, OAuthHandler, accept_oauth_callback};
use futures::future::BoxFuture;
use rmcp::model::{CreateElicitationRequestParams, ElicitationAction};
use tokio::net::TcpListener;
use tokio::sync::{mpsc, oneshot};

pub const AETHER_OAUTH_ELICITATION_ID: &str = "aether-oauth";

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
            let std_listener = std::net::TcpListener::bind("127.0.0.1:0")?;
            let port = std_listener.local_addr()?.port();
            std_listener.set_nonblocking(true)?;
            (port, TcpListener::from_std(std_listener)?)
        };

        Ok(Self {
            listener,
            redirect_uri: format!("http://127.0.0.1:{port}/oauth2callback"),
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
                request: CreateElicitationRequestParams::UrlElicitationParams {
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

            let complete = UrlElicitationCompleteParams {
                server_name: self.server_name.clone(),
                elicitation_id: AETHER_OAUTH_ELICITATION_ID.to_string(),
            };

            if self.event_sender.send(McpClientEvent::UrlElicitationComplete(complete)).await.is_err() {
                tracing::warn!("Failed to send OAuth URL elicitation completion: receiver dropped");
            }

            Ok(callback)
        })
    }
}
