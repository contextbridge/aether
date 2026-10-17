use super::error::{ClientError, Result};
use super::{ClientOptions, Elicitation, elicitation::ElicitationRequest};
use aether_auth::{OAuthCredentialStorage, OAuthError, accept_oauth_callback, bind_loopback};
use async_trait::async_trait;
use rmcp::model::{ElicitRequestParams, ElicitationAction};
use rmcp::transport::auth::{
    AuthClient, AuthError, AuthorizationManager, AuthorizationRequest, AuthorizationSession, CredentialStore,
    StoredCredentials,
};
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

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum OAuthClientRegistration {
    PreRegistered(String),
    ClientMetadata(String),
    #[default]
    Dynamic,
}

pub(super) async fn restore(
    server: &str,
    url: &str,
    oauth: &OAuthSettings,
    options: &ClientOptions,
) -> Option<AuthClient<reqwest::Client>> {
    let store = McpCredentialStore::new(server, oauth, options)?;
    match restore_manager(url, store).await {
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
    let prompts =
        options.oauth_prompts().ok_or_else(|| ClientError::OAuthUnavailable { server: server.to_string() })?;
    let store = McpCredentialStore::new(server, &oauth, options);
    let flow = authorization_flow(server, url, oauth, store, challenge, prompts);
    timeout(AUTH_TIMEOUT, flow)
        .await
        .map_err(|_| ClientError::AuthTimedOut { server: server.to_string(), timeout: AUTH_TIMEOUT })?
        .map_err(|source| ClientError::Auth { server: server.to_string(), source })
}

const AUTH_TIMEOUT: Duration = Duration::from_mins(3);
const OAUTH_CLIENT_NAME: &str = "Aether MCP Client";
const OAUTH_ELICITATION_ID: &str = "oauth";

#[derive(Clone)]
struct McpCredentialStore {
    key: String,
    store: Arc<dyn OAuthCredentialStorage>,
    expected_client_id: Option<String>,
}

impl McpCredentialStore {
    fn new(server: &str, oauth: &OAuthSettings, options: &ClientOptions) -> Option<Self> {
        let expected_client_id = match &oauth.registration {
            OAuthClientRegistration::PreRegistered(client_id) => Some(client_id.clone()),
            OAuthClientRegistration::ClientMetadata(_) | OAuthClientRegistration::Dynamic => None,
        };
        let store = Arc::clone(options.oauth_store.as_ref()?);
        Some(Self { key: format!("mcp:{server}"), store, expected_client_id })
    }
}

#[async_trait]
impl CredentialStore for McpCredentialStore {
    async fn load(&self) -> std::result::Result<Option<StoredCredentials>, AuthError> {
        let Some(value) = self.store.load(&self.key).await.map_err(|error| store_error(&error))? else {
            return Ok(None);
        };
        let credentials = serde_json::from_value::<StoredCredentials>(value)
            .map_err(|error| AuthError::CredentialStoreError(format!("invalid MCP credential: {error}")))?;
        Ok(Some(credentials).filter(|credentials| {
            self.expected_client_id.as_deref().is_none_or(|expected| credentials.client_id == expected)
        }))
    }

    async fn save(&self, credentials: StoredCredentials) -> std::result::Result<(), AuthError> {
        let value = serde_json::to_value(&credentials)
            .map_err(|error| AuthError::InternalError(format!("failed to serialize credentials: {error}")))?;
        self.store.save(&self.key, value).await.map_err(|error| store_error(&error))
    }

    async fn clear(&self) -> std::result::Result<(), AuthError> {
        self.store.delete(&self.key).await.map_err(|error| store_error(&error))
    }
}

async fn restore_manager(
    url: &str,
    store: McpCredentialStore,
) -> std::result::Result<Option<AuthorizationManager>, AuthError> {
    let mut manager = AuthorizationManager::new(url).await?;
    manager.set_credential_store(store);
    Ok(manager.initialize_from_store().await?.then_some(manager))
}

async fn authorization_flow(
    server: &str,
    url: &str,
    oauth: OAuthSettings,
    store: Option<McpCredentialStore>,
    challenge: Option<String>,
    prompts: &mpsc::Sender<Elicitation>,
) -> std::result::Result<AuthClient<reqwest::Client>, OAuthError> {
    let (listener, port) = bind_loopback(oauth.callback_port)?;
    let mut manager = AuthorizationManager::new(url).await.map_err(rmcp_err("OAuth init failed"))?;
    if let Some(store) = store {
        manager.set_credential_store(store);
    }
    let resolution = manager
        .resolve_metadata_from_challenge(challenge.as_deref())
        .await
        .map_err(rmcp_err("OAuth metadata discovery failed"))?;

    manager.set_metadata(resolution.metadata);

    let request = AuthorizationRequest::new(loopback_redirect_uri(port)).with_client_name(OAUTH_CLIENT_NAME);
    let request = match oauth.registration {
        OAuthClientRegistration::PreRegistered(client_id) => request.with_preregistered_client(client_id),
        OAuthClientRegistration::ClientMetadata(url) => request.with_client_metadata_url(url),
        OAuthClientRegistration::Dynamic => request,
    };
    let session = AuthorizationSession::new(manager, request)
        .await
        .map_err(|(_, error)| rmcp_err("OAuth authorization failed")(error))?;

    let callback_url = prompt_for_callback(server, session.get_authorization_url(), &listener, prompts).await?;
    session.handle_callback_url(&callback_url).await.map_err(rmcp_err("token exchange failed"))?;
    Ok(AuthClient::new(reqwest::Client::default(), session.auth_manager))
}

async fn prompt_for_callback(
    server: &str,
    auth_url: &str,
    listener: &TcpListener,
    prompts: &mpsc::Sender<Elicitation>,
) -> std::result::Result<String, OAuthError> {
    let request = ElicitRequestParams::UrlElicitationParams {
        meta: None,
        message: "Open this URL to authorize MCP server access.".to_string(),
        url: auth_url.to_string(),
        elicitation_id: OAUTH_ELICITATION_ID.to_string(),
    };
    let (elicitation, response) = ElicitationRequest::new(server.to_string(), request);
    prompts
        .send(Elicitation::Request(Box::new(elicitation)))
        .await
        .map_err(|_| OAuthError::Rmcp("OAuth prompt channel closed".to_string()))?;

    let result = tokio::select! {
        callback = accept_oauth_callback(listener) => callback,
        response = response => match response {
            Ok(result) if result.action == ElicitationAction::Accept => accept_oauth_callback(listener).await,
            Ok(_) | Err(_) => Err(OAuthError::UserCancelled),
        },
    };
    if !matches!(result, Err(OAuthError::UserCancelled)) {
        let completion = Elicitation::Complete { server: server.to_string(), id: OAUTH_ELICITATION_ID.to_string() };
        let _ = prompts.send(completion).await;
    }
    result
}

fn loopback_redirect_uri(port: u16) -> String {
    format!("http://localhost:{port}/")
}

fn rmcp_err(context: &'static str) -> impl Fn(AuthError) -> OAuthError {
    move |error| OAuthError::Rmcp(format!("{context}: {error}"))
}

fn store_error(error: &OAuthError) -> AuthError {
    AuthError::InternalError(error.to_string())
}
