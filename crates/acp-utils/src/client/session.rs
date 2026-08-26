use super::error::AcpClientError;
use super::event::AcpEvent;
use crate::notifications::{
    AuthMethodsUpdatedParams, ContextClearedParams, ContextCompactionParams, McpNotification, McpRequest,
    PromptSearchParams, PromptSearchResponse, SessionPreviewParams, SessionPreviewResponse, SubAgentProgressParams,
    WorkspaceListParams, WorkspaceListResponse, WorkspaceMoveParams, WorkspaceMoveResponse,
};
use agent_client_protocol::schema::v1::{
    AuthMethod, AuthenticateRequest, AuthenticateResponse, CancelNotification, CloseSessionRequest,
    CloseSessionResponse, CreateElicitationRequest, InitializeRequest, InitializeResponse, ListSessionsRequest,
    ListSessionsResponse, LoadSessionRequest, LoadSessionResponse, NewSessionRequest, NewSessionResponse,
    PermissionOptionId, PermissionOptionKind, PromptCapabilities, PromptRequest, PromptResponse,
    RequestPermissionOutcome, RequestPermissionRequest, RequestPermissionResponse, ResumeSessionRequest,
    ResumeSessionResponse, SelectedPermissionOutcome, SessionCapabilities, SessionId, SessionNotification,
    SetSessionConfigOptionRequest, SetSessionConfigOptionResponse,
};
use agent_client_protocol::{self as acp, Client, ConnectTo, ConnectionTo, JsonRpcNotification, JsonRpcRequest};
use std::sync::{Arc, Mutex};
use tokio::sync::{mpsc, oneshot};
use tracing::info;

/// A cloneable handle for issuing typed lifecycle requests and prompt commands.
#[derive(Clone)]
pub struct AcpClientHandle {
    cmd_tx: mpsc::UnboundedSender<ClientCommand>,
}

/// An initialized ACP connection that can create and manage multiple sessions.
pub struct AcpClient {
    pub initialize_response: InitializeResponse,
    pub event_rx: mpsc::UnboundedReceiver<AcpEvent>,
    pub handle: AcpClientHandle,
}

/// The result of loading an ACP session, including notifications sent before the response.
pub struct LoadedSession {
    pub session_id: SessionId,
    pub response: LoadSessionResponse,
    pub replay: Vec<SessionNotification>,
}

/// Connect to an ACP agent and complete initialization without creating a session.
pub async fn connect_acp_client(
    agent: impl ConnectTo<Client> + 'static,
    init_request: InitializeRequest,
) -> Result<AcpClient, AcpClientError> {
    let (event_tx, event_rx) = mpsc::unbounded_channel::<AcpEvent>();
    let (cmd_tx, cmd_rx) = mpsc::unbounded_channel::<ClientCommand>();
    let (init_tx, init_rx) = oneshot::channel::<InitializeResult>();
    let init_tx = Arc::new(Mutex::new(Some(init_tx)));
    let replay_state = Arc::new(Mutex::new(None));

    tokio::spawn(run_client_connection(
        agent,
        event_tx,
        cmd_rx,
        Arc::clone(&init_tx),
        init_request,
        Arc::clone(&replay_state),
    ));

    let initialize_response = init_rx
        .await
        .map_err(|_| AcpClientError::AgentCrashed("ACP task died during initialization".to_string()))??;

    Ok(AcpClient { initialize_response, event_rx, handle: AcpClientHandle { cmd_tx } })
}

impl AcpClient {
    /// The agent's display title, falling back to its implementation name.
    pub fn agent_name(&self) -> String {
        self.initialize_response
            .agent_info
            .as_ref()
            .map_or_else(|| "agent".to_string(), |info| info.title.as_deref().unwrap_or(&info.name).to_string())
    }

    pub fn prompt_capabilities(&self) -> &PromptCapabilities {
        &self.initialize_response.agent_capabilities.prompt_capabilities
    }

