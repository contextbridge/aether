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
    ResumeSessionResponse, SelectedPermissionOutcome, SessionCapabilities, SessionConfigOption, SessionId,
    SessionNotification, SetSessionConfigOptionRequest, SetSessionConfigOptionResponse,
};
use agent_client_protocol::{self as acp, Client, ConnectTo, ConnectionTo, JsonRpcRequest};
use std::sync::{Arc, Mutex};
use tokio::sync::{mpsc, oneshot};
use tracing::info;

type InitializeResult = Result<InitializeResponse, AcpClientError>;
type InitializeSender = Arc<Mutex<Option<oneshot::Sender<InitializeResult>>>>;
type Response<T> = oneshot::Sender<Result<T, AcpClientError>>;

/// A cloneable handle for issuing typed lifecycle requests and prompt commands.
#[derive(Clone)]
pub struct AcpClientHandle {
    cmd_tx: mpsc::UnboundedSender<ClientCommand>,
}

/// An initialized ACP connection that can create and manage multiple sessions.
pub struct AcpClient {
    pub initialize_response: InitializeResponse,
    pub agent_name: String,
    pub prompt_capabilities: PromptCapabilities,
    pub session_capabilities: SessionCapabilities,
    pub auth_methods: Vec<AuthMethod>,
    pub event_rx: mpsc::UnboundedReceiver<AcpEvent>,
    handle: AcpClientHandle,
}

/// The result of loading an ACP session, including notifications sent before the response.
pub struct LoadedSession {
    pub session_id: SessionId,
    pub response: LoadSessionResponse,
    pub replay: Vec<SessionNotification>,
}

/// ACP session with all handles needed by the caller.
pub struct AcpSession {
    pub session_id: SessionId,
    pub agent_name: String,
    pub prompt_capabilities: PromptCapabilities,
    pub session_capabilities: SessionCapabilities,
    pub config_options: Vec<SessionConfigOption>,
    pub auth_methods: Vec<AuthMethod>,
    pub event_rx: mpsc::UnboundedReceiver<AcpEvent>,
    pub client_handle: AcpClientHandle,
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

    let init_resp = init_rx
        .await
        .map_err(|_| AcpClientError::AgentCrashed("ACP task died during initialization".to_string()))??;
    let agent_name = init_resp
        .agent_info
        .as_ref()
        .map_or_else(|| "agent".to_string(), |info| info.title.as_deref().unwrap_or(&info.name).to_string());

    let handle = AcpClientHandle { cmd_tx };

    Ok(AcpClient {
        prompt_capabilities: init_resp.agent_capabilities.prompt_capabilities.clone(),
        session_capabilities: init_resp.agent_capabilities.session_capabilities.clone(),
        auth_methods: init_resp.auth_methods.clone(),
        initialize_response: init_resp,
        agent_name,
        event_rx,
        handle,
    })
}

impl AcpClient {
    /// Return a cloneable handle for session lifecycle operations.
    pub fn handle(&self) -> AcpClientHandle {
        self.handle.clone()
    }

    /// Create a new session on this initialized connection.
    pub async fn new_session(&self, request: NewSessionRequest) -> Result<NewSessionResponse, AcpClientError> {
        self.handle.new_session(request).await
    }

    /// List sessions using the official ACP pagination request and response types.
    pub async fn list_sessions(&self, request: ListSessionsRequest) -> Result<ListSessionsResponse, AcpClientError> {
        self.handle.list_sessions(request).await
    }

    /// Load a session and collect its replay notifications in wire order.
    pub async fn load_session(&self, request: LoadSessionRequest) -> Result<LoadedSession, AcpClientError> {
        self.handle.load_session(request).await
    }

    /// Resume a session without collecting or replaying its prior notifications.
    pub async fn resume_session(&self, request: ResumeSessionRequest) -> Result<ResumeSessionResponse, AcpClientError> {
        self.handle.resume_session(request).await
    }

    /// Close an active session.
    pub async fn close_session(&self, request: CloseSessionRequest) -> Result<CloseSessionResponse, AcpClientError> {
        self.handle.close_session(request).await
    }

    /// Search the agent's prompt history through Aether's ACP extension.
    pub async fn search_prompts(&self, params: PromptSearchParams) -> Result<PromptSearchResponse, AcpClientError> {
        self.handle.search_prompts(params).await
    }

    /// Load a session preview through Aether's ACP extension.
    pub async fn preview_session(
        &self,
        params: SessionPreviewParams,
    ) -> Result<SessionPreviewResponse, AcpClientError> {
        self.handle.preview_session(params).await
    }

    /// List workspaces through Aether's ACP extension.
    pub async fn list_workspaces(&self, params: WorkspaceListParams) -> Result<WorkspaceListResponse, AcpClientError> {
        self.handle.list_workspaces(params).await
    }

    /// Move a session through Aether's ACP extension.
    pub async fn move_workspace(&self, params: WorkspaceMoveParams) -> Result<WorkspaceMoveResponse, AcpClientError> {
        self.handle.move_workspace(params).await
    }

