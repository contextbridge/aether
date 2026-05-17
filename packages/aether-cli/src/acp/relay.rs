use acp_utils::notifications::{ElicitationParams, McpNotification, McpRequest};
use acp_utils::server::AcpServerError;
use aether_auth::OAuthCredentialStorage;
use aether_core::events::{AgentMessage, UserMessage};
use aether_core::mcp::run_mcp_task::McpCommand;
use agent_client_protocol::schema::{self as acp, SessionId};
use agent_client_protocol::{Client, ConnectionTo};
use llm::parser::ModelProviderParser;
use llm::{ContentBlock, ProviderConnectionOverrides, ReasoningEffort};
use mcp_utils::client::{ElicitationRequest, McpClientEvent, cancel_result};
use rmcp::model::CreateElicitationRequestParams;
use rmcp::model::CreateElicitationResult;
use std::fmt;
use std::sync::Arc;
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use tracing::warn;
use tracing::{error, info};

use super::mappers::{
    map_agent_message_to_session_notification, map_agent_message_to_stop_reason, try_extract_plan_notification,
    try_into_agent_notification,
};
use super::session::Session;
use super::session_store::SessionStore;
use aether_core::context::ext::{SessionEvent, UserEvent};

pub(crate) enum SessionCommand {
    Prompt {
        content: Vec<ContentBlock>,
        switch_model: Option<String>,
        reasoning_effort: Option<ReasoningEffort>,
        result_tx: oneshot::Sender<Result<acp::StopReason, RelayError>>,
    },
    Cancel,
}

pub(crate) enum RelayError {
    SwitchModelFailed(String),
    SendPromptFailed(String),
    ChannelClosed,
}

impl fmt::Display for RelayError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RelayError::SwitchModelFailed(e) => write!(f, "switch model failed: {e}"),
            RelayError::SendPromptFailed(e) => write!(f, "send prompt failed: {e}"),
            RelayError::ChannelClosed => write!(f, "agent channel closed"),
        }
    }
}

enum SlashCommandError {
    CommandChannel(String),
    McpOperation(String),
    NotFound(String),
    NoTextContent,
}

impl fmt::Display for SlashCommandError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::CommandChannel(e) => write!(f, "command channel error: {e}"),
            Self::McpOperation(e) => write!(f, "MCP operation failed: {e}"),
            Self::NotFound(name) => write!(f, "slash command '/{name}' not found"),
            Self::NoTextContent => write!(f, "prompt result contains no text content"),
        }
    }
}

pub(crate) struct RelayHandle {
    pub cmd_tx: mpsc::Sender<SessionCommand>,
    pub mcp_request_tx: mpsc::Sender<McpRequest>,
    cancel: CancellationToken,
    join: JoinHandle<()>,
}

impl RelayHandle {
    /// Signal the relay loop to exit. Idempotent.
    pub fn cancel(&self) {
        self.cancel.cancel();
    }

    /// Wait for the relay task to finish. Call [`Self::cancel`] first; when
    /// draining many relays, fan out `cancel()` before awaiting any `join()`
    /// so shutdowns run concurrently.
    pub async fn join(self) {
        let _ = self.join.await;
    }

    /// Cancel and join in one step. Prefer [`Self::cancel`] + [`Self::join`] when
    /// draining many relays so the cancel signals can fan out concurrently.
    pub async fn stop(self) {
        self.cancel();
        self.join().await;
    }
}

pub(crate) fn spawn_relay(
    session: Session,
    connection: ConnectionTo<Client>,
    acp_session_id: SessionId,
    session_store: Arc<SessionStore>,
    oauth_credential_store: Arc<dyn OAuthCredentialStorage>,
) -> RelayHandle {
    let (cmd_tx, cmd_rx) = mpsc::channel(50);
    let (mcp_request_tx, mcp_request_rx) = mpsc::channel(50);
    let cancel = CancellationToken::new();
    let join = tokio::spawn(run_session_relay(
        session,
        cmd_rx,
        mcp_request_rx,
        connection,
        acp_session_id,
        session_store,
        oauth_credential_store,
        cancel.clone(),
    ));
    RelayHandle { cmd_tx, mcp_request_tx, cancel, join }
}

