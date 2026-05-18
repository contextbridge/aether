use super::error::AcpClientError;
use super::event::AcpEvent;
use super::prompt_handle::{AcpPromptHandle, PromptCommand};
use crate::notifications::{
    AuthMethodsUpdatedParams, ContextClearedParams, ContextUsageParams, ElicitationParams, McpNotification, McpRequest,
    SubAgentProgressParams,
};
use agent_client_protocol::schema::{
    AuthMethod, AuthenticateRequest, CancelNotification, ConfigOptionUpdate, ContentBlock, InitializeRequest,
    InitializeResponse, ListSessionsRequest, LoadSessionRequest, NewSessionRequest, NewSessionResponse,
    PermissionOptionId, PermissionOptionKind, PromptCapabilities, PromptRequest, RequestPermissionOutcome,
    RequestPermissionRequest, RequestPermissionResponse, SelectedPermissionOutcome, SessionConfigOption, SessionId,
    SessionNotification, SessionUpdate, SetSessionConfigOptionRequest, TextContent,
};
use agent_client_protocol::{self as acp, AcpAgent, Client, ConnectionTo};
use std::str::FromStr;
use tokio::sync::mpsc;
use tracing::info;

type InitializeResult = Result<(InitializeResponse, NewSessionResponse), AcpClientError>;

/// ACP session with all handles needed by the caller.
pub struct AcpSession {
    pub session_id: SessionId,
    pub agent_name: String,
    pub prompt_capabilities: PromptCapabilities,
    pub config_options: Vec<SessionConfigOption>,
    pub auth_methods: Vec<AuthMethod>,
    pub event_rx: mpsc::UnboundedReceiver<AcpEvent>,
    pub prompt_handle: AcpPromptHandle,
}

/// Spawn an agent subprocess and establish an ACP session.
///
/// The connection auto-approves permissions, forwards session notifications as
/// [`AcpEvent`]s, and tunnels elicitation requests through the `_aether/elicitation`
/// extension method.
pub async fn spawn_acp_session(
    agent_command: &str,
    init_request: InitializeRequest,
    new_session_request: NewSessionRequest,
) -> Result<AcpSession, AcpClientError> {
    let agent = AcpAgent::from_str(agent_command).map_err(AcpClientError::InvalidAgentCommand)?;
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
        config_options: session_resp.config_options.unwrap_or_default(),
        auth_methods: init_resp.auth_methods,
        event_rx,
        prompt_handle: AcpPromptHandle { cmd_tx },
    })
}

#[allow(clippy::too_many_lines)]
async fn run_client_connection(
    agent: AcpAgent,
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
                async move |p: ContextUsageParams, _cx| {
                    let _ = event_tx.send(AcpEvent::ContextUsage(p));
                    Ok(())
                }
            },
            acp::on_receive_notification!(),
        )
        .on_receive_notification(
            {
                let event_tx = event_tx.clone();
                async move |p: ContextClearedParams, _cx| {
                    let _ = event_tx.send(AcpEvent::ContextCleared(p));
                    Ok(())
                }
            },
            acp::on_receive_notification!(),
        )
        .on_receive_notification(
            {
                let event_tx = event_tx.clone();
                async move |p: SubAgentProgressParams, _cx| {
                    let _ = event_tx.send(AcpEvent::SubAgentProgress(p));
                    Ok(())
                }
            },
            acp::on_receive_notification!(),
        )
        .on_receive_notification(
            {
                let event_tx = event_tx.clone();
                async move |p: AuthMethodsUpdatedParams, _cx| {
                    let _ = event_tx.send(AcpEvent::AuthMethodsUpdated(p));
                    Ok(())
                }
            },
            acp::on_receive_notification!(),
        )
        .on_receive_notification(
            {
                let event_tx = event_tx.clone();
                async move |n: McpNotification, _cx| {
                    let _ = event_tx.send(AcpEvent::McpNotification(n));
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
                            handle_side_command(&cx, &event_tx, cmd).await;
                        }
                    }
                }
            }
            PromptCommand::ListSessions => {
                let req = ListSessionsRequest::new();
                match cx.send_request(req).block_task().await {
                    Ok(resp) => {
                        let _ = event_tx.send(AcpEvent::SessionsListed { sessions: resp.sessions });
                    }
                    Err(e) => {
                        let _ = event_tx.send(AcpEvent::PromptError(e));
                    }
                }
            }
            PromptCommand::LoadSession { session_id, cwd } => {
                let req = LoadSessionRequest::new(session_id.clone(), cwd);
                match cx.send_request(req).block_task().await {
                    Ok(resp) => {
                        let config_options = resp.config_options.unwrap_or_default();
                        let _ = event_tx.send(AcpEvent::SessionLoaded { session_id, config_options });
                    }
                    Err(e) => {
                        let _ = event_tx.send(AcpEvent::PromptError(e));
                    }
                }
            }
            PromptCommand::NewSession { cwd } => {
                let req = NewSessionRequest::new(cwd);
                match cx.send_request(req).block_task().await {
                    Ok(resp) => {
                        let config_options = resp.config_options.unwrap_or_default();
                        let _ =
                            event_tx.send(AcpEvent::NewSessionCreated { session_id: resp.session_id, config_options });
                    }
                    Err(e) => {
                        let _ = event_tx.send(AcpEvent::PromptError(e));
                    }
                }
            }
            cmd => handle_side_command(&cx, &event_tx, cmd).await,
        }
    }
}

