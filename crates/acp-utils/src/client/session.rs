use super::error::AcpClientError;
use super::event::AcpEvent;
use crate::notifications::{
    AuthMethodsUpdatedParams, ContextClearedParams, ContextCompactionParams, McpNotification, SubAgentProgressParams,
};
use agent_client_protocol::schema::v2::{
    AuthMethod, CancelSessionNotification, CreateElicitationRequest, InitializeRequest, InitializeResponse,
    NewSessionRequest, NewSessionResponse, PermissionOptionId, PermissionOptionKind, PromptCapabilities, PromptRequest,
    PromptResponse, ReplayFrom, ReplayFromStart, RequestPermissionOutcome, RequestPermissionRequest,
    RequestPermissionResponse, ResumeSessionRequest, ResumeSessionResponse, SelectedPermissionOutcome,
    SessionCapabilities, SessionId, SessionUpdate, StateUpdate, UpdateSessionNotification,
};
use agent_client_protocol::util::MatchDispatchFrom;
use agent_client_protocol::{self as acp, Client, ConnectTo, ConnectionTo, Dispatch, HandleDispatchFrom, Handled};
use std::future::Future;
use std::sync::{Arc, Mutex};
use tokio::sync::{mpsc, oneshot};
use tokio_util::sync::CancellationToken;
use tracing::info;

#[derive(Clone)]
pub struct AcpClientHandle {
    cx: ConnectionTo<acp::Agent>,
    state: Arc<Mutex<ClientState>>,
    connection: Arc<ClientConnection>,
}

pub struct AcpClient {
    pub initialize_response: InitializeResponse,
    pub event_rx: mpsc::UnboundedReceiver<AcpEvent>,
    pub handle: AcpClientHandle,
}

/// A resumed session snapshot.
pub struct ResumedSession {
    pub session_id: SessionId,
    pub response: ResumeSessionResponse,
    pub replay: Vec<AcpEvent>,
}

