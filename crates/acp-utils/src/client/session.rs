use super::error::AcpClientError;
use super::event::AcpEvent;
use super::prompt_handle::{AcpPromptHandle, PromptCommand};
use crate::notifications::{
    AuthMethodsUpdatedParams, ContextClearedParams, ContextCompactionParams, McpNotification, McpRequest,
    PromptSearchParams, PromptSearchResponse, SessionPreviewParams, SessionPreviewResponse, SubAgentProgressParams,
};
use agent_client_protocol::schema::v1::{
    AuthMethod, AuthenticateRequest, CancelNotification, CloseSessionRequest, CloseSessionResponse, ConfigOptionUpdate,
    ContentBlock, CreateElicitationRequest, InitializeRequest, InitializeResponse, ListSessionsRequest,
    ListSessionsResponse, LoadSessionRequest, LoadSessionResponse, NewSessionRequest, NewSessionResponse,
    PermissionOptionId, PermissionOptionKind, PromptCapabilities, PromptRequest, RequestPermissionOutcome,
    RequestPermissionRequest, RequestPermissionResponse, ResumeSessionRequest, ResumeSessionResponse,
    SelectedPermissionOutcome, SessionCapabilities, SessionConfigOption, SessionId, SessionNotification, SessionUpdate,
    SetSessionConfigOptionRequest, TextContent,
};
use agent_client_protocol::{self as acp, Client, ConnectTo, ConnectionTo, JsonRpcRequest};
use std::future::Future;
use std::sync::{Arc, Mutex};
use tokio::sync::{mpsc, oneshot};
use tracing::info;

type InitializeResult = Result<InitializeResponse, AcpClientError>;
type InitializeSender = Arc<Mutex<Option<oneshot::Sender<InitializeResult>>>>;
type Response<T> = oneshot::Sender<Result<T, AcpClientError>>;

/// An initialized ACP connection that can create and manage multiple sessions.
pub struct AcpClient {
    pub initialize_response: InitializeResponse,
    pub agent_name: String,
    pub prompt_capabilities: PromptCapabilities,
    pub session_capabilities: SessionCapabilities,
    pub auth_methods: Vec<AuthMethod>,
    pub event_rx: mpsc::UnboundedReceiver<AcpEvent>,
    pub prompt_handle: AcpPromptHandle,
    cmd_tx: mpsc::UnboundedSender<ClientCommand>,
}

/// The result of loading an ACP session, including notifications sent before the response.
pub struct LoadedSession {
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
    pub prompt_handle: AcpPromptHandle,
}

