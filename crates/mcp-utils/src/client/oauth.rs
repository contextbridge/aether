use super::error::{ClientError, Result};
use super::{ClientOptions, Elicitation, elicitation::ElicitationRequest};
use aether_auth::{
    OAuthClientRegistration, OAuthError, OAuthFlowOptions, OAuthHandler, accept_oauth_callback,
    create_auth_manager_from_store, perform_oauth_flow,
};
use futures::future::BoxFuture;
use rmcp::model::{ElicitRequestParams, ElicitationAction};
use rmcp::transport::auth::AuthClient;
use std::sync::Arc;
use std::time::Duration;
use tokio::net::TcpListener;
use tokio::sync::mpsc;
use tokio::time::timeout;

#[derive(Debug, Clone, Default)]
pub struct OAuthSettings {
    pub registration: OAuthClientRegistration,
    pub callback_port: u16,
}

pub(super) async fn restore(
    server: &str,
    url: &str,
    oauth: &OAuthSettings,
    options: &ClientOptions,
) -> Option<AuthClient<reqwest::Client>> {
    let store = options.oauth_store.as_ref()?;
    let restored = create_auth_manager_from_store(
        server,
        url,
        oauth.registration.pre_registered_client_id(),
        &loopback_redirect_uri(oauth.callback_port),
        Arc::clone(store),
    )
    .await;
    match restored {
        Ok(manager) => manager.map(|manager| {
            tracing::debug!(server, "Using stored OAuth credentials");
            AuthClient::new(reqwest::Client::default(), manager)
        }),
        Err(error) => {
            tracing::warn!(
                server,
                %error,
                "Failed to initialize auth manager from stored credentials, proceeding without auth"
            );
            None
        }
    }
}

pub(super) async fn authorize(
    server: &str,
    url: &str,
    oauth: OAuthSettings,
    options: &ClientOptions,
    challenge: Option<String>,
) -> Result<AuthClient<reqwest::Client>> {
    let events = options.oauth_prompts().ok_or_else(|| ClientError::OAuthUnavailable { server: server.to_string() })?;
    let auth_error = |source| ClientError::Auth { server: server.to_string(), source };
    let handler = ElicitingOAuthHandler::bind(server, oauth.callback_port, events.clone())
        .map_err(|error| auth_error(OAuthError::from(error)))?;
    let flow = perform_oauth_flow(
        server,
        url,
        &handler,
        OAuthFlowOptions { client_registration: oauth.registration, challenge },
        options.oauth_store.clone(),
    );
    timeout(AUTH_TIMEOUT, flow)
        .await
        .map_err(|_| ClientError::AuthTimedOut { server: server.to_string(), timeout: AUTH_TIMEOUT })?
        .map_err(auth_error)
}

const AUTH_TIMEOUT: Duration = Duration::from_mins(3);
const OAUTH_ELICITATION_ID: &str = "oauth";

struct ElicitingOAuthHandler {
    listener: TcpListener,
    redirect_uri: String,
    server: String,
    events: mpsc::Sender<Elicitation>,
}

impl ElicitingOAuthHandler {
    fn bind(server: &str, port: u16, events: mpsc::Sender<Elicitation>) -> std::io::Result<Self> {
        let listener = std::net::TcpListener::bind(("127.0.0.1", port))?;
        let port = listener.local_addr()?.port();
        listener.set_nonblocking(true)?;
        Ok(Self {
            listener: TcpListener::from_std(listener)?,
            redirect_uri: loopback_redirect_uri(port),
            server: server.to_string(),
            events,
        })
    }
}

impl OAuthHandler for ElicitingOAuthHandler {
    fn redirect_uri(&self) -> &str {
        &self.redirect_uri
    }

    fn authorize(&self, auth_url: &str) -> BoxFuture<'_, std::result::Result<String, OAuthError>> {
        let request = ElicitRequestParams::UrlElicitationParams {
            meta: None,
            message: "Open this URL to authorize MCP server access.".to_string(),
            url: auth_url.to_string(),
            elicitation_id: OAUTH_ELICITATION_ID.to_string(),
        };
        Box::pin(async move {
            let (elicitation, response) = ElicitationRequest::new(self.server.clone(), request);
            self.events
                .send(Elicitation::Request(Box::new(elicitation)))
                .await
                .map_err(|_| OAuthError::Rmcp("OAuth prompt channel closed".to_string()))?;

            let result = tokio::select! {
                callback = accept_oauth_callback(&self.listener) => callback,
                response = response => match response {
                    Ok(result) if result.action == ElicitationAction::Accept => {
                        accept_oauth_callback(&self.listener).await
                    }
                    Ok(_) | Err(_) => Err(OAuthError::UserCancelled),
                },
            };
            if !matches!(result, Err(OAuthError::UserCancelled)) {
                let completion =
                    Elicitation::Complete { server: self.server.clone(), id: OAUTH_ELICITATION_ID.to_string() };
                let _ = self.events.send(completion).await;
            }
            result
        })
    }
}

fn loopback_redirect_uri(port: u16) -> String {
    format!("http://localhost:{port}/")
}
