use super::error::AcpClientError;
use super::event::AcpEvent;
use super::prompt_handle::{AcpPromptHandle, PromptCommand};
use crate::notifications::{
    AuthMethodsUpdatedParams, ContextClearedParams, ContextCompactionParams, ElicitationParams, McpNotification,
    McpRequest, SubAgentProgressParams,
};
use agent_client_protocol::schema::v1::{
    AuthMethod, AuthenticateRequest, CancelNotification, ConfigOptionUpdate, ContentBlock, InitializeRequest,
    InitializeResponse, ListSessionsRequest, LoadSessionRequest, NewSessionRequest, NewSessionResponse,
    PermissionOptionId, PermissionOptionKind, PromptCapabilities, PromptRequest, RequestPermissionOutcome,
    RequestPermissionRequest, RequestPermissionResponse, SelectedPermissionOutcome, SessionCapabilities,
    SessionConfigOption, SessionId, SessionNotification, SessionUpdate, SetSessionConfigOptionRequest, TextContent,
};
use agent_client_protocol::{self as acp, Client, ConnectTo, ConnectionTo, JsonRpcRequest};
use tokio::sync::mpsc;
use tracing::info;

type InitializeResult = Result<(InitializeResponse, NewSessionResponse), AcpClientError>;

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

/// Spawn an ACP agent and establish an ACP session.
///
/// The connection auto-approves permissions, forwards session notifications as
/// [`AcpEvent`]s, and tunnels elicitation requests through the `_aether/elicitation`
/// extension method.
pub async fn spawn_acp_session(
    agent: impl ConnectTo<Client> + 'static,
    init_request: InitializeRequest,
    new_session_request: NewSessionRequest,
) -> Result<AcpSession, AcpClientError> {
    let (event_tx, event_rx) = mpsc::unbounded_channel::<AcpEvent>();
    let (cmd_tx, cmd_rx) = mpsc::unbounded_channel::<PromptCommand>();
    let (init_tx, mut init_rx) = mpsc::unbounded_channel::<InitializeResult>();
    tokio::spawn(run_client_connection(agent, event_tx, cmd_rx, init_tx, init_request, new_session_request));

    let (init_resp, session_resp) = init_rx
        .recv()
        .await
        .ok_or_else(|| AcpClientError::AgentCrashed("ACP task died during initialization".to_string()))??;

    let agent_name = init_resp
        .agent_info
        .as_ref()
        .map_or_else(|| "agent".to_string(), |info| info.title.as_deref().unwrap_or(&info.name).to_string());

    Ok(AcpSession {
        session_id: session_resp.session_id,
        agent_name,
        prompt_capabilities: init_resp.agent_capabilities.prompt_capabilities,
        session_capabilities: init_resp.agent_capabilities.session_capabilities,
        config_options: session_resp.config_options.unwrap_or_default(),
        auth_methods: init_resp.auth_methods,
        event_rx,
        prompt_handle: AcpPromptHandle { cmd_tx },
    })
}

#[allow(clippy::too_many_lines)]
async fn run_client_connection(
    agent: impl ConnectTo<Client> + 'static,
    event_tx: mpsc::UnboundedSender<AcpEvent>,
    cmd_rx: mpsc::UnboundedReceiver<PromptCommand>,
    init_tx: mpsc::UnboundedSender<InitializeResult>,
    init_request: InitializeRequest,
    new_session_request: NewSessionRequest,
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
                async move |params: ElicitationParams, responder, _cx| {
                    if let Err(send_err) = event_tx.send(AcpEvent::ElicitationRequest { params, responder }) {
                        // Recover the responder and reply with an error so the remote caller doesn't hang.
                        if let AcpEvent::ElicitationRequest { responder, .. } = send_err.0 {
                            return responder.respond_with_error(acp::Error::internal_error());
                        }
                    }
                    Ok(())
                }
            },
            acp::on_receive_request!(),
        )
        .on_receive_notification(
            {
                let event_tx = event_tx.clone();
                async move |SessionNotification { session_id, update, .. }: SessionNotification, _cx| {
                    let _ = event_tx.send(AcpEvent::SessionUpdate { session_id, update: Box::new(update) });
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
            let init_tx = init_tx.clone();
            async move |cx: ConnectionTo<acp::Agent>| {
                run_main(cx, event_tx, cmd_rx, init_tx, init_request, new_session_request).await;
                Ok(())
            }
        })
        .await;

    if let Err(e) = connection_result {
        tracing::warn!("ACP connection exited with error: {e:?}");
        let _ = init_tx.send(Err(AcpClientError::ConnectFailed(e)));
    }
    let _ = event_tx.send(AcpEvent::ConnectionClosed);
}

#[allow(clippy::too_many_lines)]
async fn run_main(
    cx: ConnectionTo<acp::Agent>,
    event_tx: mpsc::UnboundedSender<AcpEvent>,
    mut cmd_rx: mpsc::UnboundedReceiver<PromptCommand>,
    init_tx: mpsc::UnboundedSender<InitializeResult>,
    init_request: InitializeRequest,
    new_session_request: NewSessionRequest,
) {
    let init_resp = match cx.send_request(init_request).block_task().await {
        Ok(r) => r,
        Err(e) => {
            let _ = init_tx.send(Err(AcpClientError::Protocol(e)));
            return;
        }
    };
    info!("ACP initialized: protocol={:?}, agent_info={:?}", init_resp.protocol_version, init_resp.agent_info);

    let session_resp = match cx.send_request(new_session_request).block_task().await {
        Ok(r) => r,
        Err(e) => {
            let _ = init_tx.send(Err(AcpClientError::Protocol(e)));
            return;
        }
    };
    info!("ACP session created: {}", session_resp.session_id);

    let _ = init_tx.send(Ok((init_resp, session_resp)));

    while let Some(cmd) = cmd_rx.recv().await {
        match cmd {
            PromptCommand::Prompt { session_id, text, content } => {
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
                                Ok(resp) => AcpEvent::PromptDone(resp.stop_reason),
                                Err(e) => AcpEvent::PromptError(e),
                            };
                            let _ = event_tx.send(event);
                            break;
                        }
                        Some(cmd) = cmd_rx.recv() => {
                            handle_command(&cx, &event_tx, cmd, ClientState::Prompting).await;
                        }
                    }
                }
            }
            cmd => handle_command(&cx, &event_tx, cmd, ClientState::Idle).await,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ClientState {
    Idle,
    Prompting,
}