    pub fn session_capabilities(&self) -> &SessionCapabilities {
        &self.initialize_response.agent_capabilities.session_capabilities
    }

    pub fn auth_methods(&self) -> &[AuthMethod] {
        &self.initialize_response.auth_methods
    }
}

impl AcpClientHandle {
    pub fn detached() -> Self {
        let (cmd_tx, _) = mpsc::unbounded_channel();
        Self { cmd_tx }
    }

    pub async fn prompt(&self, request: PromptRequest) -> Result<PromptResponse, AcpClientError> {
        let (response, receiver) = oneshot::channel();
        self.send(ClientCommand::Prompt { request, response })?;
        await_response(receiver).await
    }

    /// Load a session and collect its replay notifications in wire order.
    pub async fn load_session(&self, request: LoadSessionRequest) -> Result<LoadedSession, AcpClientError> {
        let (response, receiver) = oneshot::channel();
        self.send(ClientCommand::LoadSession { request, response })?;
        await_response(receiver).await
    }

    pub async fn new_session(&self, request: NewSessionRequest) -> Result<NewSessionResponse, AcpClientError> {
        self.request(request, false).await
    }

    pub async fn list_sessions(&self, request: ListSessionsRequest) -> Result<ListSessionsResponse, AcpClientError> {
        self.request(request, false).await
    }

