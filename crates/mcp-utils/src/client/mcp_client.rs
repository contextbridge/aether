use super::error::ClientError;
use super::transport::ClientService;
use super::{ClientOptions, ToolCall, ToolCallOptions, Transport, handler::Handler};
use futures::future::{BoxFuture, FutureExt, Shared};
use futures::stream::{self, Stream};
use rmcp::{
    RoleClient,
    model::{CallToolRequestParams, GetPromptRequestParams, GetPromptResult, Prompt, Tool},
    service::{Peer, QuitReason},
};
use serde_json::{Map, Value};
use std::fmt;
use std::pin::pin;
use std::sync::Arc;
use std::time::Duration;
use tokio::task::JoinError;
use tokio::time::timeout;
use tokio_util::sync::{CancellationToken, DropGuard};

#[derive(Clone)]
pub struct McpClient {
    pub(super) peer: Peer<RoleClient>,
    pub(super) handler: Arc<Handler>,
    shutdown: Arc<Shutdown>,
}

impl McpClient {
    pub async fn connect(
        name: impl Into<String>,
        transport: Transport,
        options: &ClientOptions,
    ) -> Result<Self, ClientError> {
        transport.connect(Handler::new(name.into(), options), options).await.map(Self::from_service)
    }

    pub async fn authorize(
        name: impl Into<String>,
        transport: Transport,
        options: &ClientOptions,
        challenge: Option<String>,
    ) -> Result<Self, ClientError> {
        transport.authorize(Handler::new(name.into(), options), options, challenge).await.map(Self::from_service)
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

    pub async fn list_tools(&self) -> Result<Vec<Tool>, ClientError> {
        self.peer.list_all_tools().await.map_err(|source| ClientError::request(self.name(), source))
    }

    pub async fn list_prompts(&self) -> Result<Vec<Prompt>, ClientError> {
        if !self.supports_prompts() {
            return Ok(Vec::new());
        }
        self.peer.list_all_prompts().await.map_err(|source| ClientError::request(self.name(), source))
    }

    pub async fn get_prompt(&self, name: &str, args: Map<String, Value>) -> Result<GetPromptResult, ClientError> {
        let mut request = GetPromptRequestParams::new(name);
        if !args.is_empty() {
            request = request.with_arguments(args);
        }
        self.peer.get_prompt(request).await.map_err(|source| ClientError::request(self.name(), source))
    }

    pub fn tool_lists(&self) -> impl Stream<Item = Result<Vec<Tool>, ClientError>> + Send + use<> {
        let mut changes = self.handler.tool_list_changes();
        changes.mark_changed();
        stream::unfold((self.clone(), changes), |(client, mut changes)| async move {
            tokio::select! {
                biased;
                () = client.shutdown.closed.clone() => return None,
                notified = changes.changed() => notified.ok()?,
            }
            let tools = client.list_tools().await;
            Some((tools, (client, changes)))
        })
    }

    pub fn call_tool(&self, name: &str, args: Map<String, Value>, options: ToolCallOptions) -> ToolCall {
        let params = CallToolRequestParams::new(name.to_string()).with_arguments(args);
        ToolCall::new(self.clone(), params, options)
    }

    pub async fn close(&self) {
        self.shutdown.cancel.cancel();
        self.shutdown.closed.clone().await;
    }

    pub(crate) fn close_on_drop(&self) -> DropGuard {
        self.shutdown.cancel.clone().drop_guard()
    }

    #[cfg(any(test, feature = "testing"))]
    pub fn peer(&self) -> &Peer<RoleClient> {
        &self.peer
    }

    fn supports_prompts(&self) -> bool {
        self.peer.peer_info().is_some_and(|info| info.capabilities.prompts.is_some())
    }

    fn from_service(service: ClientService) -> Self {
        let peer = service.peer().clone();
        let handler = Arc::clone(service.service());
        let cancel = CancellationToken::new();
        let closed = tokio::spawn(run_until_closed(service, cancel.clone())).map(|_| ()).boxed().shared();
        let shutdown = Shutdown { _cancel_on_drop: cancel.clone().drop_guard(), cancel, closed };
        Self { peer, handler, shutdown: Arc::new(shutdown) }
    }
}

impl fmt::Debug for McpClient {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.debug_struct("McpClient").field("server", &self.name()).finish_non_exhaustive()
    }
}

struct Shutdown {
    cancel: CancellationToken,
    _cancel_on_drop: DropGuard,
    closed: Shared<BoxFuture<'static, ()>>,
}

const CLOSE_TIMEOUT: Duration = Duration::from_secs(5);

async fn run_until_closed(service: ClientService, cancel: CancellationToken) {
    let server = service.service().server().to_string();
    let stop = service.cancellation_token();
    let mut running = pin!(service.waiting());
    let closed = tokio::select! {
        closed = &mut running => closed.map(Some),
        () = cancel.cancelled() => {
            stop.cancel();
            match timeout(CLOSE_TIMEOUT, running).await {
                Ok(closed) => closed.map(Some),
                Err(_) => Ok(None),
            }
        }
    };
    log_close(&server, closed);
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
