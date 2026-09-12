use super::error::AcpClientError;
use super::event::{AcpEvent, ReplayableEvent};
use crate::notifications::{
    AuthMethodsUpdatedParams, ContextClearedParams, ContextCompactionParams, McpNotification, McpRequest,
    PromptSearchParams, PromptSearchResponse, SessionPreviewParams, SessionPreviewResponse, SessionUsageParams,
    SubAgentProgressParams, WorkspaceListParams, WorkspaceListResponse, WorkspaceMoveParams, WorkspaceMoveResponse,
};
use agent_client_protocol::schema::v2::{
    AuthMethod, CancelSessionNotification, CloseSessionRequest, CloseSessionResponse, CreateElicitationRequest,
    InitializeRequest, InitializeResponse, ListSessionsRequest, ListSessionsResponse, LoginAuthRequest,
    LoginAuthResponse, NewSessionRequest, NewSessionResponse, PermissionOptionId, PermissionOptionKind,
    PromptCapabilities, PromptRequest, PromptResponse, ReplayFrom, ReplayFromStart, RequestPermissionOutcome,
    RequestPermissionRequest, RequestPermissionResponse, ResumeSessionRequest, ResumeSessionResponse,
    SelectedPermissionOutcome, SessionCapabilities, SessionId, SessionUpdate, SetSessionConfigOptionRequest,
    SetSessionConfigOptionResponse, StateUpdate, StopReason, UpdateSessionNotification,
};
use agent_client_protocol::{
    self as acp, Client, ConnectTo, ConnectionTo, JsonRpcNotification, JsonRpcRequest, Responder,
};
use std::sync::Arc;
use tokio::sync::{mpsc, oneshot};
use tokio_util::sync::CancellationToken;
use tracing::info;

/// A cloneable handle for issuing typed lifecycle requests and prompt commands.
#[derive(Clone)]
pub struct AcpClientHandle {
    inbox: mpsc::UnboundedSender<ClientMessage>,
    connection: Arc<ClientConnection>,
}

/// An initialized ACP connection with at most one foreground turn or restoration at a time.
pub struct AcpClient {
    pub initialize_response: InitializeResponse,
    pub event_rx: mpsc::UnboundedReceiver<AcpEvent>,
    pub handle: AcpClientHandle,
}

/// A resumed session snapshot
pub struct ResumedSession {
    pub session_id: SessionId,
    pub response: ResumeSessionResponse,
    pub replay: Vec<ReplayableEvent>,
}