    /// Resume a session without collecting or replaying its prior notifications.
    pub async fn resume_session(&self, request: ResumeSessionRequest) -> Result<ResumeSessionResponse, AcpClientError> {
        self.request(request, false).await
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

    pub async fn authenticate(&self, request: AuthenticateRequest) -> Result<AuthenticateResponse, AcpClientError> {
        self.request(request, true).await
    }

    pub async fn cancel(&self, request: CancelNotification) -> Result<(), AcpClientError> {
        self.notify(request).await
    }

    pub async fn authenticate_mcp_server(&self, request: McpRequest) -> Result<(), AcpClientError> {
        self.notify(request).await
    }

    async fn request<T>(&self, request: T, allow_during_prompt: bool) -> Result<T::Response, AcpClientError>
    where
        T: JsonRpcRequest + Send + 'static,
        T::Response: Send,
    {
        let (response, receiver) = oneshot::channel();
        self.send(ClientCommand::Request {
            allow_during_prompt,
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
            allow_during_prompt: true,
            run: Box::new(move |cx| {
                let result = cx.and_then(|cx| cx.send_notification(notification).map_err(AcpClientError::Protocol));
                let _ = response.send(result);
            }),
        })?;
        await_response(receiver).await
    }

    fn send(&self, command: ClientCommand) -> Result<(), AcpClientError> {
        self.cmd_tx.send(command).map_err(|_| AcpClientError::AgentCrashed("ACP task is no longer running".to_string()))
    }
}

type InitializeResult = Result<InitializeResponse, AcpClientError>;
type InitializeSender = Arc<Mutex<Option<oneshot::Sender<InitializeResult>>>>;
type Response<T> = oneshot::Sender<Result<T, AcpClientError>>;
type RequestFn = Box<dyn FnOnce(Result<&ConnectionTo<acp::Agent>, AcpClientError>) + Send>;

enum ClientCommand {
    Prompt { request: PromptRequest, response: Response<PromptResponse> },
    LoadSession { request: LoadSessionRequest, response: Response<LoadedSession> },
    Request { allow_during_prompt: bool, run: RequestFn },
}

struct ReplayState {
    session_id: SessionId,
    notifications: Vec<SessionNotification>,
}

async fn await_response<T>(receiver: oneshot::Receiver<Result<T, AcpClientError>>) -> Result<T, AcpClientError> {
    receiver.await.map_err(|_| AcpClientError::AgentCrashed("ACP task ended before responding".to_string()))?
}

#[allow(clippy::too_many_lines)]
async fn run_client_connection(
    agent: impl ConnectTo<Client> + 'static,
    event_tx: mpsc::UnboundedSender<AcpEvent>,
    mut cmd_rx: mpsc::UnboundedReceiver<ClientCommand>,
    init_tx: InitializeSender,
    init_request: InitializeRequest,
    replay_state: Arc<Mutex<Option<ReplayState>>>,
) {
    let connection_result = Client
        .builder()
        .on_receive_request(
            async move |req: RequestPermissionRequest, responder, _cx| {
                responder.respond(RequestPermissionResponse::new(RequestPermissionOutcome::Selected(
                    SelectedPermissionOutcome::new(auto_approve_option(&req)),
                )))
            },
            acp::on_receive_request!(),
        )
        .on_receive_request(
            {
                let event_tx = event_tx.clone();
                async move |params: CreateElicitationRequest, responder, _cx| {
                    if let Err(send_err) =
                        event_tx.send(AcpEvent::ElicitationRequest { params: Box::new(params), responder })
                        && let AcpEvent::ElicitationRequest { responder, .. } = send_err.0
                    {
                        return responder.respond_with_error(acp::Error::internal_error());
                    }
                    Ok(())
                }
            },
            acp::on_receive_request!(),
        )
        .on_receive_notification(
            {
                let event_tx = event_tx.clone();
                let replay_state = Arc::clone(&replay_state);
                async move |notification: SessionNotification, _cx| {
                    let passthrough = {
                        let mut replay = replay_state.lock().expect("replay state lock poisoned");
                        match replay.as_mut() {
                            Some(state) if state.session_id == notification.session_id => {
                                state.notifications.push(notification);
                                None
                            }
                            _ => Some(notification),
                        }
                    };
                    if let Some(SessionNotification { session_id, update, .. }) = passthrough {
                        let _ = event_tx.send(AcpEvent::SessionUpdate { session_id, update: Box::new(update) });
                    }
                    Ok(())
                }
            },
            acp::on_receive_notification!(),
        )
        .on_receive_notification(
            {
                let event_tx = event_tx.clone();
                async move |params: ContextCompactionParams, _cx| {
                    let _ = event_tx.send(AcpEvent::ContextCompaction(params));
                    Ok(())
                }
            },
            acp::on_receive_notification!(),
        )
        .on_receive_notification(
            {
                let event_tx = event_tx.clone();
                async move |params: ContextClearedParams, _cx| {
                    let _ = event_tx.send(AcpEvent::ContextCleared(params));
                    Ok(())
                }
            },
            acp::on_receive_notification!(),
        )
        .on_receive_notification(
            {
                let event_tx = event_tx.clone();
                async move |params: SubAgentProgressParams, _cx| {
                    let _ = event_tx.send(AcpEvent::SubAgentProgress(params));
                    Ok(())
                }
            },
            acp::on_receive_notification!(),
        )
        .on_receive_notification(
            {
                let event_tx = event_tx.clone();
                async move |params: AuthMethodsUpdatedParams, _cx| {
                    let _ = event_tx.send(AcpEvent::AuthMethodsUpdated(params));
                    Ok(())
                }
            },
            acp::on_receive_notification!(),
        )
        .on_receive_notification(
            {
                let event_tx = event_tx.clone();
                async move |params: McpNotification, _cx| {
                    let _ = event_tx.send(AcpEvent::McpNotification(params));
                    Ok(())
                }
            },
            acp::on_receive_notification!(),
        )
        .connect_with(agent, {
            let event_tx = event_tx.clone();
            let init_tx = Arc::clone(&init_tx);
            async move |cx: ConnectionTo<acp::Agent>| {
                run_main(cx, event_tx, &mut cmd_rx, Arc::clone(&init_tx), init_request, replay_state).await;
                Ok(())
            }
        })
        .await;

    if let Err(e) = connection_result {
        tracing::warn!("ACP connection exited with error: {e:?}");
        send_initialization(&init_tx, Err(AcpClientError::ConnectFailed(e)));
    }
    let _ = event_tx.send(AcpEvent::ConnectionClosed);
}

async fn run_main(
    cx: ConnectionTo<acp::Agent>,
    event_tx: mpsc::UnboundedSender<AcpEvent>,
    cmd_rx: &mut mpsc::UnboundedReceiver<ClientCommand>,
    init_tx: InitializeSender,
    init_request: InitializeRequest,
    replay_state: Arc<Mutex<Option<ReplayState>>>,
) {
    let init_resp = match cx.send_request(init_request).block_task().await {
        Ok(response) => response,
        Err(error) => {
            send_initialization(&init_tx, Err(AcpClientError::Protocol(error)));
            return;
        }
    };
    info!("ACP initialized: protocol={:?}, agent_info={:?}", init_resp.protocol_version, init_resp.agent_info);
    if !send_initialization(&init_tx, Ok(init_resp)) {
        return;
    }

    while let Some(command) = cmd_rx.recv().await {
        handle_command(&cx, &event_tx, command, ClientState::Idle, &replay_state, cmd_rx).await;
    }
}

async fn run_prompt(
    cx: &ConnectionTo<acp::Agent>,
    event_tx: &mpsc::UnboundedSender<AcpEvent>,
    cmd_rx: &mut mpsc::UnboundedReceiver<ClientCommand>,
    replay_state: &Arc<Mutex<Option<ReplayState>>>,
    request: PromptRequest,
    response: Response<PromptResponse>,
) {
    let prompt_fut = cx.send_request(request).block_task();
    tokio::pin!(prompt_fut);

    loop {
        tokio::select! {
            result = &mut prompt_fut => {
                match result {
                    Ok(prompt_response) => {
                        let _ = event_tx.send(AcpEvent::PromptDone(prompt_response.stop_reason));
                        let _ = response.send(Ok(prompt_response));
                    }
                    Err(error) => {
                        let _ = response.send(Err(AcpClientError::Protocol(error)));
                    }
                }
                break;
            }
            Some(command) = cmd_rx.recv() => {
                Box::pin(handle_command(cx, event_tx, command, ClientState::Prompting, replay_state, cmd_rx)).await;
            }
            else => break,
        }
    }
}

fn send_initialization(sender: &InitializeSender, result: InitializeResult) -> bool {
    sender.lock().expect("initialization lock poisoned").take().is_some_and(|sender| sender.send(result).is_ok())
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ClientState {
    Idle,
    Prompting,
}

async fn handle_command(
    cx: &ConnectionTo<acp::Agent>,
    event_tx: &mpsc::UnboundedSender<AcpEvent>,
    command: ClientCommand,
    state: ClientState,
    replay_state: &Arc<Mutex<Option<ReplayState>>>,
    cmd_rx: &mut mpsc::UnboundedReceiver<ClientCommand>,
) {
    match command {
        ClientCommand::Prompt { request, response } => {
            if state == ClientState::Prompting {
                let _ = response.send(Err(AcpClientError::Busy));
            } else {
                Box::pin(run_prompt(cx, event_tx, cmd_rx, replay_state, request, response)).await;
            }
        }
        ClientCommand::LoadSession { request, response } => {
            if state == ClientState::Prompting {
                let _ = response.send(Err(AcpClientError::Busy));
                return;
            }
            let session_id = request.session_id.clone();
            *replay_state.lock().expect("replay state lock poisoned") =
                Some(ReplayState { session_id: session_id.clone(), notifications: vec![] });
            let result = cx.send_request(request).block_task().await.map_err(AcpClientError::Protocol);
            let replay = replay_state
                .lock()
                .expect("replay state lock poisoned")
                .take()
                .map_or_else(Vec::new, |state| state.notifications);
            let _ = response.send(result.map(|response| LoadedSession { session_id, response, replay }));
        }
        ClientCommand::Request { allow_during_prompt, run } => {
            if state == ClientState::Prompting && !allow_during_prompt {
                run(Err(AcpClientError::Busy));
            } else {
                run(Ok(cx));
            }
        }
    }
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
