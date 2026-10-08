use super::servers::{Shared, with_servers};
use crate::McpError;
use crate::client::{ClientOptions, McpClient, Transport};
use rmcp::model::Tool;
use std::time::Duration;
use tokio::sync::watch;
use tokio::time::timeout;
use tokio_util::task::AbortOnDropHandle;

pub(super) enum Credentials {
    Existing,
    SignIn { challenge: Option<String> },
}

pub(super) async fn run(
    servers: Shared,
    generation: u64,
    server: String,
    transport: Transport,
    credentials: Credentials,
    options: ClientOptions,
) {
    let opening = AbortOnDropHandle::new(tokio::spawn(open(server, credentials, transport, options)));
    let opened = opening.await.unwrap_or_else(|error| Err(McpError::ServerTask(error)));
    let (client, mut changes, tools) = match opened {
        Ok(opened) => opened,
        Err(error) => {
            with_servers(&servers, |servers| servers.settle(generation, Err(error)));
            return;
        }
    };
    let _close = client.close_on_drop();
    if with_servers(&servers, |servers| servers.settle(generation, Ok((client.clone(), tools)))).is_none() {
        return;
    }
    while changes.changed().await.is_ok() {
        let tools = client.list_tools().await;
        if with_servers(&servers, |servers| servers.refresh(generation, tools)).is_none() {
            return;
        }
    }
}

async fn open(
    server: String,
    credentials: Credentials,
    transport: Transport,
    options: ClientOptions,
) -> Result<(McpClient, watch::Receiver<()>, Vec<Tool>), McpError> {
    let client = match credentials {
        Credentials::Existing => McpClient::connect(&server, transport, &options).await?,
        Credentials::SignIn { challenge } => {
            timeout(AUTH_TIMEOUT, McpClient::authorize(&server, transport, &options, challenge))
                .await
                .map_err(|_| McpError::AuthTimedOut { server: server.clone(), timeout: AUTH_TIMEOUT })??
        }
    };
    let changes = client.tool_list_changes();
    let tools = client.list_tools().await?;
    Ok((client, changes, tools))
}

const AUTH_TIMEOUT: Duration = Duration::from_mins(3);
