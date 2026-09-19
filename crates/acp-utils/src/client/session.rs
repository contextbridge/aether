use super::error::AcpClientError;
use super::event::AcpEvent;
use crate::notifications::{AuthMethodsUpdatedParams, ContextClearedParams, McpNotification, SubAgentProgressParams};
use agent_client_protocol::schema::v2::{
    AuthMethod, CancelSessionNotification, CreateElicitationRequest, InitializeRequest, InitializeResponse,
    NewSessionRequest, NewSessionResponse, PermissionOptionId, PermissionOptionKind, PromptCapabilities, PromptRequest,
    PromptResponse, ReplayFrom, ReplayFromStart, RequestPermissionOutcome, RequestPermissionRequest,
    RequestPermissionResponse, ResumeSessionRequest, ResumeSessionResponse, SelectedPermissionOutcome,
    SessionCapabilities, UpdateSessionNotification,
};
use agent_client_protocol::util::MatchDispatchFrom;
use agent_client_protocol::{
    self as acp, Client, ConnectTo, ConnectionTo, Dispatch, HandleDispatchFrom, Handled, V2ConnectionTo,
};
use std::future::Future;
use std::sync::Arc;
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use tracing::info;

#[derive(Clone)]
pub struct AcpClientHandle {
    cx: V2ConnectionTo<acp::Agent>,
    connection: Arc<ClientConnection>,
}

pub struct AcpClient {
    pub initialize_response: InitializeResponse,
    pub event_rx: mpsc::UnboundedReceiver<AcpEvent>,
    pub handle: AcpClientHandle,
}

/// Connect to an ACP agent and complete initialization without creating a session.
pub async fn connect_acp_client(
    agent: impl ConnectTo<Client> + 'static,
    init_request: InitializeRequest,
) -> Result<AcpClient, AcpClientError> {
    let (event_tx, event_rx) = mpsc::unbounded_channel();
    let (init_tx, init_rx) = oneshot::channel();
    let closed = CancellationToken::new();
    let events = ConnectionEvents { event_tx, closed: closed.clone() };
    let driver = tokio::spawn(run_client_connection(agent, init_request, init_tx, events));
    let connection = Arc::new(ClientConnection { driver, closed });
    let (initialize_response, cx) = await_response(init_rx).await?;
    Ok(AcpClient { initialize_response, event_rx, handle: AcpClientHandle { cx, connection } })
}

impl AcpClient {
    /// The agent's display title, falling back to its implementation name.
    pub fn agent_name(&self) -> String {
        let info = &self.initialize_response.info;
        info.title.as_deref().unwrap_or(&info.name).to_string()
    }

    pub fn prompt_capabilities(&self) -> Option<&PromptCapabilities> {
        self.session_capabilities().and_then(|session| session.prompt.as_ref())
    }

    pub fn session_capabilities(&self) -> Option<&SessionCapabilities> {
        self.initialize_response.capabilities.session.as_ref()
    }

    pub fn auth_methods(&self) -> &[AuthMethod] {
        &self.initialize_response.auth_methods
    }
}

impl AcpClientHandle {
    /// Stop this connection and wait for it to close without sending session/cancel or session/close.
    pub async fn disconnect(&self) {
        self.connection.driver.abort();
        self.connection.closed.cancelled().await;
    }

    pub fn prompt(
        &self,
        request: PromptRequest,
    ) -> impl Future<Output = Result<PromptResponse, AcpClientError>> + Send + use<> {
        self.request(request)
    }

    /// Request history as ordinary session updates, preceding the resume response.
    pub fn resume_session_with_replay(
        &self,
        request: ResumeSessionRequest,
    ) -> impl Future<Output = Result<ResumeSessionResponse, AcpClientError>> + Send + use<> {
        self.request(request.replay_from(ReplayFrom::Start(ReplayFromStart::new())))
    }

    pub fn new_session(
        &self,
        request: NewSessionRequest,
    ) -> impl Future<Output = Result<NewSessionResponse, AcpClientError>> + Send + use<> {
        self.request(request)
    }

    /// Send immediately; polling the returned future only waits for the response.
    pub fn request<R: acp::JsonRpcRequest>(
        &self,
        request: R,
    ) -> impl Future<Output = Result<R::Response, AcpClientError>> + Send + use<R> {
        let sent = self.cx.send_request(request);
        async move { sent.block_task().await.map_err(AcpClientError::Protocol) }
    }