#[allow(clippy::too_many_arguments)]
async fn run_session_relay(
    session: Session,
    mut cmd_rx: mpsc::Receiver<SessionCommand>,
    mut mcp_request_rx: mpsc::Receiver<McpRequest>,
    connection: ConnectionTo<Client>,
    acp_session_id: SessionId,
    session_store: Arc<SessionStore>,
    oauth_credential_store: Arc<dyn OAuthCredentialStorage>,
    cancel: CancellationToken,
) {
    let Session {
        agent_tx,
        mut agent_rx,
        agent_handle: _agent_handle,
        _mcp_handle,
        mcp_tx,
        mut event_rx,
        initial_server_statuses,
        provider_connections,
    } = session;

    if let Err(e) = connection
        .send_notification(McpNotification::ServerStatus { servers: initial_server_statuses })
        .map_err(|e| AcpServerError::protocol("_aether/mcp_event", e))
    {
        error!("Failed to send initial MCP server status: {:?}", e);
    }

    loop {
        tokio::select! {
            () = cancel.cancelled() => break,
            Some(cmd) = cmd_rx.recv() => {
                match cmd {
                    SessionCommand::Prompt {
                        content,
                        switch_model,
                        reasoning_effort,
                        result_tx,
                    } => {
                        let mut ctx = PromptContext {
                            agent_tx: &agent_tx,
                            agent_rx: &mut agent_rx,
                            mcp_tx: &mcp_tx,
                            event_rx: &mut event_rx,
                            mcp_request_rx: &mut mcp_request_rx,
                            cmd_rx: &mut cmd_rx,
                            connection: &connection,
                            acp_session_id: &acp_session_id,
                            session_store: &session_store,
                            oauth_credential_store: &oauth_credential_store,
                            provider_connections: &provider_connections,
                            cancel: &cancel,
                        };
                        let result = handle_prompt(&mut ctx, content, switch_model, reasoning_effort).await;
                        let _ = result_tx.send(result);
                    }
                    SessionCommand::Cancel => {
                        info!("Cancel received while idle, ignoring");
                    }
                }
            }
            Some(msg) = mcp_request_rx.recv() => {
                match msg {
                    McpRequest::Authenticate { server_name, .. } => {
                        authenticate_mcp_server(&mcp_tx, &server_name).await;
                    }
                }
            }
            Some(event) = event_rx.recv() => {
                handle_mcp_client_event(&connection, &agent_tx, event).await;
            }
            else => break,
        }
    }
}

struct PromptContext<'a> {
    agent_tx: &'a mpsc::Sender<UserMessage>,
    agent_rx: &'a mut mpsc::Receiver<AgentMessage>,
    mcp_tx: &'a mpsc::Sender<McpCommand>,
    event_rx: &'a mut mpsc::Receiver<McpClientEvent>,
    mcp_request_rx: &'a mut mpsc::Receiver<McpRequest>,
    cmd_rx: &'a mut mpsc::Receiver<SessionCommand>,
    connection: &'a ConnectionTo<Client>,
    acp_session_id: &'a SessionId,
    session_store: &'a Arc<SessionStore>,
    oauth_credential_store: &'a Arc<dyn OAuthCredentialStorage>,
    provider_connections: &'a ProviderConnectionOverrides,
    cancel: &'a CancellationToken,
}

