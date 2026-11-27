use super::error::ClientError;
use super::transport::ClientService;
use super::{ClientOptions, ToolCall, ToolCallOptions, Transport, handler::Handler};
use async_stream::stream;
use futures::future::{self, BoxFuture, FutureExt, Shared};
use futures::stream::Stream;
use rmcp::{
    RoleClient,
    model::{
        CallToolRequestParams, DEFAULT_MRTR_MAX_ROUNDS, GetPromptRequestParams, GetPromptResponse, GetPromptResult,
        GetSkillRequestParams, InputRequests, InputResponses, Prompt, ProtocolVersion,
        ReadResourceDirectoryRequestParams, ReadResourceDirectoryResult, ReadResourceRequestParams, ResourceContents,
        ServerCapabilities, Skill, SubscriptionFilter, Tool,
    },
    service::{Peer, QuitReason, ServiceError, Subscription},
};
use serde_json::{Map, Value};
use std::fmt;
use std::pin::pin;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::watch;
use tokio::task::JoinError;
use tokio::time::timeout;
use tokio_util::sync::{CancellationToken, DropGuard};

#[derive(Clone)]
pub struct McpClient {
    pub(super) peer: Peer<RoleClient>,
    pub(super) handler: Arc<Handler>,
    shutdown: Arc<Shutdown>,
    unauthorized: Arc<watch::Sender<Option<Unauthorized>>>,
}

#[derive(Clone, Debug)]
pub(crate) struct Unauthorized {
    pub(crate) challenge: Option<String>,
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

    pub fn capabilities(&self) -> ServerCapabilities {
        self.peer.peer_info().map_or_default(|info| info.capabilities.clone())
    }

    pub async fn list_tools(&self) -> Result<Vec<Tool>, ClientError> {
        if self.capabilities().tools.is_none() {
            return Ok(Vec::new());
        }
        self.peer.list_all_tools().await.map_err(|source| self.failed(source))
    }

    pub async fn list_prompts(&self) -> Result<Vec<Prompt>, ClientError> {
        if self.capabilities().prompts.is_none() {
            return Ok(Vec::new());
        }
        self.peer.list_all_prompts().await.map_err(|source| self.failed(source))
    }

    pub async fn get_prompt(&self, name: &str, args: Map<String, Value>) -> Result<GetPromptResult, ClientError> {
        let mut request = GetPromptRequestParams::new(name);
        if !args.is_empty() {
            request = request.with_arguments(args);
        }
        for _ in 0..DEFAULT_MRTR_MAX_ROUNDS {
            let response = self.peer.get_prompt_once(request.clone()).await;
            let required = match response.map_err(|source| self.failed(source))? {
                GetPromptResponse::Complete(result) => return Ok(result),
                GetPromptResponse::InputRequired(required) => required,
                _ => return Err(self.failed(ServiceError::UnexpectedResponse)),
            };
            request.input_responses = Some(self.answer_inputs(required.input_requests).await?);
            request.request_state = required.request_state;
        }
        Err(self.failed(ServiceError::InputRequiredRoundsExceeded { max_rounds: DEFAULT_MRTR_MAX_ROUNDS }))
    }

    pub async fn list_skills(&self) -> Result<Vec<Skill>, ClientError> {
        self.peer.list_all_skills().await.map_err(|source| self.failed(source))
    }

    pub async fn get_skill(&self, uri: &str) -> Result<Skill, ClientError> {
        let got = self.peer.get_skill(GetSkillRequestParams::new(uri)).await;
        got.map(|result| result.skill).map_err(|source| self.failed(source))
    }

    pub async fn read_resource(&self, uri: &str) -> Result<Vec<ResourceContents>, ClientError> {
        let read = self.peer.read_resource(ReadResourceRequestParams::new(uri)).await;
        read.map(|result| result.contents).map_err(|source| self.failed(source))
    }