/// Connect to an ACP agent and complete initialization without creating a session.
pub async fn connect_acp_client(
    agent: impl ConnectTo<Client> + 'static,
    init_request: InitializeRequest,
) -> Result<AcpClient, AcpClientError> {
    let (event_tx, event_rx) = mpsc::unbounded_channel();
    let (init_tx, init_rx) = oneshot::channel();
    let connection =
        Arc::new(ClientConnection { shutdown: CancellationToken::new(), shutdown_complete: CancellationToken::new() });
    let state = Arc::new(Mutex::new(ClientState {
        event_tx: Some(event_tx),
        session_id: None,
        cancelled: false,
        restore: None,
        generation: 0,
    }));
    let shutdown = connection.shutdown.clone();
    let stopped = connection.shutdown_complete.clone();
    let driver_state = state.clone();
    tokio::spawn(async move {
        let _stopped = stopped.drop_guard();
        run_client_connection(agent, init_request, init_tx, driver_state, shutdown).await;
    });
    let (initialize_response, cx) = await_response(init_rx).await?;
    Ok(AcpClient { initialize_response, event_rx, handle: AcpClientHandle { cx, state, connection } })
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
    /// Stop this connection and wait for its transport and response tasks to
    /// be dropped. Does not send session/cancel or session/close.
    pub async fn disconnect(&self) {
        self.connection.shutdown.cancel();
        self.connection.shutdown_complete.cancelled().await;
    }

    pub fn prompt(
        &self,
        request: PromptRequest,
    ) -> impl Future<Output = Result<PromptResponse, AcpClientError>> + Send + use<> {
        let sent = {
            let mut state = self.state.lock().unwrap();
            state.session_id = Some(request.session_id.clone());
            state.cancelled = false;
            self.cx.send_request(request)
        };
        async move { sent.block_task().await.map_err(AcpClientError::Protocol) }
    }

    pub async fn resume_session_with_replay(
        &self,
        request: ResumeSessionRequest,
    ) -> Result<ResumeSessionResponse, AcpClientError> {
        self.resume(request.replay_from(ReplayFrom::Start(ReplayFromStart::new()))).await
    }

    /// Establish the new session ID before dispatching subsequent live updates.
    pub async fn new_session(&self, request: NewSessionRequest) -> Result<NewSessionResponse, AcpClientError> {
        let (response, receiver) = oneshot::channel();
        let state = self.state.clone();
        let cx = self.cx.clone();
        self.cx
            .spawn(async move {
                let _ = cx.send_request(request).on_receiving_result(move |result| async move {
                    if let Ok(metadata) = &result {
                        let mut state = state.lock().unwrap();
                        state.session_id = Some(metadata.session_id.clone());
                        state.cancelled = false;
                    }
                    let _ = response.send(result.map_err(AcpClientError::Protocol));
                    Ok(())
                });
                Ok(())
            })
            .map_err(AcpClientError::Protocol)?;
        await_response(receiver).await
    }

    pub async fn request<R: acp::JsonRpcRequest>(&self, request: R) -> Result<R::Response, AcpClientError> {
        self.cx.send_request(request).block_task().await.map_err(AcpClientError::Protocol)
    }

    /// Resume a session without collecting or replaying its prior notifications.
    pub async fn resume_session(&self, request: ResumeSessionRequest) -> Result<ResumeSessionResponse, AcpClientError> {
        self.resume(request.replay_from(None)).await
    }

    pub fn cancel(&self, request: CancelSessionNotification) -> Result<(), AcpClientError> {
        let mut state = self.state.lock().unwrap();
        let session_id = request.session_id.clone();
        self.cx.send_notification(request).map_err(AcpClientError::Protocol)?;
        if state.session_id.as_ref() == Some(&session_id) {
            state.cancelled = true;
        }
        if let Some(restore) = &mut state.restore
            && restore.session_id == session_id
        {
            restore.abandoned = true;
            restore.replay = None;
        }
        Ok(())
    }

    pub fn notify(&self, request: impl acp::JsonRpcNotification) -> Result<(), AcpClientError> {
        self.cx.send_notification(request).map_err(AcpClientError::Protocol)
    }

    async fn resume(&self, request: ResumeSessionRequest) -> Result<ResumeSessionResponse, AcpClientError> {
        let generation = {
            let mut state = self.state.lock().unwrap();
            if state.restore.is_some() {
                return Err(AcpClientError::RestorationPending);
            }
            state.generation += 1;
            let generation = state.generation;
            state.restore = Some(PendingRestore {
                generation,
                session_id: request.session_id.clone(),
                replay: request.replay_from.is_some().then(Vec::new),
                abandoned: false,
            });
            generation
        };
        let _caller = RestoreCaller { state: self.state.clone(), generation };
        let completion = RestoreCompletion { state: self.state.clone(), generation };
        let (response, receiver) = oneshot::channel();
        let cx = self.cx.clone();
        self.cx
            .spawn(async move {
                if response.is_closed() {
                    return Ok(());
                }
                let _ = cx.send_request(request).on_receiving_result(move |result| async move {
                    let mut state = completion.state.lock().unwrap();
                    if let Some(restore) = state.restore.take() {
                        if restore.abandoned || response.is_closed() {
                            let _ = response.send(Err(AcpClientError::RestorationCancelled));
                        } else {
                            if let Ok(metadata) = &result {
                                state.session_id = Some(restore.session_id.clone());
                                state.cancelled = false;
                                if let Some(replay) = restore.replay {
                                    state.emit(AcpEvent::SessionResumed(ResumedSession {
                                        session_id: restore.session_id,
                                        response: metadata.clone(),
                                        replay,
                                    }));
                                }
                            }
                            let _ = response.send(result.map_err(AcpClientError::Protocol));
                        }
                    }
                    drop(state);
                    drop(completion);
                    Ok(())
                });
                Ok(())
            })
            .map_err(AcpClientError::Protocol)?;
        await_response(receiver).await
    }
}

struct ClientConnection {
    shutdown: CancellationToken,
    shutdown_complete: CancellationToken,
}

impl Drop for ClientConnection {
    fn drop(&mut self) {
        self.shutdown.cancel();
    }
}

struct ClientState {
    event_tx: Option<mpsc::UnboundedSender<AcpEvent>>,
    session_id: Option<SessionId>,
    cancelled: bool,
    restore: Option<PendingRestore>,
    generation: u64,
}

struct PendingRestore {
    generation: u64,
    session_id: SessionId,
    replay: Option<Vec<AcpEvent>>,
    abandoned: bool,
}

struct RestoreCaller {
    state: Arc<Mutex<ClientState>>,
    generation: u64,
}

struct RestoreCompletion {
    state: Arc<Mutex<ClientState>>,
    generation: u64,
}

impl Drop for RestoreCaller {
    fn drop(&mut self) {
        let mut state = self.state.lock().unwrap();
        if let Some(restore) = &mut state.restore
            && restore.generation == self.generation
        {
            restore.abandoned = true;
            restore.replay = None;
        }
    }
}

impl Drop for RestoreCompletion {
    fn drop(&mut self) {
        let mut state = self.state.lock().unwrap();
        if state.restore.as_ref().is_some_and(|restore| restore.generation == self.generation) {
            state.restore = None;
        }
    }
}

impl ClientState {
    fn emit(&self, event: AcpEvent) {
        if let Some(sender) = &self.event_tx
            && let Err(error) = sender.send(event)
            && let AcpEvent::ElicitationRequest { responder, .. } = error.0
        {
            let _ = responder.respond_with_error(acp::Error::internal_error());
        }
    }