/// Connect to an ACP agent and complete initialization without creating a session.
pub async fn connect_acp_client(
    agent: impl ConnectTo<Client> + 'static,
    init_request: InitializeRequest,
) -> Result<AcpClient, AcpClientError> {
    let (event_tx, event_rx) = mpsc::unbounded_channel();
    let (message_tx, message_rx) = mpsc::unbounded_channel();
    let (init_tx, init_rx) = oneshot::channel();
    let connection =
        Arc::new(ClientConnection { shutdown: CancellationToken::new(), shutdown_complete: CancellationToken::new() });
    let shutdown = connection.shutdown.clone();
    let stopped = connection.shutdown_complete.clone();
    let state =
        ClientLoop { activity: Activity::Idle, event_tx, init_tx: Some(init_tx), cx: None, inbox: message_tx.clone() };
    tokio::spawn(async move {
        let _stopped = stopped.drop_guard();
        run_client_connection(agent, init_request, state, message_rx, shutdown).await;
    });

    let initialize_response = init_rx
        .await
        .map_err(|_| AcpClientError::AgentCrashed("ACP task died during initialization".to_string()))??;

    Ok(AcpClient { initialize_response, event_rx, handle: AcpClientHandle { inbox: message_tx, connection } })
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

    pub async fn prompt(&self, request: PromptRequest) -> Result<PromptResponse, AcpClientError> {
        let (response, receiver) = oneshot::channel();
        self.send(ClientCommand::Prompt { request, response })?;
        await_response(receiver).await
    }

    pub async fn resume_session_with_replay(
        &self,
        request: ResumeSessionRequest,
    ) -> Result<ResumeSessionResponse, AcpClientError> {
        self.resume(request.replay_from(ReplayFrom::Start(ReplayFromStart::new()))).await
    }

    pub async fn new_session(&self, request: NewSessionRequest) -> Result<NewSessionResponse, AcpClientError> {
        self.request(request, false).await
    }

    pub async fn list_sessions(&self, request: ListSessionsRequest) -> Result<ListSessionsResponse, AcpClientError> {
        self.request(request, false).await
    }

    /// Resume a session without collecting or replaying its prior notifications.
    pub async fn resume_session(&self, request: ResumeSessionRequest) -> Result<ResumeSessionResponse, AcpClientError> {
        self.resume(request.replay_from(None)).await
    }

    pub async fn close_session(&self, request: CloseSessionRequest) -> Result<CloseSessionResponse, AcpClientError> {
        self.request(request, false).await
    }

    /// Search the agent's prompt history through Aether's ACP extension.
    pub async fn search_prompts(&self, params: PromptSearchParams) -> Result<PromptSearchResponse, AcpClientError> {
        self.request(params, false).await
    }

    /// Load a session preview through Aether's ACP extension.
    pub async fn preview_session(
        &self,
        params: SessionPreviewParams,
    ) -> Result<SessionPreviewResponse, AcpClientError> {
        self.request(params, false).await
    }

    /// List workspaces through Aether's ACP extension.
    pub async fn list_workspaces(&self, params: WorkspaceListParams) -> Result<WorkspaceListResponse, AcpClientError> {
        self.request(params, false).await
    }

    /// Move a session through Aether's ACP extension.
    pub async fn move_workspace(&self, params: WorkspaceMoveParams) -> Result<WorkspaceMoveResponse, AcpClientError> {
        self.request(params, false).await
    }

    pub async fn set_config_option(
        &self,
        request: SetSessionConfigOptionRequest,
    ) -> Result<SetSessionConfigOptionResponse, AcpClientError> {
        self.request(request, true).await
    }

    pub async fn login(&self, request: LoginAuthRequest) -> Result<LoginAuthResponse, AcpClientError> {
        self.request(request, true).await
    }

    pub async fn cancel(&self, request: CancelSessionNotification) -> Result<(), AcpClientError> {
        let (response, receiver) = oneshot::channel();
        self.send(ClientCommand::Cancel { request, response })?;
        await_response(receiver).await
    }

    pub async fn authenticate_mcp_server(&self, request: McpRequest) -> Result<(), AcpClientError> {
        self.notify(request).await
    }

    async fn resume(&self, request: ResumeSessionRequest) -> Result<ResumeSessionResponse, AcpClientError> {
        let (response, receiver) = oneshot::channel();
        self.send(ClientCommand::ResumeSession { request, response })?;
        await_response(receiver).await
    }

    async fn request<T>(&self, request: T, allow_during_activity: bool) -> Result<T::Response, AcpClientError>
    where
        T: JsonRpcRequest + Send + 'static,
        T::Response: Send,
    {
        let (response, receiver) = oneshot::channel();
        self.send(ClientCommand::Request {
            allow_during_activity,
            run: Box::new(move |cx| match cx {
                Ok(cx) => send_typed_response(cx, request, response),
                Err(error) => {
                    let _ = response.send(Err(error));
                }
            }),
        })?;
        await_response(receiver).await
    }

    async fn notify<T>(&self, notification: T) -> Result<(), AcpClientError>
    where
        T: JsonRpcNotification + Send + 'static,
    {
        let (response, receiver) = oneshot::channel();
        self.send(ClientCommand::Request {
            allow_during_activity: true,
            run: Box::new(move |cx| {
                let result = cx.and_then(|cx| cx.send_notification(notification).map_err(AcpClientError::Protocol));
                let _ = response.send(result);
            }),
        })?;
        await_response(receiver).await
    }

    fn send(&self, command: ClientCommand) -> Result<(), AcpClientError> {
        self.inbox
            .send(ClientMessage::Command(command))
            .map_err(|_| AcpClientError::AgentCrashed("ACP task is no longer running".to_string()))
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

type Response<T> = oneshot::Sender<Result<T, AcpClientError>>;
type RequestFn = Box<dyn FnOnce(Result<&ConnectionTo<acp::Agent>, AcpClientError>) + Send>;

enum ClientCommand {
    Cancel { request: CancelSessionNotification, response: Response<()> },
    Prompt { request: PromptRequest, response: Response<PromptResponse> },
    ResumeSession { request: ResumeSessionRequest, response: Response<ResumeSessionResponse> },
    Request { allow_during_activity: bool, run: RequestFn },
}

enum ClientMessage {
    Command(ClientCommand),
    Initialized { cx: ConnectionTo<acp::Agent>, result: Result<Box<InitializeResponse>, acp::Error> },
    Replayable(ReplayableEvent),
    Event(AcpEvent),
    Permission { request: Box<RequestPermissionRequest>, responder: Responder<RequestPermissionResponse> },
    PromptResult { result: Result<PromptResponse, acp::Error>, response: Response<PromptResponse> },
    ResumeResult { result: Result<ResumeSessionResponse, acp::Error>, response: Response<ResumeSessionResponse> },
}

enum Activity {
    Idle,
    Prompting(ActiveTurn),
    Restoring { session_id: SessionId, replay: Option<Vec<ReplayableEvent>> },
}

struct ActiveTurn {
    session_id: SessionId,
    phase: TurnPhase,
    cancelled: bool,
}

#[derive(PartialEq, Eq)]
enum TurnPhase {
    AwaitingAcceptance,
    Running,
    CompletedAwaitingAcceptance,
}

struct ClientLoop {
    activity: Activity,
    event_tx: mpsc::UnboundedSender<AcpEvent>,
    init_tx: Option<Response<InitializeResponse>>,
    cx: Option<ConnectionTo<acp::Agent>>,
    inbox: mpsc::UnboundedSender<ClientMessage>,
}

impl ClientLoop {
    fn receive(&mut self, message: ClientMessage) {
        match message {
            ClientMessage::Command(command) => self.command(command),
            ClientMessage::Initialized { cx, result } => {
                self.cx = Some(cx);
                if let Some(response) = self.init_tx.take() {
                    let result = result
                        .map(|response| {
                            info!(
                                "ACP initialized: protocol={:?}, agent_info={:?}",
                                response.protocol_version, response.info
                            );
                            *response
                        })
                        .map_err(AcpClientError::Protocol);
                    let _ = response.send(result);
                }
            }
            ClientMessage::Replayable(event) => self.replayable(event),
            ClientMessage::Event(event) => {
                if let Err(error) = self.event_tx.send(event)
                    && let AcpEvent::ElicitationRequest { responder, .. } = error.0
                {
                    let _ = responder.respond_with_error(acp::Error::internal_error());
                }
            }
            ClientMessage::Permission { request, responder } => {
                let cancelled = matches!(&self.activity, Activity::Prompting(turn)
                    if turn.session_id == request.session_id && turn.cancelled
                        && turn.phase != TurnPhase::CompletedAwaitingAcceptance);
                let outcome = if cancelled {
                    RequestPermissionOutcome::Cancelled
                } else {
                    RequestPermissionOutcome::Selected(SelectedPermissionOutcome::new(auto_approve_option(&request)))
                };
                let _ = responder.respond(RequestPermissionResponse::new(outcome));
            }
            ClientMessage::PromptResult { result, response } => {
                if let Activity::Prompting(turn) = &mut self.activity {
                    if result.is_err() || turn.phase == TurnPhase::CompletedAwaitingAcceptance {
                        self.activity = Activity::Idle;
                    } else {
                        turn.phase = TurnPhase::Running;
                    }
                }
                let _ = response.send(result.map_err(AcpClientError::Protocol));
            }
            ClientMessage::ResumeResult { result, response } => {
                if let Activity::Restoring { session_id, replay: Some(replay) } =
                    std::mem::replace(&mut self.activity, Activity::Idle)
                    && let Ok(metadata) = &result
                {
                    let _ = self.event_tx.send(AcpEvent::SessionResumed(ResumedSession {
                        session_id,
                        response: metadata.clone(),
                        replay,
                    }));
                }
                let _ = response.send(result.map_err(AcpClientError::Protocol));
            }
        }
    }

    fn command(&mut self, command: ClientCommand) {
        let Some(cx) = &self.cx else { return };
        let busy = !matches!(self.activity, Activity::Idle);
        match command {
            ClientCommand::Cancel { request, response } => {
                let session_id = request.session_id.clone();
                let result = cx.send_notification(request).map_err(AcpClientError::Protocol);
                if result.is_ok()
                    && let Activity::Prompting(turn) = &mut self.activity
                    && turn.session_id == session_id
                {
                    turn.cancelled = true;
                }
                let _ = response.send(result);
            }
            ClientCommand::Prompt { request, response } => {
                if busy {
                    let _ = response.send(Err(AcpClientError::Busy));
                    return;
                }
                self.activity = Activity::Prompting(ActiveTurn {
                    session_id: request.session_id.clone(),
                    phase: TurnPhase::AwaitingAcceptance,
                    cancelled: false,
                });
                let inbox = self.inbox.clone();
                let _ = cx.send_request(request).on_receiving_result(move |result| async move {
                    let _ = inbox.send(ClientMessage::PromptResult { result, response });
                    Ok(())
                });
            }
            ClientCommand::ResumeSession { request, response } => {
                if busy {
                    let _ = response.send(Err(AcpClientError::Busy));
                    return;
                }
                self.activity = Activity::Restoring {
                    session_id: request.session_id.clone(),
                    replay: request.replay_from.is_some().then(Vec::new),
                };
                let inbox = self.inbox.clone();
                let _ = cx.send_request(request).on_receiving_result(move |result| async move {
                    let _ = inbox.send(ClientMessage::ResumeResult { result, response });
                    Ok(())
                });
            }
            ClientCommand::Request { allow_during_activity, run } => {
                run(if busy && !allow_during_activity { Err(AcpClientError::Busy) } else { Ok(cx) });
            }
        }
    }

    fn replayable(&mut self, event: ReplayableEvent) {
        if let Activity::Restoring { session_id, replay: Some(replay) } = &mut self.activity {
            let matches_session = match &event {
                ReplayableEvent::SessionUpdate(notification) => notification.session_id == *session_id,
                _ => true,
            };
            if matches_session {
                replay.push(event);
                return;
            }
        }
        let completion = if let ReplayableEvent::SessionUpdate(notification) = &event
            && let SessionUpdate::StateUpdate(StateUpdate::Idle(idle)) = &notification.update
            && let Activity::Prompting(turn) = &mut self.activity
            && turn.session_id == notification.session_id
            && turn.phase != TurnPhase::CompletedAwaitingAcceptance
        {
            if turn.phase == TurnPhase::Running {
                self.activity = Activity::Idle;
            } else {
                turn.phase = TurnPhase::CompletedAwaitingAcceptance;
            }
            Some(AcpEvent::PromptCompleted {
                session_id: notification.session_id.clone(),
                stop_reason: idle.stop_reason.clone().unwrap_or(StopReason::EndTurn),
            })
        } else {
            None
        };
        let _ = self.event_tx.send(event.into());
        if let Some(completion) = completion {
            let _ = self.event_tx.send(completion);
        }
    }
}

#[allow(clippy::too_many_lines)]
async fn run_client_connection(
    agent: impl ConnectTo<Client> + 'static,
    init_request: InitializeRequest,
    mut state: ClientLoop,
    mut messages: mpsc::UnboundedReceiver<ClientMessage>,
    shutdown: CancellationToken,
) {
    let inbox = &state.inbox;
    let connection_result = {
        let connection = Client
            .v2()
            .on_receive_request(
                {
                    let inbox = inbox.clone();
                    async move |request: RequestPermissionRequest, responder, _cx| {
                        let _ = inbox.send(ClientMessage::Permission { request: Box::new(request), responder });
                        Ok(())
                    }
                },
                acp::on_receive_request!(),
            )
            .on_receive_request(
                {
                    let inbox = inbox.clone();
                    async move |params: CreateElicitationRequest, responder, _cx| {
                        let _ = inbox.send(ClientMessage::Event(AcpEvent::ElicitationRequest {
                            params: Box::new(params),
                            responder,
                        }));
                        Ok(())
                    }
                },
                acp::on_receive_request!(),
            )
            .on_receive_notification(
                {
                    let inbox = inbox.clone();
                    async move |notification: UpdateSessionNotification, _cx| {
                        let _ = inbox.send(ClientMessage::Replayable(notification.into()));
                        Ok(())
                    }
                },
                acp::on_receive_notification!(),
            )
            .on_receive_notification(
                {
                    let inbox = inbox.clone();
                    async move |params: ContextCompactionParams, _cx| {
                        let _ = inbox.send(ClientMessage::Replayable(ReplayableEvent::ContextCompaction(params)));
                        Ok(())
                    }
                },
                acp::on_receive_notification!(),
            )
            .on_receive_notification(
                {
                    let inbox = inbox.clone();
                    async move |params: ContextClearedParams, _cx| {
                        let _ = inbox.send(ClientMessage::Replayable(ReplayableEvent::ContextCleared(params)));
                        Ok(())
                    }
                },
                acp::on_receive_notification!(),
            )
            .on_receive_notification(
                {
                    let inbox = inbox.clone();
                    async move |params: SubAgentProgressParams, _cx| {
                        let _ =
                            inbox.send(ClientMessage::Replayable(ReplayableEvent::SubAgentProgress(Box::new(params))));
                        Ok(())
                    }
                },
                acp::on_receive_notification!(),
            )
            .on_receive_notification(
                {
                    let inbox = inbox.clone();
                    async move |params: SessionUsageParams, _cx| {
                        let _ = inbox.send(ClientMessage::Replayable(ReplayableEvent::SessionUsage(Box::new(params))));
                        Ok(())
                    }
                },
                acp::on_receive_notification!(),
            )
            .on_receive_notification(
                {
                    let inbox = inbox.clone();
                    async move |params: AuthMethodsUpdatedParams, _cx| {
                        let _ = inbox.send(ClientMessage::Event(AcpEvent::AuthMethodsUpdated(params)));
                        Ok(())
                    }
                },
                acp::on_receive_notification!(),
            )
            .on_receive_notification(
                {
                    let inbox = inbox.clone();
                    async move |params: McpNotification, _cx| {
                        let _ = inbox.send(ClientMessage::Replayable(ReplayableEvent::McpNotification(params)));
                        Ok(())
                    }
                },
                acp::on_receive_notification!(),
            )
            .connect_with(agent, {
                let inbox = inbox.clone();
                async move |cx: ConnectionTo<acp::Agent>| {
                    let connection = cx.clone();
                    cx.send_request(init_request).on_receiving_result(move |result| async move {
                        let _ = inbox.send(ClientMessage::Initialized { cx: connection, result: result.map(Box::new) });
                        Ok(())
                    })?;
                    cx.incoming_closed().await;
                    Ok(())
                }
            });
        tokio::pin!(connection);
        loop {
            tokio::select! {
                result = &mut connection => break Some(result),
                () = shutdown.cancelled() => break None,
                Some(message) = messages.recv() => state.receive(message),
            }
        }
    };

    messages.close();
    if let Some(result) = connection_result {
        while let Ok(message) = messages.try_recv() {
            if !matches!(message, ClientMessage::Command(_)) {
                state.receive(message);
            }
        }
        if let Err(error) = result {
            tracing::warn!("ACP connection exited with error: {error:?}");
            if let Some(response) = state.init_tx.take() {
                let _ = response.send(Err(AcpClientError::ConnectFailed(error)));
            }
        }
    }
    let _ = state.event_tx.send(AcpEvent::ConnectionClosed);
}

async fn await_response<T>(receiver: oneshot::Receiver<Result<T, AcpClientError>>) -> Result<T, AcpClientError> {
    receiver.await.map_err(|_| AcpClientError::AgentCrashed("ACP task ended before responding".to_string()))?
}

fn send_typed_response<T: JsonRpcRequest + 'static>(
    cx: &ConnectionTo<acp::Agent>,
    request: T,
    response: Response<T::Response>,
) {
    let request = cx.send_request(request).block_task();
    if let Err(error) = cx.spawn(async move {
        let result = request.await.map_err(AcpClientError::Protocol);
        let _ = response.send(result);
        Ok(())
    }) {
        tracing::warn!("failed to spawn ACP request: {error:?}");
    }
}

fn auto_approve_option(req: &RequestPermissionRequest) -> PermissionOptionId {
    debug_assert!(!req.options.is_empty(), "ACP guarantees at least one permission option");
    req.options
        .iter()
        .find(|option| matches!(option.kind, PermissionOptionKind::AllowOnce | PermissionOptionKind::AllowAlways))
        .map_or_else(|| req.options[0].option_id.clone(), |option| option.option_id.clone())
}
