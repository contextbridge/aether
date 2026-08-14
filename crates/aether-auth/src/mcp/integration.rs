use super::credential_store::McpCredentialStore;
use crate::{OAuthCredentialStorage, OAuthError, OAuthHandler};
use rmcp::transport::auth::{
    AuthClient, AuthError, AuthorizationManager, AuthorizationRequest, AuthorizationSession, OAuthClientConfig,
};
use std::sync::Arc;

const OAUTH_CLIENT_NAME: &str = "Aether MCP Client";

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum OAuthClientRegistration {
    PreRegistered(String),
    ClientMetadata(String),
    #[default]
    Dynamic,
}

impl OAuthClientRegistration {
    pub fn pre_registered_client_id(&self) -> Option<&str> {
        match self {
            Self::PreRegistered(client_id) => Some(client_id),
            Self::ClientMetadata(_) | Self::Dynamic => None,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct OAuthFlowOptions {
    pub client_registration: OAuthClientRegistration,
    pub challenge: Option<String>,
}

pub async fn create_auth_manager_from_store(
    server_id: &str,
    base_url: &str,
    expected_client_id: Option<&str>,
    redirect_uri: &str,
    store: Arc<dyn OAuthCredentialStorage>,
) -> Result<Option<AuthorizationManager>, OAuthError> {
    let credential_store =
        McpCredentialStore::new(store, server_id.to_string()).with_expected_client_id(expected_client_id);
    let Some(stored) = credential_store.load_stored().await? else { return Ok(None) };
    let manager = {
        let mut manager = AuthorizationManager::new(base_url).await.map_err(rmcp_err("OAuth init failed"))?;
        manager.set_credential_store(credential_store);

        if !manager.initialize_from_store().await.map_err(rmcp_err("credential store load failed"))? {
            return Ok(None);
        }

        manager
            .configure_client(OAuthClientConfig::new(stored.client_id, redirect_uri))
            .map_err(rmcp_err("OAuth client restoration failed"))?;

        manager
    };

    Ok(Some(manager))
}

/// Run an interactive MCP OAuth flow using rmcp's state machine.
pub async fn perform_oauth_flow(
    server_id: &str,
    base_url: &str,
    handler: &dyn OAuthHandler,
    options: OAuthFlowOptions,
    store: Option<Arc<dyn OAuthCredentialStorage>>,
) -> Result<AuthClient<reqwest::Client>, OAuthError> {
    let mut manager = AuthorizationManager::new(base_url).await.map_err(rmcp_err("OAuth init failed"))?;
    if let Some(store) = store {
        manager.set_credential_store(
            McpCredentialStore::new(store, server_id.to_string())
                .with_expected_client_id(options.client_registration.pre_registered_client_id()),
        );
    }

    let resolution = manager
        .resolve_metadata_from_challenge(options.challenge.as_deref())
        .await
        .map_err(rmcp_err("OAuth metadata discovery failed"))?;
    manager.set_metadata(resolution.metadata);

    let request = AuthorizationRequest::new(handler.redirect_uri()).with_client_name(OAUTH_CLIENT_NAME);
    let session = AuthorizationSession::new(
        manager,
        match options.client_registration {
            OAuthClientRegistration::PreRegistered(client_id) => request.with_preregistered_client(client_id),
            OAuthClientRegistration::ClientMetadata(url) => request.with_client_metadata_url(url),
            OAuthClientRegistration::Dynamic => request,
        },
    )
    .await
    .map_err(|(_, error)| rmcp_err("OAuth authorization failed")(error))?;

    let callback_url = handler.authorize(session.get_authorization_url()).await?;
    session.handle_callback_url(&callback_url).await.map_err(rmcp_err("token exchange failed"))?;
    Ok(AuthClient::new(reqwest::Client::default(), session.auth_manager))
}

fn rmcp_err(context: &'static str) -> impl Fn(AuthError) -> OAuthError {
    move |error| OAuthError::Rmcp(format!("{context}: {error}"))
}