async fn handle_prompt(
    ctx: &mut PromptContext<'_>,
    content: Vec<ContentBlock>,
    switch_model: Option<String>,
    reasoning_effort: Option<ReasoningEffort>,
) -> Result<acp::StopReason, RelayError> {
    if let Some(model) = switch_model {
        let parser = ModelProviderParser::default()
            .with_provider_connections(ctx.provider_connections.clone())
            .with_codex_provider(Arc::clone(ctx.oauth_credential_store));
        let (provider, _) = parser.parse(&model).await.map_err(|e| RelayError::SwitchModelFailed(format!("{e}")))?;
        ctx.agent_tx
            .send(UserMessage::SwitchModel(provider))
            .await
            .map_err(|e| RelayError::SwitchModelFailed(format!("{e}")))?;
    }

    ctx.agent_tx
        .send(UserMessage::SetReasoningEffort(reasoning_effort))
        .await
        .map_err(|e| RelayError::SendPromptFailed(format!("{e}")))?;

    let content = expand_slash_command_in_content(ctx.mcp_tx, content).await;
    log_event(
        ctx.session_store,
        &ctx.acp_session_id.0,
        &SessionEvent::User(UserEvent::Message { content: content.clone() }),
    );

    ctx.agent_tx
        .send(UserMessage::with_content(content))
        .await
        .map_err(|e| RelayError::SendPromptFailed(format!("{e}")))?;

    // The agent sends Cancelled then Done on cancel. Capture stop reason from Cancelled
    // but keep draining until Done to avoid leaving stale messages in the channel.
    let mut early_stop_reason: Option<acp::StopReason> = None;
    run_turn_loop(ctx, "Agent channel closed unexpectedly", |msg| match msg {
        AgentMessage::Cancelled { .. } => {
            early_stop_reason = Some(map_agent_message_to_stop_reason(msg));
            None
        }
        AgentMessage::Done => Some(early_stop_reason.take().unwrap_or_else(|| map_agent_message_to_stop_reason(msg))),
        AgentMessage::Error { .. } => Some(map_agent_message_to_stop_reason(msg)),
        _ => None,
    })
    .await
}

async fn run_turn_loop<F>(
    ctx: &mut PromptContext<'_>,
    channel_closed_log: &'static str,
    mut on_agent_message: F,
) -> Result<acp::StopReason, RelayError>
where
    F: FnMut(&AgentMessage) -> Option<acp::StopReason>,
{
    loop {
        tokio::select! {
            () = ctx.cancel.cancelled() => {
                info!("Relay cancellation observed during active prompt; forwarding Cancel to agent");
                let _ = ctx.agent_tx.send(UserMessage::Cancel).await;
                return Ok(acp::StopReason::Cancelled);
            }
            msg = ctx.agent_rx.recv() => {
                if let Some(msg) = msg {
                    log_event(
                        ctx.session_store,
                        &ctx.acp_session_id.0,
                        &SessionEvent::Agent(msg.clone()),
                    );
                    forward_notification(ctx.connection, ctx.acp_session_id, &msg);
                    if let Some(reason) = on_agent_message(&msg) {
                        info!("Turn completed, stop reason: {:?}", reason);
                        return Ok(reason);
                    }
                } else {
                    error!("{channel_closed_log}");
                    return Err(RelayError::ChannelClosed);
                }
            }
            Some(event) = ctx.event_rx.recv() => {
                handle_mcp_client_event(ctx.connection, ctx.agent_tx, event).await;
            }
            Some(msg) = ctx.mcp_request_rx.recv() => {
                match msg {
                    McpRequest::Authenticate { server_name, .. } => {
                        authenticate_mcp_server(ctx.mcp_tx, &server_name).await;
                    }
                }
            }
            Some(cmd) = ctx.cmd_rx.recv() => {
                handle_in_flight_command(ctx.agent_tx, cmd).await;
            }
        }
    }
}

async fn handle_in_flight_command(agent_tx: &mpsc::Sender<UserMessage>, cmd: SessionCommand) {
    match cmd {
        SessionCommand::Cancel => {
            info!("Cancel received during prompt processing");
            let _ = agent_tx.send(UserMessage::Cancel).await;
        }
        SessionCommand::Prompt { result_tx, .. } => {
            // Can't process a new prompt while one is in-flight
            let _ = result_tx.send(Err(RelayError::SendPromptFailed("prompt already in progress".to_string())));
        }
    }
}