    fn replayable(&mut self, event: AcpEvent) {
        if let AcpEvent::SessionUpdate(notification) = &event {
            let target = self.restore.as_ref().map(|restore| &restore.session_id).or(self.session_id.as_ref());
            if target != Some(&notification.session_id) {
                return;
            }
        }
        if let Some(restore) = &mut self.restore {
            if restore.abandoned {
                return;
            }
            if let Some(replay) = &mut restore.replay {
                replay.push(event);
                return;
            }
        }
        if let AcpEvent::SessionUpdate(notification) = &event
            && matches!(notification.update, SessionUpdate::StateUpdate(StateUpdate::Idle(_)))
        {
            self.cancelled = false;
        }
        self.emit(event);
    }
}

async fn run_client_connection(
    agent: impl ConnectTo<Client> + 'static,
    init_request: InitializeRequest,
    init_tx: oneshot::Sender<Result<(InitializeResponse, ConnectionTo<acp::Agent>), AcpClientError>>,
    state: Arc<Mutex<ClientState>>,
    shutdown: CancellationToken,
) {
    let connection_result = Client
        .v2()
        .name("wisp")
        .with_handler(ClientHandlers(state.clone()))
        .connect_with(agent, async move |cx: ConnectionTo<acp::Agent>| {
            tokio::select! {
                () = async {
                    let result = cx.send_request(init_request).block_task().await.map_err(AcpClientError::Protocol);
                    let _ = init_tx.send(result.map(|response| {
                        info!("ACP initialized: protocol={:?}, agent_info={:?}", response.protocol_version, response.info);
                        (response, cx.clone())
                    }));
                    cx.incoming_closed().await;
                } => {},
                () = shutdown.cancelled() => {},
            }
            Ok(())
        })
        .await;
    if let Err(error) = connection_result {
        tracing::warn!("ACP connection exited with error: {error:?}");
    }
    let mut state = state.lock().unwrap();
    state.restore = None;
    state.emit(AcpEvent::ConnectionClosed);
    state.event_tx = None;
}

struct ClientHandlers(Arc<Mutex<ClientState>>);

impl HandleDispatchFrom<acp::Agent> for ClientHandlers {
    async fn handle_dispatch_from(
        &mut self,
        message: Dispatch,
        cx: ConnectionTo<acp::Agent>,
    ) -> Result<Handled<Dispatch>, acp::Error> {
        let state = &self.0;
        MatchDispatchFrom::new(message, &cx)
            .if_request(async |request: RequestPermissionRequest, responder| {
                let state = state.lock().unwrap();
                let outcome = if state.session_id.as_ref() != Some(&request.session_id) || state.cancelled {
                    RequestPermissionOutcome::Cancelled
                } else {
                    RequestPermissionOutcome::Selected(SelectedPermissionOutcome::new(auto_approve_option(&request)))
                };
                let _ = responder.respond(RequestPermissionResponse::new(outcome));
                Ok(())
            })
            .await
            .if_request(async |params: CreateElicitationRequest, responder| {
                state.lock().unwrap().emit(AcpEvent::ElicitationRequest { params: Box::new(params), responder });
                Ok(())
            })
            .await
            .if_notification(async |params: UpdateSessionNotification| {
                state.lock().unwrap().replayable(params.into());
                Ok(())
            })
            .await
            .if_notification(async |params: ContextCompactionParams| {
                state.lock().unwrap().replayable(AcpEvent::ContextCompaction(params));
                Ok(())
            })
            .await
            .if_notification(async |params: ContextClearedParams| {
                state.lock().unwrap().replayable(AcpEvent::ContextCleared(params));
                Ok(())
            })
            .await
            .if_notification(async |params: SubAgentProgressParams| {
                state.lock().unwrap().replayable(AcpEvent::SubAgentProgress(params));
                Ok(())
            })
            .await
            .if_notification(async |params: McpNotification| {
                state.lock().unwrap().replayable(AcpEvent::McpNotification(params));
                Ok(())
            })
            .await
            .if_notification(async |params: AuthMethodsUpdatedParams| {
                state.lock().unwrap().emit(AcpEvent::AuthMethodsUpdated(params));
                Ok(())
            })
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

fn auto_approve_option(req: &RequestPermissionRequest) -> PermissionOptionId {
    debug_assert!(!req.options.is_empty(), "ACP guarantees at least one permission option");
    req.options
        .iter()
        .find(|option| matches!(option.kind, PermissionOptionKind::AllowOnce | PermissionOptionKind::AllowAlways))
        .map_or_else(|| req.options[0].option_id.clone(), |option| option.option_id.clone())
}