    pub async fn read_directory(
        &self,
        request: ReadResourceDirectoryRequestParams,
    ) -> Result<ReadResourceDirectoryResult, ClientError> {
        self.peer.read_resource_directory(request).await.map_err(|source| self.failed(source))
    }

    pub fn tool_lists(&self) -> impl Stream<Item = Result<Vec<Tool>, ClientError>> + Send + use<> {
        let client = self.clone();
        let mut changes = self.handler.tool_list_changes();
        stream! {
            let mut subscription = client.tool_list_subscription().await;
            loop {
                yield client.list_tools().await;
                loop {
                    tokio::select! {
                        biased;
                        () = client.shutdown.closed.clone() => return,
                        notified = changes.changed() => match notified {
                            Ok(()) => break,
                            Err(_) => return,
                        },
                        reported = next_change(&mut subscription) => {
                            if reported {
                                break;
                            }
                            subscription = None;
                        }
                    }
                }
            }
        }
    }

    pub fn call_tool(&self, name: &str, args: Map<String, Value>, options: ToolCallOptions) -> ToolCall {
        let params = CallToolRequestParams::new(name.to_string()).with_arguments(args);
        ToolCall::new(self.clone(), params, options)
    }

    pub async fn close(&self) {
        self.shutdown.cancel.cancel();
        self.shutdown.closed.clone().await;
    }

    pub(crate) async fn unauthorized(&self) -> Unauthorized {
        let mut unauthorized = self.unauthorized.subscribe();
        let refused = unauthorized.wait_for(Option::is_some).await.expect("this client holds the sender");
        refused.clone().expect("waited for a refusal")
    }

    pub(super) fn observe(&self, error: &ServiceError) {
        if error.is_authorization_required() || error.auth_challenge().is_some() {
            let challenge = error.auth_challenge().map(str::to_string);
            self.unauthorized.send_replace(Some(Unauthorized { challenge }));
        }
    }

    pub(crate) fn close_on_drop(&self) -> DropGuard {
        self.shutdown.cancel.clone().drop_guard()
    }

    #[cfg(any(test, feature = "testing"))]
    pub fn peer(&self) -> &Peer<RoleClient> {
        &self.peer
    }

    async fn tool_list_subscription(&self) -> Option<Subscription> {
        let info = self.peer.peer_info()?;
        let announces_changes = info.capabilities.tools.as_ref().is_some_and(|tools| tools.list_changed == Some(true));
        if !announces_changes || info.protocol_version < ProtocolVersion::V_2026_07_28 {
            return None;
        }
        let filter = SubscriptionFilter::builder().tools_list_changed().build();
        let listening = self.peer.listen(filter).await;
        listening
            .inspect_err(
                |error| tracing::warn!(server = self.name(), %error, "Failed to subscribe to MCP tool list changes"),
            )
            .ok()
    }

    async fn answer_inputs(&self, requests: Option<InputRequests>) -> Result<InputResponses, ClientError> {
        let answered = self.handler.elicit_inputs(requests.unwrap_or_default()).await;
        answered
            .map(|(responses, _)| responses)
            .map_err(|_| ClientError::UnsupportedInput { server: self.name().into() })
    }

    fn failed(&self, source: ServiceError) -> ClientError {
        self.observe(&source);
        ClientError::request(self.name(), source)
    }

    fn from_service(service: ClientService) -> Self {
        let peer = service.peer().clone();
        let handler = Arc::clone(service.service());
        let cancel = CancellationToken::new();
        let closed = tokio::spawn(run_until_closed(service, cancel.clone())).map(|_| ()).boxed().shared();
        let shutdown = Shutdown { _cancel_on_drop: cancel.clone().drop_guard(), cancel, closed };
        Self { peer, handler, shutdown: Arc::new(shutdown), unauthorized: Arc::new(watch::Sender::new(None)) }
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

async fn next_change(subscription: &mut Option<Subscription>) -> bool {
    match subscription {
        Some(subscription) => matches!(subscription.next().await, Ok(Some(_))),
        None => future::pending().await,
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