fn log_event(store: &SessionStore, session_id: &str, event: &SessionEvent) {
    if let Err(e) = store.append_event(session_id, event) {
        warn!("Failed to append session log entry: {e}");
    }
}

async fn handle_elicitation_request(connection: &ConnectionTo<Client>, elicitation: ElicitationRequest) {
    let params = build_elicitation_params(&elicitation.server_name, &elicitation.request);

    let mcp_result = match connection
        .send_request(params)
        .block_task()
        .await
        .map_err(|e| AcpServerError::protocol("_aether/elicitation", e))
    {
        Ok(response) => {
            let mut result = CreateElicitationResult::new(response.action);
            result.content = response.content;
            result
        }
        Err(e) => {
            error!("Failed to send elicitation request: {:?}", e);
            cancel_result()
        }
    };

    if elicitation.response_sender.send(mcp_result).is_err() {
        error!("Failed to send elicitation response: receiver dropped");
    }
}

fn build_elicitation_params(server_name: &str, request: &CreateElicitationRequestParams) -> ElicitationParams {
    ElicitationParams { server_name: server_name.to_string(), request: request.clone() }
}

async fn expand_slash_command_in_content(
    mcp_tx: &mpsc::Sender<McpCommand>,
    mut content: Vec<ContentBlock>,
) -> Vec<ContentBlock> {
    if let Some(ContentBlock::Text { text }) = content.first()
        && text.starts_with('/')
    {
        let expanded = expand_slash_command_if_needed(mcp_tx, text.clone()).await;
        content[0] = ContentBlock::text(expanded);
    }
    content
}

async fn expand_slash_command_if_needed(mcp_tx: &mpsc::Sender<McpCommand>, text: String) -> String {
    let Some(slash_command_text) = text.strip_prefix('/') else {
        return text;
    };

    let (command_name, args_text) = if let Some(space_idx) = slash_command_text.find(char::is_whitespace) {
        let (cmd, args) = slash_command_text.split_at(space_idx);
        (cmd, args.trim())
    } else {
        (slash_command_text, "")
    };

    match expand_slash_command(mcp_tx, command_name, args_text).await {
        Ok(expanded) => {
            info!("Expanded slash command '{}' -> {} chars", command_name, expanded.len());
            expanded
        }
        Err(e) => {
            error!("Failed to expand slash command '{}': {}", command_name, e);
            text
        }
    }
}

async fn expand_slash_command(
    mcp_tx: &mpsc::Sender<McpCommand>,
    command_name: &str,
    args_text: &str,
) -> Result<String, SlashCommandError> {
    let arguments = parse_slash_command_arguments(args_text);

    let (tx_list, rx_list) = oneshot::channel();
    mcp_tx
        .send(McpCommand::ListPrompts { tx: tx_list })
        .await
        .map_err(|e| SlashCommandError::CommandChannel(format!("failed to send ListPrompts: {e}")))?;

    let prompts = rx_list
        .await
        .map_err(|e| SlashCommandError::CommandChannel(format!("failed to receive prompts: {e}")))?
        .map_err(SlashCommandError::McpOperation)?;

    let matching_prompt = prompts
        .iter()
        .find(|p| p.name.split("__").last().unwrap_or("") == command_name)
        .ok_or_else(|| SlashCommandError::NotFound(command_name.to_string()))?;

    let namespaced_name = matching_prompt.name.clone();

    let (tx_get, rx_get) = oneshot::channel();
    mcp_tx
        .send(McpCommand::GetPrompt { name: namespaced_name.clone(), arguments, tx: tx_get })
        .await
        .map_err(|e| SlashCommandError::CommandChannel(format!("failed to send GetPrompt: {e}")))?;

    let prompt_result = rx_get
        .await
        .map_err(|e| SlashCommandError::CommandChannel(format!("failed to receive prompt: {e}")))?
        .map_err(SlashCommandError::McpOperation)?;

    prompt_result
        .messages
        .first()
        .and_then(|message| match &message.content {
            rmcp::model::PromptMessageContent::Text { text } => Some(text.clone()),
            _ => None,
        })
        .ok_or(SlashCommandError::NoTextContent)
}