async fn handle_side_command(
    cx: &ConnectionTo<acp::Agent>,
    event_tx: &mpsc::UnboundedSender<AcpEvent>,
    cmd: PromptCommand,
) {
    match cmd {
        PromptCommand::Cancel { session_id } => {
            let _ = cx.send_notification(CancelNotification::new(session_id));
        }
        PromptCommand::SetConfigOption { session_id, config_id, value } => {
            let req = SetSessionConfigOptionRequest::new(session_id.clone(), config_id, value);
            match cx.send_request(req).block_task().await {
                Ok(resp) => {
                    let update = ConfigOptionUpdate::new(resp.config_options);
                    let _ = event_tx.send(AcpEvent::SessionUpdate {
                        session_id,
                        update: Box::new(SessionUpdate::ConfigOptionUpdate(update)),
                    });
                }
                Err(e) => {
                    tracing::warn!("set_session_config_option failed: {e:?}");
                }
            }
        }
        PromptCommand::Prompt { .. } => {
            tracing::warn!("ignoring duplicate Prompt while one is in-flight");
        }
        PromptCommand::ListSessions => {
            tracing::warn!("ignoring ListSessions while prompt is in-flight");
        }
        PromptCommand::LoadSession { .. } => {
            tracing::warn!("ignoring LoadSession while prompt is in-flight");
        }
        PromptCommand::NewSession { .. } => {
            tracing::warn!("ignoring NewSession while prompt is in-flight");
        }
        PromptCommand::AuthenticateMcpServer { session_id, server_name } => {
            let msg = McpRequest::Authenticate { session_id: session_id.0.to_string(), server_name };
            if let Err(e) = cx.send_notification(msg) {
                tracing::warn!("authenticate_mcp_server notification failed: {e:?}");
            }
        }
        PromptCommand::Authenticate { method_id } => {
            match cx.send_request(AuthenticateRequest::new(method_id.clone())).block_task().await {
                Ok(_) => {
                    let _ = event_tx.send(AcpEvent::AuthenticateComplete { method_id });
                }
                Err(e) => {
                    tracing::warn!("authenticate failed: {e:?}");
                    let _ = event_tx.send(AcpEvent::AuthenticateFailed { method_id, error: format!("{e:?}") });
                }
            }
        }
    }
}

fn auto_approve_option(req: &RequestPermissionRequest) -> PermissionOptionId {
    debug_assert!(!req.options.is_empty(), "ACP guarantees at least one permission option");
    req.options
        .iter()
        .find(|o| matches!(o.kind, PermissionOptionKind::AllowOnce | PermissionOptionKind::AllowAlways))
        .map_or_else(|| req.options[0].option_id.clone(), |o| o.option_id.clone())
}