async fn handle_command(
    cx: &ConnectionTo<acp::Agent>,
    event_tx: &mpsc::UnboundedSender<AcpEvent>,
    cmd: PromptCommand,
    state: ClientState,
) {
    match cmd {
        PromptCommand::Prompt { .. } => {
            tracing::warn!("ignoring duplicate Prompt while one is in-flight");
        }
        PromptCommand::Cancel { session_id } => {
            let _ = cx.send_notification(CancelNotification::new(session_id));
        }
        PromptCommand::AuthenticateMcpServer { session_id, server_name } => {
            let msg = McpRequest::Authenticate { session_id: session_id.0.to_string(), server_name };
            if let Err(e) = cx.send_notification(msg) {
                tracing::warn!("authenticate_mcp_server notification failed: {e:?}");
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
                |e| AcpEvent::ConfigOptionUpdateFailed { error: format!("{e:?}") },
            );
        }
        PromptCommand::Authenticate { method_id } => {
            let failed_method_id = method_id.clone();
            spawn_request_to_event(
                cx,
                event_tx,
                AuthenticateRequest::new(method_id.clone()),
                move |_| Ok(AcpEvent::AuthenticateComplete { method_id }),
                move |e| AcpEvent::AuthenticateFailed { method_id: failed_method_id, error: format!("{e:?}") },
            );
        }
        PromptCommand::SearchPrompts(params) => {
            let query = params.query.clone();
            spawn_request_to_event(
                cx,
                event_tx,
                params,
                |resp| Ok(AcpEvent::PromptSearchResults(resp)),
                move |e| AcpEvent::PromptSearchFailed { query, error: format!("{e}") },
            );
        }
        PromptCommand::SessionPreview(params) => {
            let session_id = params.session_id.clone();
            spawn_request_to_event(
                cx,
                event_tx,
                params,
                |resp| Ok(AcpEvent::SessionPreviewLoaded(resp)),
                move |e| AcpEvent::SessionPreviewFailed { session_id, error: format!("{e}") },
            );
        }
        PromptCommand::ListWorkspaces(params) => {
            spawn_request_to_event(
                cx,
                event_tx,
                params,
                |resp| Ok(AcpEvent::WorkspacesListed(resp)),
                |e| AcpEvent::WorkspaceListFailed { error: format!("{e}") },
            );
        }
        cmd => handle_lifecycle_command(cx, event_tx, cmd, state).await,
    }
}

/// Handle the session-lifecycle commands (`ListSessions`, `LoadSession`,
/// `NewSession`, `MoveWorkspace`).
async fn handle_lifecycle_command(
    cx: &ConnectionTo<acp::Agent>,
    event_tx: &mpsc::UnboundedSender<AcpEvent>,
    cmd: PromptCommand,
    state: ClientState,
) {
    if state == ClientState::Prompting {
        tracing::warn!("ignoring session-lifecycle command while prompt is in-flight: {cmd:?}");
        if matches!(cmd, PromptCommand::MoveWorkspace(_)) {
            let _ = event_tx.send(AcpEvent::WorkspaceMoveFailed { error: "a prompt is in flight".to_string() });
        }
        return;
    }

    match cmd {
        PromptCommand::ListSessions => {
            request_to_event(
                cx,
                event_tx,
                ListSessionsRequest::new(),
                |resp| Ok(AcpEvent::SessionsListed { sessions: resp.sessions }),
                AcpEvent::PromptError,
            )
            .await;
        }
        PromptCommand::LoadSession { session_id, cwd } => {
            request_to_event(
                cx,
                event_tx,
                LoadSessionRequest::new(session_id.clone(), cwd),
                |resp| {
                    Ok(AcpEvent::SessionLoaded { session_id, config_options: resp.config_options.unwrap_or_default() })
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
                |resp| {
                    Ok(AcpEvent::NewSessionCreated {
                        session_id: resp.session_id,
                        config_options: resp.config_options.unwrap_or_default(),
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
                |resp| Ok(AcpEvent::WorkspaceMoved(resp)),
                |e| AcpEvent::WorkspaceMoveFailed { error: format!("{e}") },
            )
            .await;
        }
        cmd => unreachable!("non-lifecycle command routed to handle_lifecycle_command: {cmd:?}"),
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
            Err(e) => err(e),
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
    if let Err(e) = cx.spawn(async move {
        fut.await;
        Ok(())
    }) {
        tracing::warn!("failed to spawn request task: {e:?}");
    }
}

fn auto_approve_option(req: &RequestPermissionRequest) -> PermissionOptionId {
    debug_assert!(!req.options.is_empty(), "ACP guarantees at least one permission option");
    req.options
        .iter()
        .find(|o| matches!(o.kind, PermissionOptionKind::AllowOnce | PermissionOptionKind::AllowAlways))
        .map_or_else(|| req.options[0].option_id.clone(), |o| o.option_id.clone())
}