/// Parse slash command arguments into a map with both positional and special variables.
///
/// Creates an argument map with:
/// - "ARGUMENTS": The full argument string
/// - "1", "2", "3", etc.: Individual positional arguments (1-based)
fn parse_slash_command_arguments(args_text: &str) -> Option<serde_json::Map<String, serde_json::Value>> {
    if args_text.is_empty() {
        None
    } else {
        let mut arg_map = serde_json::Map::new();

        arg_map.insert("ARGUMENTS".to_string(), serde_json::Value::String(args_text.to_string()));

        for (i, arg) in args_text.split_whitespace().enumerate() {
            arg_map.insert((i + 1).to_string(), serde_json::Value::String(arg.to_string()));
        }

        Some(arg_map)
    }
}

async fn authenticate_mcp_server(mcp_tx: &mpsc::Sender<McpCommand>, name: &str) {
    if let Err(e) = mcp_tx.send(McpCommand::AuthenticateServer { name: name.to_string() }).await {
        error!("MCP server authentication failed: Failed to send AuthenticateServer command: {e}");
    }
}

fn forward_notification(connection: &ConnectionTo<Client>, acp_session_id: &SessionId, msg: &AgentMessage) {
    if let Some(notification) = map_agent_message_to_session_notification(acp_session_id.clone(), msg) {
        if let Err(e) =
            connection.send_notification(notification).map_err(|e| AcpServerError::protocol("session/update", e))
        {
            error!("Failed to send session notification: {:?}", e);
        }
    } else if let Some(agent_notif) = try_into_agent_notification(msg)
        && let Err(e) = send_agent_notification(connection, agent_notif)
    {
        error!("Failed to send ext notification: {:?}", e);
    }

    if let AgentMessage::ToolResult { result_meta, .. } = msg
        && let Some(plan_notif) = try_extract_plan_notification(acp_session_id.clone(), result_meta.as_ref())
        && let Err(e) =
            connection.send_notification(plan_notif).map_err(|e| AcpServerError::protocol("session/update", e))
    {
        error!("Failed to send plan notification: {:?}", e);
    }
}

fn send_agent_notification(
    connection: &ConnectionTo<Client>,
    notification: super::mappers::AgentExtNotification,
) -> Result<(), AcpServerError> {
    use super::mappers::AgentExtNotification;
    match notification {
        AgentExtNotification::ContextUsage(p) => {
            connection.send_notification(p).map_err(|e| AcpServerError::protocol("_aether/context_usage", e))
        }
        AgentExtNotification::ContextCleared(p) => {
            connection.send_notification(p).map_err(|e| AcpServerError::protocol("_aether/context_cleared", e))
        }
        AgentExtNotification::SubAgentProgress(p) => {
            connection.send_notification(p).map_err(|e| AcpServerError::protocol("_aether/sub_agent_progress", e))
        }
    }
}