    pub async fn prompt(&self, request: PromptRequest) -> Result<PromptResponse, AcpClientError> {
        self.handle.prompt(request).await
    }

    pub async fn cancel(&self, request: CancelNotification) -> Result<(), AcpClientError> {
        self.handle.cancel(request).await
    }

    pub async fn set_config_option(
        &self,
        request: SetSessionConfigOptionRequest,
    ) -> Result<SetSessionConfigOptionResponse, AcpClientError> {
        self.handle.set_config_option(request).await
    }

    pub async fn authenticate_mcp_server(
        &self,
        request: crate::notifications::McpRequest,
    ) -> Result<(), AcpClientError> {
        self.handle.authenticate_mcp_server(request).await
    }

    pub async fn authenticate(&self, request: AuthenticateRequest) -> Result<AuthenticateResponse, AcpClientError> {
        self.handle.authenticate(request).await
    }
}

impl AcpClientHandle {
    pub async fn new_session(&self, request: NewSessionRequest) -> Result<NewSessionResponse, AcpClientError> {
        self.request(|response| ClientCommand::NewSession { request, response }).await
    }

    pub async fn list_sessions(&self, request: ListSessionsRequest) -> Result<ListSessionsResponse, AcpClientError> {
        self.request(|response| ClientCommand::ListSessions { request, response }).await
    }

    pub async fn load_session(&self, request: LoadSessionRequest) -> Result<LoadedSession, AcpClientError> {
        self.request(|response| ClientCommand::LoadSession { request, response }).await
    }

    pub async fn resume_session(&self, request: ResumeSessionRequest) -> Result<ResumeSessionResponse, AcpClientError> {
        self.request(|response| ClientCommand::ResumeSession { request, response }).await
    }

    pub async fn close_session(&self, request: CloseSessionRequest) -> Result<CloseSessionResponse, AcpClientError> {
        self.request(|response| ClientCommand::CloseSession { request, response }).await
    }

    pub async fn search_prompts(&self, params: PromptSearchParams) -> Result<PromptSearchResponse, AcpClientError> {
        self.request(|response| ClientCommand::SearchPrompts { params, response }).await
    }

    pub async fn preview_session(
        &self,
        params: SessionPreviewParams,
    ) -> Result<SessionPreviewResponse, AcpClientError> {
        self.request(|response| ClientCommand::PreviewSession { params, response }).await
    }

    pub async fn list_workspaces(&self, params: WorkspaceListParams) -> Result<WorkspaceListResponse, AcpClientError> {
        self.request(|response| ClientCommand::ListWorkspaces { params, response }).await
    }

    pub async fn move_workspace(&self, params: WorkspaceMoveParams) -> Result<WorkspaceMoveResponse, AcpClientError> {
        self.request(|response| ClientCommand::MoveWorkspace { params, response }).await
    }

    pub async fn prompt(&self, request: PromptRequest) -> Result<PromptResponse, AcpClientError> {
        self.request(|response| ClientCommand::Prompt { request, response }).await
    }

    pub async fn cancel(&self, request: CancelNotification) -> Result<(), AcpClientError> {
        self.request(|response| ClientCommand::Cancel { request, response }).await
    }

    pub async fn set_config_option(
        &self,
        request: SetSessionConfigOptionRequest,
    ) -> Result<SetSessionConfigOptionResponse, AcpClientError> {
        self.request(|response| ClientCommand::SetConfigOption { request, response }).await
    }

    pub async fn authenticate_mcp_server(
        &self,
        request: crate::notifications::McpRequest,
    ) -> Result<(), AcpClientError> {
        self.request(|response| ClientCommand::AuthenticateMcpServer { request, response }).await
    }

    pub async fn authenticate(&self, request: AuthenticateRequest) -> Result<AuthenticateResponse, AcpClientError> {
        self.request(|response| ClientCommand::Authenticate { request, response }).await
    }

    async fn request<T>(&self, make_command: impl FnOnce(Response<T>) -> ClientCommand) -> Result<T, AcpClientError> {
        let (response, receiver) = oneshot::channel();
        self.cmd_tx
            .send(make_command(response))
            .map_err(|_| AcpClientError::AgentCrashed("ACP task is no longer running".to_string()))?;
        receiver.await.map_err(|_| AcpClientError::AgentCrashed("ACP task ended before responding".to_string()))?
    }
}

/// Connect to an ACP agent, create one session, and retain the older session-shaped API.
pub async fn spawn_acp_session(
    agent: impl ConnectTo<Client> + 'static,
    init_request: InitializeRequest,
    new_session_request: NewSessionRequest,
) -> Result<AcpSession, AcpClientError> {
    let client = connect_acp_client(agent, init_request).await?;
    let session_resp = client.new_session(new_session_request).await?;
    let session_id = session_resp.session_id;
    let config_options = session_resp.config_options.unwrap_or_default();

    let AcpClient { agent_name, prompt_capabilities, session_capabilities, auth_methods, event_rx, handle, .. } =
        client;

    Ok(AcpSession {
        session_id,
        agent_name,
        prompt_capabilities,
        session_capabilities,
        config_options,
        auth_methods,
        event_rx,
        client_handle: handle,
    })
}

