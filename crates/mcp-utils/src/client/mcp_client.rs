use super::transport::Session;
use super::{ClientOptions, ToolCall, ToolCallOptions, Transport, handler::Handler};
use crate::error::McpError;
use futures::future::{BoxFuture, FutureExt, Shared};
use rmcp::{
    RoleClient,
    model::{CallToolRequestParams, GetPromptRequestParams, GetPromptResult, Prompt, Tool},
    service::{Peer, QuitReason},
};
use serde_json::{Map, Value};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::watch;
use tokio::task::JoinError;
use tokio_util::sync::{CancellationToken, DropGuard};

#[derive(Clone)]
pub struct McpClient {
    peer: Peer<RoleClient>,
    handler: Arc<Handler>,
    shutdown: Arc<Shutdown>,
}

impl McpClient {
    pub async fn connect(
        name: impl Into<String>,
        transport: Transport,
        options: &ClientOptions,
    ) -> Result<Self, McpError> {
        transport.connect(Handler::new(name.into(), options), options).await.map(Self::from_session)
    }

    pub async fn authorize(
        name: impl Into<String>,
        transport: Transport,
        options: &ClientOptions,
        challenge: Option<String>,
    ) -> Result<Self, McpError> {
        transport.authorize(Handler::new(name.into(), options), options, challenge).await.map(Self::from_session)
    }

    pub fn name(&self) -> &str {
        self.handler.server()
    }

    pub fn description(&self) -> Option<String> {
        let info = self.peer.peer_info()?;
        non_empty(info.server_info.as_ref()?.description.as_ref())
    }

    pub fn instructions(&self) -> Option<String> {
        non_empty(self.peer.peer_info()?.instructions.as_ref())
    }

    pub async fn list_tools(&self) -> Result<Vec<Tool>, McpError> {
        self.peer.list_all_tools().await.map_err(|source| McpError::request(self.name(), source))
    }

    pub async fn list_prompts(&self) -> Result<Vec<Prompt>, McpError> {
        if !self.supports_prompts() {
            return Ok(Vec::new());
        }
        self.peer.list_all_prompts().await.map_err(|source| McpError::request(self.name(), source))
    }

    pub async fn get_prompt(&self, name: &str, args: Map<String, Value>) -> Result<GetPromptResult, McpError> {
        let mut request = GetPromptRequestParams::new(name);
        if !args.is_empty() {
            request = request.with_arguments(args);
        }
        self.peer.get_prompt(request).await.map_err(|source| McpError::request(self.name(), source))
    }

    pub fn tool_list_changes(&self) -> watch::Receiver<()> {
        self.handler.tool_list_changes()
    }

    pub fn call_tool(&self, name: &str, args: Map<String, Value>, options: ToolCallOptions) -> ToolCall {
        let params = CallToolRequestParams::new(name.to_string()).with_arguments(args);
        ToolCall::new(self.clone(), params, options)
    }

    pub async fn close(&self) {
        self.shutdown.cancel.cancel();
        self.shutdown.closed.clone().await;
    }

    pub fn close_on_drop(&self) -> DropGuard {
        self.shutdown.cancel.clone().drop_guard()
    }

    pub(super) fn peer(&self) -> &Peer<RoleClient> {
        &self.peer
    }

    pub(super) fn handler(&self) -> &Handler {
        &self.handler
    }

    fn supports_prompts(&self) -> bool {
        self.peer.peer_info().is_some_and(|info| info.capabilities.prompts.is_some())
    }

    fn from_session(session: Session) -> Self {
        let peer = session.service.peer().clone();
        let handler = Arc::clone(session.service.service());
        let cancel = CancellationToken::new();
        let closed = tokio::spawn(close_once_cancelled(session, cancel.clone())).map(|_| ()).boxed().shared();
        let shutdown = Shutdown { _cancel_on_drop: cancel.clone().drop_guard(), cancel, closed };
        Self { peer, handler, shutdown: Arc::new(shutdown) }
    }
}

struct Shutdown {
    cancel: CancellationToken,
    _cancel_on_drop: DropGuard,
    closed: Shared<BoxFuture<'static, ()>>,
}

const CLOSE_TIMEOUT: Duration = Duration::from_secs(5);

async fn close_once_cancelled(Session { mut service, hosted }: Session, cancel: CancellationToken) {
    cancel.cancelled().await;
    let server = service.service().server().to_string();
    log_close(&server, service.close_with_timeout(CLOSE_TIMEOUT).await);
    if let Some(mut hosted) = hosted {
        log_close(&server, hosted.close_with_timeout(CLOSE_TIMEOUT).await);
    }
}

fn non_empty(text: Option<&String>) -> Option<String> {
    text.filter(|text| !text.is_empty()).cloned()
}

fn log_close(server: &str, result: Result<Option<QuitReason>, JoinError>) {
    match result {
        Ok(Some(reason)) => tracing::debug!(server, ?reason, "MCP server connection closed"),
        Ok(None) => tracing::warn!(server, "MCP server connection did not close within {CLOSE_TIMEOUT:?}"),
        Err(error) => tracing::warn!(server, %error, "MCP server connection task failed while closing"),
    }
}