    /// Resume a session without requesting history.
    pub fn resume_session(
        &self,
        request: ResumeSessionRequest,
    ) -> impl Future<Output = Result<ResumeSessionResponse, AcpClientError>> + Send + use<> {
        self.request(request.replay_from(None))
    }

    pub fn cancel(&self, request: CancelSessionNotification) -> Result<(), AcpClientError> {
        self.notify(request)
    }

    pub fn notify(&self, request: impl acp::JsonRpcNotification) -> Result<(), AcpClientError> {
        self.cx.send_notification(request).map_err(AcpClientError::Protocol)
    }
}

struct ClientConnection {
    driver: JoinHandle<()>,
    closed: CancellationToken,
}

impl Drop for ClientConnection {
    fn drop(&mut self) {
        self.driver.abort();
    }
}

struct ConnectionEvents {
    event_tx: mpsc::UnboundedSender<AcpEvent>,
    closed: CancellationToken,
}

impl Drop for ConnectionEvents {
    fn drop(&mut self) {
        let _ = self.event_tx.send(AcpEvent::ConnectionClosed);
        self.closed.cancel();
    }
}

async fn run_client_connection(
    agent: impl ConnectTo<Client> + 'static,
    init_request: InitializeRequest,
    init_tx: oneshot::Sender<Result<(InitializeResponse, V2ConnectionTo<acp::Agent>), AcpClientError>>,
    events: ConnectionEvents,
) {
    let connection_result = Client
        .v2()
        .name("wisp")
        .with_handler(ClientHandlers(events.event_tx.clone()))
        .connect_with(agent, async move |cx: V2ConnectionTo<acp::Agent>| {
            let result = cx.send_request(init_request).block_task().await.map_err(AcpClientError::Protocol);
            let _ = init_tx.send(result.map(|response| {
                info!("ACP initialized: protocol={:?}, agent_info={:?}", response.protocol_version, response.info);
                (response, cx.clone())
            }));
            cx.incoming_closed().await;
            Ok(())
        })
        .await;
    if let Err(error) = connection_result {
        tracing::warn!("ACP connection exited with error: {error:?}");
    }
}

struct ClientHandlers(mpsc::UnboundedSender<AcpEvent>);

impl HandleDispatchFrom<acp::Agent> for ClientHandlers {
    async fn handle_dispatch_from(
        &mut self,
        message: Dispatch,
        cx: ConnectionTo<acp::Agent>,
    ) -> Result<Handled<Dispatch>, acp::Error> {
        let emit = |event| {
            if let Err(error) = self.0.send(event)
                && let AcpEvent::ElicitationRequest { responder, .. } = error.0
            {
                let _ = responder.respond_with_error(acp::Error::internal_error());
            }
            Ok::<_, acp::Error>(())
        };
        MatchDispatchFrom::new(message, &cx)
            .if_request(async |request: RequestPermissionRequest, responder| {
                let outcome = auto_approve_option(&request).map_or(RequestPermissionOutcome::Cancelled, |option| {
                    RequestPermissionOutcome::Selected(SelectedPermissionOutcome::new(option))
                });
                let _ = responder.respond(RequestPermissionResponse::new(outcome));
                Ok(())
            })
            .await
            .if_request(async |params: CreateElicitationRequest, responder| {
                emit(AcpEvent::ElicitationRequest { params: Box::new(params), responder })
            })
            .await
            .if_notification(async |params: UpdateSessionNotification| emit(params.into()))
            .await
            .if_notification(async |params: ContextClearedParams| emit(AcpEvent::ContextCleared(params)))
            .await
            .if_notification(async |params: SubAgentProgressParams| emit(AcpEvent::SubAgentProgress(params)))
            .await
            .if_notification(async |params: McpNotification| emit(AcpEvent::McpNotification(params)))
            .await
            .if_notification(async |params: AuthMethodsUpdatedParams| emit(AcpEvent::AuthMethodsUpdated(params)))
            .await
            .done()
    }

    fn describe_chain(&self) -> impl std::fmt::Debug {
        "ClientHandlers"
    }
}

async fn await_response<T>(receiver: oneshot::Receiver<Result<T, AcpClientError>>) -> Result<T, AcpClientError> {
    receiver.await.map_err(|_| AcpClientError::AgentCrashed("ACP task ended before responding".to_string()))?
}

fn auto_approve_option(req: &RequestPermissionRequest) -> Option<PermissionOptionId> {
    req.options
        .iter()
        .find(|option| matches!(option.kind, PermissionOptionKind::AllowOnce | PermissionOptionKind::AllowAlways))
        .or_else(|| req.options.first())
        .map(|option| option.option_id.clone())
}