/// Connect to an ACP agent and complete initialization without creating a session.
pub async fn connect_acp_client(
    agent: impl ConnectTo<Client> + 'static,
    init_request: InitializeRequest,
) -> Result<AcpClient, AcpClientError> {
    let (event_tx, event_rx) = mpsc::unbounded_channel::<AcpEvent>();
    let (legacy_tx, legacy_rx) = mpsc::unbounded_channel::<PromptCommand>();
    let (cmd_tx, cmd_rx) = mpsc::unbounded_channel::<ClientCommand>();
    let (init_tx, init_rx) = oneshot::channel::<InitializeResult>();
    let init_tx = Arc::new(Mutex::new(Some(init_tx)));
    let replay_state = Arc::new(Mutex::new(None));

    tokio::spawn(run_client_connection(
        agent,
        event_tx,
        legacy_rx,
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

    Ok(AcpClient {
        prompt_capabilities: init_resp.agent_capabilities.prompt_capabilities.clone(),
        session_capabilities: init_resp.agent_capabilities.session_capabilities.clone(),
        auth_methods: init_resp.auth_methods.clone(),
        initialize_response: init_resp,
        agent_name,
        event_rx,
        prompt_handle: AcpPromptHandle { cmd_tx: legacy_tx },
        cmd_tx,
    })
}

impl AcpClient {
    /// Create a new session on this initialized connection.
    pub async fn new_session(&self, request: NewSessionRequest) -> Result<NewSessionResponse, AcpClientError> {
        self.request(|response| ClientCommand::NewSession { request, response }).await
    }

    /// List sessions using the official ACP pagination request and response types.
    pub async fn list_sessions(&self, request: ListSessionsRequest) -> Result<ListSessionsResponse, AcpClientError> {
        self.request(|response| ClientCommand::ListSessions { request, response }).await
    }

    /// Load a session and collect its replay notifications in wire order.
    pub async fn load_session(&self, request: LoadSessionRequest) -> Result<LoadedSession, AcpClientError> {
        self.request(|response| ClientCommand::LoadSession { request, response }).await
    }

    /// Resume a session without collecting or replaying its prior notifications.
    pub async fn resume_session(&self, request: ResumeSessionRequest) -> Result<ResumeSessionResponse, AcpClientError> {
        self.request(|response| ClientCommand::ResumeSession { request, response }).await
    }

    /// Close an active session.
    pub async fn close_session(&self, request: CloseSessionRequest) -> Result<CloseSessionResponse, AcpClientError> {
        self.request(|response| ClientCommand::CloseSession { request, response }).await
    }

    /// Search the agent's prompt history through Aether's ACP extension.
    pub async fn search_prompts(&self, params: PromptSearchParams) -> Result<PromptSearchResponse, AcpClientError> {
        self.request(|response| ClientCommand::SearchPrompts { params, response }).await
    }

    /// Load a session preview through Aether's ACP extension.
    pub async fn preview_session(
        &self,
        params: SessionPreviewParams,
    ) -> Result<SessionPreviewResponse, AcpClientError> {
        self.request(|response| ClientCommand::PreviewSession { params, response }).await
    }

    /// Send a prompt using the legacy event-stream command interface.
    pub fn prompt(
        &self,
        session_id: &SessionId,
        text: &str,
        content: Option<Vec<ContentBlock>>,
    ) -> Result<(), AcpClientError> {
        self.prompt_handle.prompt(session_id, text, content)
    }

    /// Cancel a prompt using the legacy event-stream command interface.
    pub fn cancel(&self, session_id: &SessionId) -> Result<(), AcpClientError> {
        self.prompt_handle.cancel(session_id)
    }

    /// Change a session configuration option using the legacy event-stream command interface.
    pub fn set_config_option(
        &self,
        session_id: &SessionId,
        config_id: &str,
        value: &str,
    ) -> Result<(), AcpClientError> {
        self.prompt_handle.set_config_option(session_id, config_id, value)
    }

    /// Authenticate an MCP server using the legacy event-stream command interface.
    pub fn authenticate_mcp_server(&self, session_id: &SessionId, server_name: &str) -> Result<(), AcpClientError> {
        self.prompt_handle.authenticate_mcp_server(session_id, server_name)
    }

    /// Authenticate using the legacy event-stream command interface.
    pub fn authenticate(&self, method_id: &str) -> Result<(), AcpClientError> {
        self.prompt_handle.authenticate(method_id)
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

    let AcpClient {
        agent_name, prompt_capabilities, session_capabilities, auth_methods, event_rx, prompt_handle, ..
    } = client;

    Ok(AcpSession {
        session_id,
        agent_name,
        prompt_capabilities,
        session_capabilities,
        config_options,
        auth_methods,
        event_rx,
        prompt_handle,
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
    Legacy(PromptCommand),
    NewSession { request: NewSessionRequest, response: Response<NewSessionResponse> },
    ListSessions { request: ListSessionsRequest, response: Response<ListSessionsResponse> },
    LoadSession { request: LoadSessionRequest, response: Response<LoadedSession> },
    ResumeSession { request: ResumeSessionRequest, response: Response<ResumeSessionResponse> },
    CloseSession { request: CloseSessionRequest, response: Response<CloseSessionResponse> },
    SearchPrompts { params: PromptSearchParams, response: Response<PromptSearchResponse> },
    PreviewSession { params: SessionPreviewParams, response: Response<SessionPreviewResponse> },
}

struct ReplayState {
    session_id: SessionId,
    notifications: Vec<SessionNotification>,
}

#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
async fn run_client_connection(
    agent: impl ConnectTo<Client> + 'static,
    event_tx: mpsc::UnboundedSender<AcpEvent>,
    mut legacy_rx: mpsc::UnboundedReceiver<PromptCommand>,
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
                run_main(cx, event_tx, &mut legacy_rx, &mut cmd_rx, Arc::clone(&init_tx), init_request, replay_state)
                    .await;
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
    legacy_rx: &mut mpsc::UnboundedReceiver<PromptCommand>,
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

    while let Some(command) = receive_command(cmd_rx, legacy_rx).await {
        match command {
            ClientCommand::Legacy(PromptCommand::Prompt { session_id, text, content }) => {
                run_prompt(&cx, &event_tx, cmd_rx, legacy_rx, &replay_state, session_id, text, content).await;
            }
            command => handle_command(&cx, &event_tx, command, ClientState::Idle, &replay_state).await,
        }
    }
}

async fn receive_command(
    cmd_rx: &mut mpsc::UnboundedReceiver<ClientCommand>,
    legacy_rx: &mut mpsc::UnboundedReceiver<PromptCommand>,
) -> Option<ClientCommand> {
    tokio::select! {
        Some(command) = cmd_rx.recv() => Some(command),
        Some(command) = legacy_rx.recv() => Some(ClientCommand::Legacy(command)),
        else => None,
    }
}

#[allow(clippy::too_many_arguments)]
async fn run_prompt(
    cx: &ConnectionTo<acp::Agent>,
    event_tx: &mpsc::UnboundedSender<AcpEvent>,
    cmd_rx: &mut mpsc::UnboundedReceiver<ClientCommand>,
    legacy_rx: &mut mpsc::UnboundedReceiver<PromptCommand>,
    replay_state: &Arc<Mutex<Option<ReplayState>>>,
    session_id: SessionId,
    text: String,
    content: Option<Vec<ContentBlock>>,
) {
    let mut prompt = vec![ContentBlock::Text(TextContent::new(text))];
    if let Some(extra_content) = content {
        prompt.extend(extra_content);
    }
    let prompt_fut = cx.send_request(PromptRequest::new(session_id, prompt)).block_task();
    tokio::pin!(prompt_fut);

    loop {
        tokio::select! {
            result = &mut prompt_fut => {
                let event = match result {
                    Ok(response) => AcpEvent::PromptDone(response.stop_reason),
                    Err(error) => AcpEvent::PromptError(error),
                };
                let _ = event_tx.send(event);
                break;
            }
            command = receive_command(cmd_rx, legacy_rx) => {
                if let Some(command) = command {
                    handle_command(cx, event_tx, command, ClientState::Prompting, replay_state).await;
                } else {
                    break;
                }
            }
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
) {
    match command {
        ClientCommand::Legacy(PromptCommand::Prompt { .. }) => {
            tracing::warn!("ignoring duplicate Prompt while one is in-flight");
        }
        ClientCommand::Legacy(command) => handle_legacy_command(cx, event_tx, command, state).await,
        ClientCommand::NewSession { request, response } => {
            send_typed_response(cx, request, response, state).await;
        }
        ClientCommand::ListSessions { request, response } => {
            send_typed_response(cx, request, response, state).await;
        }
        ClientCommand::LoadSession { request, response } => {
            if state == ClientState::Prompting {
                let _ = response.send(Err(AcpClientError::Busy));
                return;
            }
            let session_id = request.session_id.clone();
            *replay_state.lock().expect("replay state lock poisoned") =
                Some(ReplayState { session_id, notifications: vec![] });
            let result = cx.send_request(request).block_task().await.map_err(AcpClientError::Protocol);
            let replay = replay_state
                .lock()
                .expect("replay state lock poisoned")
                .take()
                .map_or_else(Vec::new, |state| state.notifications);
            let result = result.map(|response| LoadedSession { response, replay });
            let _ = response.send(result);
        }
        ClientCommand::ResumeSession { request, response } => {
            send_typed_response(cx, request, response, state).await;
        }
        ClientCommand::CloseSession { request, response } => {
            send_typed_response(cx, request, response, state).await;
        }
        ClientCommand::SearchPrompts { params, response } => {
            send_typed_response(cx, params, response, state).await;
        }
        ClientCommand::PreviewSession { params, response } => {
            send_typed_response(cx, params, response, state).await;
        }
    }
}

async fn send_typed_response<T: JsonRpcRequest>(
    cx: &ConnectionTo<acp::Agent>,
    request: T,
    response: Response<T::Response>,
    state: ClientState,
) {
    if state == ClientState::Prompting {
        let _ = response.send(Err(AcpClientError::Busy));
        return;
    }
    let result = cx.send_request(request).block_task().await.map_err(AcpClientError::Protocol);
    let _ = response.send(result);
}

async fn handle_legacy_command(
    cx: &ConnectionTo<acp::Agent>,
    event_tx: &mpsc::UnboundedSender<AcpEvent>,
    command: PromptCommand,
    state: ClientState,
) {
    match command {
        PromptCommand::Prompt { .. } => tracing::warn!("ignoring duplicate Prompt while one is in-flight"),
        PromptCommand::Cancel { session_id } => {
            let _ = cx.send_notification(CancelNotification::new(session_id));
        }
        PromptCommand::AuthenticateMcpServer { session_id, server_name } => {
            let msg = McpRequest::Authenticate { session_id: session_id.0.to_string(), server_name };
            if let Err(error) = cx.send_notification(msg) {
                tracing::warn!("authenticate_mcp_server notification failed: {error:?}");
            }
        }
        PromptCommand::SetConfigOption { session_id, config_id, value } => {
            let req = SetSessionConfigOptionRequest::new(session_id.clone(), config_id, value.as_str());
            spawn_request_to_event(
                cx,
                event_tx,
                req,
                move |resp| {
                    let update = ConfigOptionUpdate::new(resp.config_options);
                    Ok(AcpEvent::SessionUpdate {
                        session_id,
                        update: Box::new(SessionUpdate::ConfigOptionUpdate(update)),
                    })
                },
                |error| AcpEvent::ConfigOptionUpdateFailed { error: format!("{error:?}") },
            );
        }
        PromptCommand::Authenticate { method_id } => {
            let failed_method_id = method_id.clone();
            spawn_request_to_event(
                cx,
                event_tx,
                AuthenticateRequest::new(method_id.clone()),
                move |_| Ok(AcpEvent::AuthenticateComplete { method_id }),
                move |error| AcpEvent::AuthenticateFailed { method_id: failed_method_id, error: format!("{error:?}") },
            );
        }
        PromptCommand::SearchPrompts(params) => {
            let query = params.query.clone();
            spawn_request_to_event(
                cx,
                event_tx,
                params,
                |response| Ok(AcpEvent::PromptSearchResults(response)),
                move |error| AcpEvent::PromptSearchFailed { query, error: format!("{error}") },
            );
        }
        PromptCommand::SessionPreview(params) => {
            let session_id = params.session_id.clone();
            spawn_request_to_event(
                cx,
                event_tx,
                params,
                |response| Ok(AcpEvent::SessionPreviewLoaded(response)),
                move |error| AcpEvent::SessionPreviewFailed { session_id, error: format!("{error}") },
            );
        }
        PromptCommand::ListWorkspaces(params) => {
            spawn_request_to_event(
                cx,
                event_tx,
                params,
                |response| Ok(AcpEvent::WorkspacesListed(response)),
                |error| AcpEvent::WorkspaceListFailed { error: format!("{error}") },
            );
        }
        command => handle_legacy_lifecycle_command(cx, event_tx, command, state).await,
    }
}

async fn handle_legacy_lifecycle_command(
    cx: &ConnectionTo<acp::Agent>,
    event_tx: &mpsc::UnboundedSender<AcpEvent>,
    command: PromptCommand,
    state: ClientState,
) {
    if state == ClientState::Prompting {
        tracing::warn!("ignoring session-lifecycle command while prompt is in-flight: {command:?}");
        if matches!(command, PromptCommand::MoveWorkspace(_)) {
            let _ = event_tx.send(AcpEvent::WorkspaceMoveFailed { error: "a prompt is in flight".to_string() });
        }
        return;
    }

    match command {
        PromptCommand::ListSessions => {
            request_to_event(
                cx,
                event_tx,
                ListSessionsRequest::new(),
                |response| Ok(AcpEvent::SessionsListed { sessions: response.sessions }),
                AcpEvent::PromptError,
            )
            .await;
        }
        PromptCommand::LoadSession { session_id, cwd } => {
            request_to_event(
                cx,
                event_tx,
                LoadSessionRequest::new(session_id.clone(), cwd),
                |response| {
                    Ok(AcpEvent::SessionLoaded {
                        session_id,
                        config_options: response.config_options.unwrap_or_default(),
                    })
                },
                AcpEvent::PromptError,
            )
            .await;
        }
        PromptCommand::NewSession { cwd } => {
            request_to_event(
                cx,
                event_tx,
                NewSessionRequest::new(cwd),
                |response| {
                    Ok(AcpEvent::NewSessionCreated {
                        session_id: response.session_id,
                        config_options: response.config_options.unwrap_or_default(),
                    })
                },
                AcpEvent::PromptError,
            )
            .await;
        }
        PromptCommand::MoveWorkspace(params) => {
            request_to_event(
                cx,
                event_tx,
                params,
                |response| Ok(AcpEvent::WorkspaceMoved(response)),
                |error| AcpEvent::WorkspaceMoveFailed { error: format!("{error}") },
            )
            .await;
        }
        command => unreachable!("non-lifecycle command routed to handle_legacy_lifecycle_command: {command:?}"),
    }
}

fn request_to_event<T: JsonRpcRequest>(
    cx: &ConnectionTo<acp::Agent>,
    event_tx: &mpsc::UnboundedSender<AcpEvent>,
    params: T,
    ok: impl FnOnce(T::Response) -> Result<AcpEvent, acp::Error> + Send + 'static,
    err: impl FnOnce(acp::Error) -> AcpEvent + Send + 'static,
) -> impl Future<Output = ()> + 'static {
    let sent = cx.send_request(params).map(ok);
    let event_tx = event_tx.clone();
    async move {
        let event = match sent.block_task().await {
            Ok(event) => event,
            Err(error) => err(error),
        };
        let _ = event_tx.send(event);
    }
}

fn spawn_request_to_event<T: JsonRpcRequest>(
    cx: &ConnectionTo<acp::Agent>,
    event_tx: &mpsc::UnboundedSender<AcpEvent>,
    params: T,
    ok: impl FnOnce(T::Response) -> Result<AcpEvent, acp::Error> + Send + 'static,
    err: impl FnOnce(acp::Error) -> AcpEvent + Send + 'static,
) {
    let fut = request_to_event(cx, event_tx, params, ok, err);
    if let Err(error) = cx.spawn(async move {
        fut.await;
        Ok(())
    }) {
        tracing::warn!("failed to spawn request task: {error:?}");
    }
}

fn auto_approve_option(req: &RequestPermissionRequest) -> PermissionOptionId {
    debug_assert!(!req.options.is_empty(), "ACP guarantees at least one permission option");
    req.options
        .iter()
        .find(|option| matches!(option.kind, PermissionOptionKind::AllowOnce | PermissionOptionKind::AllowAlways))
        .map_or_else(|| req.options[0].option_id.clone(), |option| option.option_id.clone())
}
