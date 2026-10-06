use super::{
    McpClient, McpClientEvent, McpError, Result,
    config::McpHttpConfig,
    connection::{
        ConnectConfig, McpConnectAttempt, McpConnectOutcome, McpServerConnection, http_connection_error, new_client,
    },
    manager::RuntimeMcpTransport,
};
use crate::protocol::client_lifecycle_mode;
use aether_auth::{OAuthFlowOptions, OAuthHandler, create_auth_manager_from_store, perform_oauth_flow};
use rmcp::{
    serve_client_with_lifecycle,
    transport::{StreamableHttpClientTransport, auth::AuthClient},
};
use std::num::NonZeroU16;
use std::sync::{Arc, atomic::Ordering};
use tokio::sync::mpsc;

pub type OAuthHandlerFactory = Arc<dyn Fn(OAuthHandlerContext) -> Result<Arc<dyn OAuthHandler>> + Send + Sync>;

/// Context passed to an `OAuthHandlerFactory` so the constructed handler can
/// dispatch user-facing prompts back to the host through the MCP event channel.
#[derive(Clone)]
pub struct OAuthHandlerContext {
    pub server_name: String,
    pub callback_port: Option<NonZeroU16>,
    pub tx: mpsc::Sender<McpClientEvent>,
}

pub(super) async fn authenticate_http(
    name: String,
    config: McpHttpConfig,
    challenge: Option<String>,
    ctx: Arc<ConnectConfig>,
) -> McpConnectAttempt {
    let outcome = match async {
        let factory = ctx
            .oauth_handler_factory
            .as_ref()
            .ok_or_else(|| McpError::ConnectionFailed(format!("No OAuth handler factory available for '{name}'")))?;
        let tx = ctx
            .event_sender
            .clone()
            .ok_or_else(|| McpError::ConnectionFailed(format!("OAuth for '{name}' requires an MCP event sender")))?;
        let oauth = config
            .resolved_oauth()
            .ok_or_else(|| McpError::ConnectionFailed(format!("OAuth is not available for '{name}'")))?;
        let handler =
            factory(OAuthHandlerContext { server_name: name.clone(), callback_port: Some(oauth.callback_port), tx })?;

        let auth_client = perform_oauth_flow(
            &name,
            &config.transport.uri,
            handler.as_ref(),
            OAuthFlowOptions { client_registration: oauth.client_registration, challenge },
            ctx.oauth_credential_store.clone(),
        )
        .await
        .map_err(|e| McpError::ConnectionFailed(format!("OAuth failed for '{name}': {e}")))?;

        let generation = ctx.next_connection_generation.fetch_add(1, Ordering::Relaxed);
        let transport = StreamableHttpClientTransport::with_client(auth_client, config.transport.clone());
        let client =
            serve_client_with_lifecycle(new_client(&ctx, &name, generation), transport, client_lifecycle_mode())
                .await
                .map_err(|e| McpError::ConnectionFailed(format!("reconnect failed for '{name}': {e}")))?;
        Ok(McpServerConnection::from_parts(client, None, generation))
    }
    .await
    {
        Ok(conn) => McpConnectOutcome::Connected { conn, reauth_config: Some(config) },
        Err(error) => McpConnectOutcome::Failed { error },
    };

    McpConnectAttempt { name, outcome }
}

/// Connect over HTTP, authorizing with credentials stored by a previous OAuth flow, and
/// classify authorization failures as needing OAuth when a handler can run the flow.
pub(super) async fn connect_http(
    name: &str,
    config: McpHttpConfig,
    mcp_client: McpClient,
    ctx: &ConnectConfig,
    generation: u64,
) -> McpConnectOutcome {
    let result = if let Some(auth_client) = restore_auth_client(name, &config, ctx).await {
        let transport = StreamableHttpClientTransport::with_client(auth_client, config.transport.clone());
        serve_client_with_lifecycle(mcp_client, transport, client_lifecycle_mode()).await
    } else {
        let transport = StreamableHttpClientTransport::from_config(config.transport.clone());
        serve_client_with_lifecycle(mcp_client, transport, client_lifecycle_mode()).await
    };
    match result {
        Ok(client) => McpConnectOutcome::Connected {
            conn: McpServerConnection::from_parts(client, None, generation),
            reauth_config: None,
        },
        Err(error) => {
            let challenge = error.auth_challenge().map(str::to_string);
            let needs_oauth = ctx.oauth_handler_factory.is_some()
                && config.resolved_oauth().is_some()
                && (error.is_authorization_required() || challenge.is_some());
            let error = http_connection_error(name, &error);
            if needs_oauth {
                McpConnectOutcome::NeedsOAuth { config, challenge, error }
            } else {
                McpConnectOutcome::Failed { error }
            }
        }
    }
}

async fn restore_auth_client(
    name: &str,
    config: &McpHttpConfig,
    ctx: &ConnectConfig,
) -> Option<AuthClient<reqwest::Client>> {
    let (store, oauth) = (ctx.oauth_credential_store.as_ref()?, config.resolved_oauth()?);
    let manager = match create_auth_manager_from_store(
        name,
        &config.transport.uri,
        oauth.client_registration.pre_registered_client_id(),
        &oauth.redirect_uri(),
        Arc::clone(store),
    )
    .await
    {
        Ok(manager) => manager,
        Err(e) => {
            tracing::warn!(
                server = %name,
                error = %e,
                "Failed to initialize auth manager from stored credentials, proceeding without auth"
            );
            None
        }
    };
    manager.map(|manager| {
        tracing::debug!("Using OAuth for server '{name}'");
        AuthClient::new(reqwest::Client::default(), manager)
    })
}

pub(super) fn reauth_config_for(transport: &RuntimeMcpTransport, ctx: &ConnectConfig) -> Option<McpHttpConfig> {
    match transport {
        RuntimeMcpTransport::Http(config)
            if ctx.oauth_handler_factory.is_some() && !config.has_explicit_authorization() =>
        {
            Some(config.clone())
        }
        _ => None,
    }
}