/// Connect, initialize, and load a session through the shared client implementation.
pub async fn spawn_loaded_acp_session(
    agent: impl ConnectTo<Client> + 'static,
    init_request: InitializeRequest,
    load_request: LoadSessionRequest,
) -> Result<(AcpClient, LoadedSession), AcpClientError> {
    let client = connect_acp_client(agent, init_request).await?;
    let loaded = client.load_session(load_request).await?;
    Ok((client, loaded))
}

/// Connect, initialize, and discover sessions without creating or loading one.
pub async fn discover_acp_sessions(
    agent: impl ConnectTo<Client> + 'static,
    init_request: InitializeRequest,
) -> Result<(AcpClient, ListSessionsResponse), AcpClientError> {
    let client = connect_acp_client(agent, init_request).await?;
    let sessions = client.list_sessions(ListSessionsRequest::new()).await?;
    Ok((client, sessions))
}

enum ClientCommand {
    Prompt { request: PromptRequest, response: Response<PromptResponse> },
    Cancel { request: CancelNotification, response: Response<()> },
    SetConfigOption { request: SetSessionConfigOptionRequest, response: Response<SetSessionConfigOptionResponse> },
    AuthenticateMcpServer { request: McpRequest, response: Response<()> },
    Authenticate { request: AuthenticateRequest, response: Response<AuthenticateResponse> },
    NewSession { request: NewSessionRequest, response: Response<NewSessionResponse> },
    ListSessions { request: ListSessionsRequest, response: Response<ListSessionsResponse> },
    LoadSession { request: LoadSessionRequest, response: Response<LoadedSession> },
    ResumeSession { request: ResumeSessionRequest, response: Response<ResumeSessionResponse> },
    CloseSession { request: CloseSessionRequest, response: Response<CloseSessionResponse> },
    SearchPrompts { params: PromptSearchParams, response: Response<PromptSearchResponse> },
    PreviewSession { params: SessionPreviewParams, response: Response<SessionPreviewResponse> },
    ListWorkspaces { params: WorkspaceListParams, response: Response<WorkspaceListResponse> },
    MoveWorkspace { params: WorkspaceMoveParams, response: Response<WorkspaceMoveResponse> },
}

struct ReplayState {
    session_id: SessionId,
    notifications: Vec<SessionNotification>,
}

#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
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
                    let should_buffer = replay_state
                        .lock()
                        .expect("replay state lock poisoned")
                        .as_ref()
                        .is_some_and(|state| state.session_id == notification.session_id);
                    if should_buffer {
                        replay_state
                            .lock()
                            .expect("replay state lock poisoned")
                            .as_mut()
                            .expect("replay state disappeared")
                            .notifications
                            .push(notification);
                    } else {
                        let SessionNotification { session_id, update, .. } = notification;
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
        let _ = event_tx.send(AcpEvent::ConnectionClosed);
    } else {
        let _ = event_tx.send(AcpEvent::ConnectionClosed);
    }
}

#[allow(clippy::too_many_arguments)]
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
                        let _ = event_tx.send(AcpEvent::PromptError(error.clone()));
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
        ClientCommand::Cancel { request, response } => {
            let result = cx.send_notification(request).map_err(AcpClientError::Protocol);
            let _ = response.send(result);
        }
        ClientCommand::SetConfigOption { request, response } => {
            send_typed_response(cx, request, response, state, true);
        }
        ClientCommand::AuthenticateMcpServer { request, response } => {
            let result = cx.send_notification(request).map_err(AcpClientError::Protocol);
            let _ = response.send(result);
        }
        ClientCommand::Authenticate { request, response } => {
            send_typed_response(cx, request, response, state, true);
        }
        ClientCommand::NewSession { request, response } => {
            send_typed_response(cx, request, response, state, false);
        }
        ClientCommand::ListSessions { request, response } => {
            send_typed_response(cx, request, response, state, false);
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
            let result = result.map(|response| LoadedSession { session_id, response, replay });
            let _ = response.send(result);
        }
        ClientCommand::ResumeSession { request, response } => {
            send_typed_response(cx, request, response, state, false);
        }
        ClientCommand::CloseSession { request, response } => {
            send_typed_response(cx, request, response, state, false);
        }
        ClientCommand::SearchPrompts { params, response } => {
            send_typed_response(cx, params, response, state, false);
        }
        ClientCommand::PreviewSession { params, response } => {
            send_typed_response(cx, params, response, state, false);
        }
        ClientCommand::ListWorkspaces { params, response } => {
            send_typed_response(cx, params, response, state, false);
        }
        ClientCommand::MoveWorkspace { params, response } => {
            send_typed_response(cx, params, response, state, false);
        }
    }
}

fn send_typed_response<T: JsonRpcRequest + 'static>(
    cx: &ConnectionTo<acp::Agent>,
    request: T,
    response: Response<T::Response>,
    state: ClientState,
    allow_during_prompt: bool,
) {
    if state == ClientState::Prompting && !allow_during_prompt {
        let _ = response.send(Err(AcpClientError::Busy));
        return;
    }
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