async fn handle_mcp_client_event(
    connection: &ConnectionTo<Client>,
    agent_tx: &mpsc::Sender<UserMessage>,
    event: McpClientEvent,
) {
    match event {
        McpClientEvent::Elicitation(elicitation) => {
            handle_elicitation_request(connection, elicitation).await;
        }
        McpClientEvent::UrlElicitationComplete(params) => {
            if let Err(e) = connection
                .send_notification(McpNotification::UrlElicitationComplete(params))
                .map_err(|e| AcpServerError::protocol("_aether/mcp_event", e))
            {
                error!("Failed to send URL elicitation complete notification: {:?}", e);
            }
        }
        McpClientEvent::ServerStatusesChanged(servers) => {
            if let Err(e) = connection
                .send_notification(McpNotification::ServerStatus { servers })
                .map_err(|e| AcpServerError::protocol("_aether/mcp_event", e))
            {
                error!("Failed to send updated MCP server status: {:?}", e);
            }
        }
        McpClientEvent::ToolDefinitionsChanged(tool_definitions) => {
            if let Err(e) = agent_tx.send(UserMessage::UpdateTools(tool_definitions)).await {
                error!("Failed to send updated tools to agent: {:?}", e);
            }
        }
        McpClientEvent::AuthenticationFailed { server, error } => {
            error!("MCP server authentication failed for '{server}': {error}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use acp_utils::testing::test_connection;
    use llm::ToolDefinition;
    use mcp_utils::client::{McpServerStatus, McpServerStatusEntry};
    use tokio::task::LocalSet;
    #[test]
    fn test_argument_parsing() {
        let arg_map = parse_slash_command_arguments("do a thing that has spaces").expect("Expected Some");
        let expected = serde_json::Map::from_iter([
            ("ARGUMENTS".to_string(), serde_json::Value::String("do a thing that has spaces".to_string())),
            ("1".to_string(), serde_json::Value::String("do".to_string())),
            ("2".to_string(), serde_json::Value::String("a".to_string())),
            ("3".to_string(), serde_json::Value::String("thing".to_string())),
            ("4".to_string(), serde_json::Value::String("that".to_string())),
            ("5".to_string(), serde_json::Value::String("has".to_string())),
            ("6".to_string(), serde_json::Value::String("spaces".to_string())),
        ]);
        assert_eq!(arg_map, expected);
    }

    #[test]
    fn test_empty_arguments_returns_none() {
        assert!(parse_slash_command_arguments("").is_none());
    }

    #[tokio::test]
    async fn in_flight_cancel_is_forwarded() {
        let (agent_tx, mut agent_rx) = mpsc::channel(1);
        handle_in_flight_command(&agent_tx, SessionCommand::Cancel).await;

        let msg = tokio::time::timeout(std::time::Duration::from_millis(200), agent_rx.recv())
            .await
            .expect("cancel should be forwarded")
            .expect("agent channel should stay open");
        assert!(matches!(msg, UserMessage::Cancel));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn run_turn_loop_exits_on_cancel_and_forwards_cancel_to_agent() {
        LocalSet::new()
            .run_until(async {
                let tmp = tempfile::tempdir().expect("tempdir");
                let session_store = Arc::new(SessionStore::from_path(tmp.path().to_path_buf()));
                let (cx, _peer) = test_connection().await;
                let acp_session_id = SessionId::new("test-session");
                let cancel = CancellationToken::new();

                let (agent_tx, mut outbound_user_messages) = mpsc::channel::<UserMessage>(1);
                let (_agent_from_tx, mut agent_rx) = mpsc::channel::<AgentMessage>(1);
                let (mcp_tx, _mcp_rx) = mpsc::channel(1);
                let (_event_tx, mut event_rx) = mpsc::channel(1);
                let (_mcp_req_tx, mut mcp_request_rx) = mpsc::channel(1);
                let (_cmd_tx, mut cmd_rx) = mpsc::channel(1);
                let oauth_credential_store: Arc<dyn OAuthCredentialStorage> =
                    Arc::new(aether_auth::FakeOAuthCredentialStore::new());
                let provider_connections = ProviderConnectionOverrides::default();

                let mut ctx = PromptContext {
                    agent_tx: &agent_tx,
                    agent_rx: &mut agent_rx,
                    mcp_tx: &mcp_tx,
                    event_rx: &mut event_rx,
                    mcp_request_rx: &mut mcp_request_rx,
                    cmd_rx: &mut cmd_rx,
                    connection: &cx,
                    acp_session_id: &acp_session_id,
                    session_store: &session_store,
                    oauth_credential_store: &oauth_credential_store,
                    provider_connections: &provider_connections,
                    cancel: &cancel,
                };

                cancel.cancel();
                let result = run_turn_loop(&mut ctx, "closed", |_| None).await;
                assert!(matches!(result, Ok(acp::StopReason::Cancelled)));

                let forwarded = outbound_user_messages.recv().await.expect("cancel forwarded");
                assert!(matches!(forwarded, UserMessage::Cancel));
            })
            .await;
    }

    #[tokio::test]
    async fn in_flight_prompt_is_rejected_while_turn_in_progress() {
        let (agent_tx, _agent_rx) = mpsc::channel(1);
        let (result_tx, result_rx) = oneshot::channel();

        handle_in_flight_command(
            &agent_tx,
            SessionCommand::Prompt {
                content: vec![ContentBlock::text("second prompt")],
                switch_model: None,
                reasoning_effort: None,
                result_tx,
            },
        )
        .await;

        match result_rx.await.expect("result channel should receive response") {
            Ok(reason) => panic!("expected rejection, got stop reason: {reason:?}"),
            Err(RelayError::SendPromptFailed(message)) => {
                assert_eq!(message, "prompt already in progress");
            }
            Err(other) => panic!("expected SendPromptFailed, got {other}"),
        }
    }

    #[test]
    fn test_build_elicitation_params_from_form() {
        let elicitation = CreateElicitationRequestParams::FormElicitationParams {
            meta: None,
            message: "Pick a color".to_string(),
            requested_schema: rmcp::model::ElicitationSchema::builder().required_bool("approved").build().unwrap(),
        };

        let params = build_elicitation_params("test-server", &elicitation);
        assert_eq!(params.server_name, "test-server");
        match &params.request {
            CreateElicitationRequestParams::FormElicitationParams { message, requested_schema, .. } => {
                assert_eq!(message, "Pick a color");
                assert_eq!(requested_schema.properties.len(), 1);
                assert!(requested_schema.properties.contains_key("approved"));
            }
            CreateElicitationRequestParams::UrlElicitationParams { .. } => panic!("Expected Form, got Url"),
        }
    }

    #[test]
    fn test_build_elicitation_params_from_url() {
        let elicitation = CreateElicitationRequestParams::UrlElicitationParams {
            meta: None,
            message: "Authorize GitHub".to_string(),
            url: "https://github.com/login/oauth".to_string(),
            elicitation_id: "el-123".to_string(),
        };

        let params = build_elicitation_params("github", &elicitation);
        assert_eq!(params.server_name, "github");
        match &params.request {
            CreateElicitationRequestParams::UrlElicitationParams { message, url, elicitation_id, .. } => {
                assert_eq!(message, "Authorize GitHub");
                assert_eq!(url, "https://github.com/login/oauth");
                assert_eq!(elicitation_id, "el-123");
            }
            CreateElicitationRequestParams::FormElicitationParams { .. } => panic!("Expected Url, got Form"),
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn url_elicitation_complete_is_forwarded_as_mcp_notification() {
        LocalSet::new()
            .run_until(async {
                let (cx, mut peer) = test_connection().await;
                let event = McpClientEvent::UrlElicitationComplete(mcp_utils::client::UrlElicitationCompleteParams {
                    server_name: "github".to_string(),
                    elicitation_id: "el-42".to_string(),
                });

                let (agent_tx, _agent_rx) = mpsc::channel(1);
                handle_mcp_client_event(&cx, &agent_tx, event).await;

                let received = peer.next_mcp_notification().await;
                assert!(matches!(received, McpNotification::UrlElicitationComplete(_)));
            })
            .await;
    }

    #[tokio::test(flavor = "current_thread")]
    async fn server_status_changed_with_tools_forwards_status_and_tools() {
        LocalSet::new()
            .run_until(async {
                let (cx, mut peer) = test_connection().await;
                let (agent_tx, mut agent_rx) = mpsc::channel(1);
                let tools = vec![ToolDefinition {
                    name: "github__issues".to_string(),
                    description: "List issues".to_string(),
                    parameters: "{}".to_string(),
                    server: Some("github".to_string()),
                }];
                let servers =
                    vec![McpServerStatusEntry::new("github", McpServerStatus::Connected { tool_count: tools.len() })];

                handle_mcp_client_event(&cx, &agent_tx, McpClientEvent::ServerStatusesChanged(servers.clone())).await;
                handle_mcp_client_event(&cx, &agent_tx, McpClientEvent::ToolDefinitionsChanged(tools.clone())).await;

                let received = peer.next_mcp_notification().await;
                assert!(matches!(received, McpNotification::ServerStatus { .. }));
                let Some(UserMessage::UpdateTools(received_tools)) = agent_rx.recv().await else {
                    panic!("expected tool update");
                };
                assert_eq!(received_tools, tools);
            })
            .await;
    }

    #[tokio::test(flavor = "current_thread")]
    async fn server_status_changed_without_tools_skips_agent_update() {
        LocalSet::new()
            .run_until(async {
                let (cx, mut peer) = test_connection().await;
                let (agent_tx, mut agent_rx) = mpsc::channel(1);
                let servers = vec![McpServerStatusEntry::new(
                    "github",
                    McpServerStatus::Failed { error: "authentication timed out after 3 minutes".to_string() },
                )];

                handle_mcp_client_event(&cx, &agent_tx, McpClientEvent::ServerStatusesChanged(servers)).await;
                handle_mcp_client_event(
                    &cx,
                    &agent_tx,
                    McpClientEvent::AuthenticationFailed {
                        server: "github".to_string(),
                        error: "authentication timed out after 3 minutes".to_string(),
                    },
                )
                .await;

                assert!(matches!(peer.next_mcp_notification().await, McpNotification::ServerStatus { .. }));
                drop(agent_tx);
                assert!(agent_rx.recv().await.is_none(), "no UpdateTools should be sent on failure");
            })
            .await;
    }

    #[tokio::test(flavor = "current_thread")]
    async fn elicitation_request_forwards_response_from_peer() {
        LocalSet::new()
            .run_until(async {
                let (cx, mut peer) = test_connection().await;
                peer.queue_elicitation_response(acp_utils::notifications::ElicitationResponse {
                    action: rmcp::model::ElicitationAction::Accept,
                    content: Some(serde_json::json!({ "color": "red" })),
                });

                let (tx, rx) = oneshot::channel();
                let elicitation = ElicitationRequest {
                    server_name: "test-server".to_string(),
                    request: CreateElicitationRequestParams::FormElicitationParams {
                        meta: None,
                        message: "Pick a color".to_string(),
                        requested_schema: rmcp::model::ElicitationSchema::builder()
                            .required_bool("approved")
                            .build()
                            .unwrap(),
                    },
                    response_sender: tx,
                };

                handle_elicitation_request(&cx, elicitation).await;

                let result = rx.await.expect("response forwarded");
                assert_eq!(result.action, rmcp::model::ElicitationAction::Accept);
                assert_eq!(result.content, Some(serde_json::json!({ "color": "red" })));

                let received = peer.next_elicitation_request().await;
                assert_eq!(received.server_name, "test-server");
            })
            .await;
    }

    #[tokio::test(flavor = "current_thread")]
    async fn elicitation_request_surfaces_cancel_on_transport_error() {
        LocalSet::new()
            .run_until(async {
                let (cx, _peer) = test_connection().await;
                // No response queued → peer replies with method_not_found, which surfaces
                // as an AcpServerError and triggers the cancel_result() fallback.

                let (tx, rx) = oneshot::channel();
                let elicitation = ElicitationRequest {
                    server_name: "test-server".to_string(),
                    request: CreateElicitationRequestParams::UrlElicitationParams {
                        meta: None,
                        message: "Authorize".to_string(),
                        url: "https://example.com".to_string(),
                        elicitation_id: "el-1".to_string(),
                    },
                    response_sender: tx,
                };

                handle_elicitation_request(&cx, elicitation).await;

                let result = rx.await.expect("response forwarded");
                assert_eq!(result.action, rmcp::model::ElicitationAction::Cancel);
            })
            .await;
    }
}
