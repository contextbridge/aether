use crate::{RemoteError, RemoteToolRuntime};
use futures::{StreamExt, future::BoxFuture, stream::BoxStream};
use mcp_utils::{
    client::{
        CallToolError, CallToolOptions, CancellationToken, McpClient, ToolCallEvent, call_tool_with_responder,
        client_capabilities,
    },
    request_context::GatewayRequestContext,
    tool_gateway::{
        UnixSocketMcpTransport, UnixSocketServer,
        service::{GatewayBackend, GatewayService},
    },
    transport::create_in_memory_transport,
};
use rmcp::{
    RoleClient, RoleServer, ServiceExt,
    model::{CallToolRequestParams, CallToolResult, ClientConfig, ErrorData, Implementation, Tool},
    service::{RequestContext, RunningService},
};
use serde_json::json;
use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::sync::mpsc;

pub(crate) struct GatewayResources {
    endpoint: PathBuf,
    socket: Mutex<Option<UnixSocketServer>>,
    cancel: Mutex<Option<rmcp::service::RunningServiceCancellationToken>>,
    server_task: tokio::task::JoinHandle<()>,
}

impl GatewayResources {
    pub async fn start(runtime: RemoteToolRuntime, transport: UnixSocketMcpTransport) -> Result<Self, RemoteError> {
        let (client_transport, server_transport) = create_in_memory_transport();
        let (events, receiver) = mpsc::channel(1);
        drop(receiver);
        let client = McpClient::new(
            ClientConfig::new(client_capabilities(), Implementation::new("remote-composition", "1")),
            "remote".into(),
            events,
        );
        let (server, client) = tokio::join!(
            runtime.clone().serve(server_transport),
            rmcp::serve_client_with_lifecycle(
                client,
                client_transport,
                rmcp::ClientLifecycleMode::Discover {
                    preferred_versions: vec![rmcp::model::ProtocolVersion::V_2026_07_28]
                }
            )
        );
        let server = server.map_err(|_| RemoteError::Invalid("could not start in-process gateway service".into()))?;
        let cancel = server.cancellation_token();
        let server_task = tokio::spawn(async move {
            let _ = server.waiting().await;
        });
        let client =
            Arc::new(client.map_err(|_| RemoteError::Invalid("could not start in-process gateway client".into()))?);
        let gateway = RemoteGateway { runtime, client };
        let endpoint = transport.path().to_path_buf();
        Ok(Self {
            endpoint,
            socket: Mutex::new(Some(transport.spawn(GatewayService::new(Arc::new(gateway))))),
            cancel: Mutex::new(Some(cancel)),
            server_task,
        })
    }

    pub fn endpoint(&self) -> &Path {
        &self.endpoint
    }

    pub fn shutdown(&self) {
        self.socket.lock().expect("gateway socket poisoned").take();
        if let Some(cancel) = self.cancel.lock().expect("gateway cancellation poisoned").take() {
            cancel.cancel();
        }
        self.server_task.abort();
    }
}

impl Drop for GatewayResources {
    fn drop(&mut self) {
        self.shutdown();
    }
}

struct RemoteGateway {
    runtime: RemoteToolRuntime,
    client: Arc<RunningService<RoleClient, McpClient>>,
}

impl GatewayBackend for RemoteGateway {
    fn tools(&self, context: RequestContext<RoleServer>) -> BoxFuture<'_, Result<Vec<Tool>, ErrorData>> {
        Box::pin(async move {
            let policy = GatewayRequestContext::from_meta(Some(&context.meta))
                .map_err(|error| ErrorData::invalid_params(error.to_string(), None))?;
            let mut tools = self.runtime.list_tools(None, context).await?.tools;
            tools.retain(|tool| !policy.defer_tools.is_model_visible_tool(&tool.name));
            Ok(tools)
        })
    }

    fn servers(&self, context: RequestContext<RoleServer>) -> BoxFuture<'_, Result<CallToolResult, ErrorData>> {
        Box::pin(async move {
            let tools = self.tools(context).await?;
            let servers = tools
                .iter()
                .filter_map(|tool| tool.name.split_once("__").map(|(server, _)| server))
                .collect::<BTreeSet<_>>();
            Ok(CallToolResult::structured(json!(
                servers.into_iter().map(|name| json!({"name":name,"description":name})).collect::<Vec<_>>()
            )))
        })
    }

    fn call(
        &self,
        mut request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
        cancel: CancellationToken,
    ) -> BoxStream<'static, ToolCallEvent> {
        let runtime = self.runtime.clone();
        let client = self.client.clone();
        let validation = async move {
            let policy = GatewayRequestContext::from_meta(Some(&context.meta))
                .map_err(|error| CallToolError::Unavailable { message: error.to_string() })?;
            if policy.defer_tools.is_model_visible_tool(&request.name) {
                return Err(CallToolError::Unavailable { message: "tool is not deferred".into() });
            }
            runtime
                .authorize(&request.name, &context)
                .await
                .map_err(|_| CallToolError::Unavailable { message: "tool is not permitted".into() })?;
            let responder = runtime
                .input_responder(&policy, &context)
                .await
                .map_err(|_| CallToolError::Unavailable { message: "invalid outer execution task".into() })?;
            request.meta = Some(context.meta.clone());
            Ok(call_tool_with_responder(
                client,
                request,
                CallToolOptions { timeout: Duration::from_secs(3600), meta: Some(context.meta), cancel },
                Some(responder),
            )
            .boxed())
        };
        futures::stream::once(validation)
            .flat_map(|result| match result {
                Ok(stream) => stream,
                Err(error) => futures::stream::once(async move { ToolCallEvent::Complete(Err(error)) }).boxed(),
            })
            .boxed()
    }
}

use rmcp::ServerHandler;
