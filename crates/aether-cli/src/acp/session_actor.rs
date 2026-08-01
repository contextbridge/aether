use acp_utils::notifications::{ElicitationParams, McpNotification};
use acp_utils::server::AcpServerError;
use aether_auth::OAuthCredentialStorage;
use aether_core::context::ext::conversation_messages_from_events;
use aether_core::events::{AgentCommand, AgentEvent, Command, ToolEvent, TurnOutcome};
use aether_core::session::{SessionControlEvent, SessionEvent, UserEvent};
use agent_client_protocol::schema::{self as acp, PromptResponse, SessionId, SetSessionConfigOptionResponse};
use agent_client_protocol::{Client, ConnectionTo, JsonRpcNotification, Responder};
use llm::catalog::LlmModel;
use llm::parser::ModelProviderParser;
use llm::{ChatMessage, ContentBlock, ProviderConnectionOverrides, ReasoningEffort};
use mcp_utils::client::{ElicitationRequest, McpClientEvent, McpServerStatusEntry, cancel_result};
use rmcp::model::{ElicitRequestParams, ElicitResult};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{mpsc, watch};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use tracing::{error, info, warn};

use super::agent_key::AgentKey;
use super::agent_runtime::{AgentRuntime, RUNTIME_EVENT_CHANNEL_CAPACITY, RuntimeEvent, RuntimeFactory};
use super::config_setting::ConfigSetting;
use super::error::SessionError;
use super::model_config::{Modes, get_all_models};
use super::protocol::commands::map_mcp_prompt_to_available_command;
use super::protocol::events::{
    AgentExtNotification, map_agent_event_to_session_notification, try_extract_plan_notification,
    try_into_agent_notification,
};
use super::session_agents::SessionAgents;
use super::session_config_state::{SessionConfigState, Switch};
use super::session_store::SessionStore;
use super::slash_commands::{expand_slash_command_in_content, send_available_commands};
use crate::slash_commands::dedupe_commands_by_name;

/// Capacity of the per-session command channel feeding the actor loop.
const SESSION_COMMAND_CHANNEL_CAPACITY: usize = 50;

/// A command routed to a single session's actor. The actor is the only consumer
/// of these, so per-session state never needs an additional lock.
pub(crate) enum SessionCommand {
    Prompt { content: Vec<ContentBlock>, responder: Responder<PromptResponse> },
    Cancel,
    SetConfig { setting: ConfigSetting, available: Vec<LlmModel>, responder: Responder<SetSessionConfigOptionResponse> },
    AuthenticateMcp { server_name: String },
}

/// Handle the global [`AcpState`](super::state::AcpState) keeps for each live
/// session: a command channel into the actor plus a lock-free snapshot of the
/// session's config for broadcast fanout.
pub(crate) struct SessionHandle {
    cmd_tx: mpsc::Sender<SessionCommand>,
    snapshot_rx: watch::Receiver<ConfigSnapshot>,
    cancel: CancellationToken,
    join: JoinHandle<()>,
}

impl SessionHandle {
    /// Clone the command channel so callers can route without holding the
    /// session-map lock across an await.
    pub(crate) fn command_sender(&self) -> mpsc::Sender<SessionCommand> {
        self.cmd_tx.clone()
    }

    pub(crate) fn config_snapshot(&self) -> ConfigSnapshot {
        self.snapshot_rx.borrow().clone()
    }

    /// Signal the actor loop to exit. Idempotent.
    pub(crate) fn cancel(&self) {
        self.cancel.cancel();
    }

    /// Wait for the actor task to finish. Call [`Self::cancel`] first; when
    /// draining many sessions, fan out `cancel()` before awaiting any `join()`
    /// so shutdowns run concurrently.
    pub(crate) async fn join(self) {
        let _ = self.join.await;
    }
}

/// Lock-free view of a session's mode/model selection, published whenever the
/// actor mutates its config so global broadcasts (e.g. auth updates) can rebuild
/// config options without touching the actor.
#[derive(Clone)]
pub(crate) struct ConfigSnapshot {
    pub modes: Modes,
    pub selected_mode: Option<String>,
    pub effective_model: String,
    pub reasoning_effort: Option<ReasoningEffort>,
}

impl ConfigSnapshot {
    pub(crate) fn config_options(
        &self,
        available: &[LlmModel],
        credential_store: &dyn OAuthCredentialStorage,
    ) -> Vec<acp::SessionConfigOption> {
        let all_models = get_all_models(available);
        self.modes.config_options(
            available,
            self.selected_mode.as_deref(),
            &self.effective_model,
            self.reasoning_effort,
            &all_models,
            credential_store,
        )
    }
}

pub(crate) struct SessionActorInit {
    pub session_id: SessionId,
    pub connection: ConnectionTo<Client>,
    pub repository: Arc<SessionStore>,
    pub oauth_credential_store: Arc<dyn OAuthCredentialStorage>,
    pub active_agent: AgentKey,
    pub specs: SessionAgents,
    pub runtime_factory: Arc<dyn RuntimeFactory>,
    pub transcript: Vec<SessionEvent>,
    pub modes: Modes,
    pub config: SessionConfigState,
}

/// The mutable per-session state. The actor loop is the only owner; all mutation
/// is serialized through the command channel.
pub(crate) struct SessionActor {
    active_agent: AgentKey,
    specs: SessionAgents,
    runtimes: HashMap<AgentKey, AgentRuntime>,
    runtime_factory: Arc<dyn RuntimeFactory>,
    runtime_event_tx: mpsc::Sender<RuntimeEvent>,
    transcript: Vec<SessionEvent>,
    config: SessionConfigState,
    modes: Modes,
}

/// List a runtime's MCP prompts as ACP available commands, de-duplicated by
/// name. Used at actor startup and after agent switches.
async fn available_commands_for(runtime: &AgentRuntime) -> Result<Vec<acp::AvailableCommand>, SessionError> {
    let prompts = runtime.list_prompts().await?;
    let prompt_commands = prompts.iter().map(map_mcp_prompt_to_available_command).collect();
    Ok(dedupe_commands_by_name(prompt_commands))
}

impl SessionActor {
    pub(crate) async fn spawn(init: SessionActorInit) -> Result<SessionHandle, SessionError> {
        let (runtime_event_tx, mut runtime_event_rx) = mpsc::channel(RUNTIME_EVENT_CHANNEL_CAPACITY);

        let mut actor = SessionActor {
            active_agent: init.active_agent,
            specs: init.specs,
            runtimes: HashMap::new(),
            runtime_factory: init.runtime_factory,
            runtime_event_tx,
            transcript: init.transcript,
            config: init.config,
            modes: init.modes,
        };

        actor.ensure_active_running().await?;
        let (cmd_tx, mut cmd_rx) = mpsc::channel(SESSION_COMMAND_CHANNEL_CAPACITY);
        let (snapshot_tx, snapshot_rx) = watch::channel(actor.get_config());
        let cancel = CancellationToken::new();
        let io = SessionIo {
            connection: init.connection,
            session_id: init.session_id,
            repository: init.repository,
            oauth_credential_store: init.oauth_credential_store,
            snapshot_tx,
            cancel: cancel.clone(),
        };

        let join = tokio::spawn(async move {
            if let Ok(runtime) = actor.active_runtime() {
                send_mcp_server_status(&io.connection, runtime.mcp_server_statuses());
            }
            match actor.list_available_commands().await {
                Ok(commands) => send_available_commands(&io.connection, io.session_id.clone(), commands),
                Err(error) => error!("Failed to list initial available commands: {error}"),
            }

            loop {
                tokio::select! {
                    () = io.cancel.cancelled() => break,
                    Some(cmd) = cmd_rx.recv() => {
                        on_session_command(&mut actor, &mut runtime_event_rx, &mut cmd_rx, &io, cmd).await;
                    }
                    Some(event) = runtime_event_rx.recv() => {
                        on_runtime_event(&mut actor, &io, event).await;
                    }
                    else => break,
                }
            }
        });

        Ok(SessionHandle { cmd_tx, snapshot_rx, cancel, join })
    }

    async fn list_available_commands(&self) -> Result<Vec<acp::AvailableCommand>, SessionError> {
        available_commands_for(self.active_runtime()?).await
    }

    fn active_agent(&self) -> &AgentKey {
        &self.active_agent
    }

    fn active_runtime(&self) -> Result<&AgentRuntime, SessionError> {
        self.runtimes.get(&self.active_agent).ok_or(SessionError::ActiveRuntimeNotRunning)
    }

    fn active_provider_connections(&self) -> ProviderConnectionOverrides {
        self.specs.get(&self.active_agent).map(|spec| spec.provider_connections.clone()).unwrap_or_default()
    }

    fn effective_model(&self) -> String {
        self.config.effective_model(&self.modes)
    }

    fn get_config(&self) -> ConfigSnapshot {
        ConfigSnapshot {
            modes: self.modes.clone(),
            selected_mode: self.config.selected_mode.clone(),
            effective_model: self.effective_model(),
            reasoning_effort: self.config.reasoning_effort,
        }
    }

    async fn select_agent(&mut self, agent_name: &str) -> Result<Option<SessionEvent>, SessionError> {
        let target = AgentKey::Named(agent_name.to_owned());
        if target == self.active_agent {
            self.sync_active_conversation().await?;
            return Ok(None);
        }

        let messages = conversation_messages_from_events(&self.transcript);
        self.ensure_running_with(&target, messages).await?;

        let from = self.active_agent.agent_name();
        let to = target.agent_name();
        self.active_agent = target;

        Ok(Some(SessionEvent::Control(SessionControlEvent::AgentSwitched { from, to })))
    }

    async fn sync_active_conversation(&self) -> Result<(), SessionError> {
        let messages = conversation_messages_from_events(&self.transcript);
        self.active_runtime()?.replace_conversation(messages).await
    }

    async fn send_active_command(&self, command: Command) -> Result<(), SessionError> {
        self.active_runtime()?.send_agent_command(command).await
    }

    async fn authenticate_active_mcp_server(&self, name: &str) -> Result<(), SessionError> {
        self.active_runtime()?.authenticate_mcp_server(name).await
    }

    async fn ensure_active_running(&mut self) -> Result<(), SessionError> {
        if self.runtimes.contains_key(&self.active_agent) {
            return Ok(());
        }
        let active = self.active_agent.clone();
        let messages = conversation_messages_from_events(&self.transcript);
        self.ensure_running_with(&active, messages).await
    }

    async fn ensure_running_with(&mut self, target: &AgentKey, messages: Vec<ChatMessage>) -> Result<(), SessionError> {
        if let Some(runtime) = self.runtimes.get(target) {
            return runtime.replace_conversation(messages).await;
        }

        let spec = self.specs.get(target).ok_or_else(|| SessionError::AgentNotFound(target.display_name()))?;
        let runtime = self.runtime_factory.spawn(target.clone(), spec, messages, self.runtime_event_tx.clone()).await?;
        self.runtimes.insert(target.clone(), runtime);
        Ok(())
    }

    fn record_event(&mut self, event: SessionEvent) {
        self.transcript.push(event);
    }
}

/// Shared emission/storage context for the actor loop. Separate from
/// [`SessionActor`] so the loop can borrow the receivers and the mutable state
/// independently.
struct SessionIo {
    connection: ConnectionTo<Client>,
    session_id: SessionId,
    repository: Arc<SessionStore>,
    oauth_credential_store: Arc<dyn OAuthCredentialStorage>,
    snapshot_tx: watch::Sender<ConfigSnapshot>,
    cancel: CancellationToken,
}

async fn on_session_command(
    actor: &mut SessionActor,
    runtime_event_rx: &mut mpsc::Receiver<RuntimeEvent>,
    cmd_rx: &mut mpsc::Receiver<SessionCommand>,
    io: &SessionIo,
    cmd: SessionCommand,
) {
    match cmd {
        SessionCommand::Prompt { content, responder } => {
            let result = handle_prompt(actor, runtime_event_rx, cmd_rx, io, content).await;
            let turn_ok = result.is_ok();
            respond_prompt(responder, result);
            if turn_ok {
                let _ = apply_deferred_agent_switch(actor, io).await;
            }
        }
        SessionCommand::Cancel => info!("Cancel received while idle, ignoring"),
        SessionCommand::SetConfig { setting, available, responder } => {
            let result = apply_idle_config_change(actor, io, &setting, &available).await;
            let _ = responder.respond_with_result(result);
        }
        SessionCommand::AuthenticateMcp { server_name } => {
            if let Err(error) = actor.authenticate_active_mcp_server(&server_name).await {
                error!("MCP server authentication failed: {error}");
            }
        }
    }
}

async fn handle_prompt(
    actor: &mut SessionActor,
    runtime_event_rx: &mut mpsc::Receiver<RuntimeEvent>,
    cmd_rx: &mut mpsc::Receiver<SessionCommand>,
    io: &SessionIo,
    content: Vec<ContentBlock>,
) -> Result<acp::StopReason, SessionError> {
    let switch = actor.config.begin_prompt(&actor.modes);
    publish_snapshot(actor, io);
    apply_switch(actor, io, switch).await?;

    actor.send_active_command(Command::agent(AgentCommand::SetReasoningEffort(actor.config.reasoning_effort))).await?;

    let content = expand_slash_command_in_content(actor.active_runtime()?, content).await;
    persist_event(actor, io, SessionEvent::User(UserEvent::Message { content: content.clone() }));
    actor.send_active_command(Command::with_content(content)).await?;

    loop {
        tokio::select! {
            () = io.cancel.cancelled() => {
                info!("Cancellation observed during active prompt; forwarding Cancel to agent");
                let _ = actor.send_active_command(Command::cancel()).await;
                break Ok(acp::StopReason::Cancelled);
            }
            event = runtime_event_rx.recv() => {
                let Some(event) = event else {
                    error!("Agent channel closed unexpectedly");
                    break Err(SessionError::CommandChannel("agent channel closed".to_string()));
                };
                if let Some(message) = on_runtime_event(actor, io, event).await
                    && let Some(reason) = turn_stop_reason(&message)
                {
                    info!("Turn completed, stop reason: {:?}", reason);
                    break Ok(reason);
                }
            }
            Some(cmd) = cmd_rx.recv() => {
                handle_in_flight_command(actor, io, cmd).await;
            }
        }
    }
}

async fn apply_deferred_agent_switch(actor: &mut SessionActor, io: &SessionIo) -> Result<(), SessionError> {
    let switch = actor.config.take_agent_switch(&actor.modes);
    apply_switch(actor, io, switch).await.inspect_err(|error| error!("Failed to activate selected mode: {error}"))
}

async fn apply_idle_config_change(
    actor: &mut SessionActor,
    io: &SessionIo,
    setting: &ConfigSetting,
    available: &[LlmModel],
) -> Result<SetSessionConfigOptionResponse, acp::Error> {
    apply_config_change(actor, io, setting, available)?;
    apply_deferred_agent_switch(actor, io).await.map_err(|_| acp::Error::internal_error())?;
    let options = actor.get_config().config_options(available, io.oauth_credential_store.as_ref());
    Ok(SetSessionConfigOptionResponse::new(options))
}

fn turn_stop_reason(message: &AgentEvent) -> Option<acp::StopReason> {
    message.turn_outcome().map(|outcome| match outcome {
        TurnOutcome::Cancelled => acp::StopReason::Cancelled,
        TurnOutcome::Completed | TurnOutcome::Failed { .. } => acp::StopReason::EndTurn,
    })
}

async fn handle_in_flight_command(actor: &mut SessionActor, io: &SessionIo, cmd: SessionCommand) {
    match cmd {
        SessionCommand::Cancel => {
            info!("Cancel received during prompt processing");
            let _ = actor.send_active_command(Command::cancel()).await;
        }
        SessionCommand::AuthenticateMcp { server_name } => {
            if let Err(error) = actor.authenticate_active_mcp_server(&server_name).await {
                error!("MCP server authentication failed: {error}");
            }
        }
        SessionCommand::SetConfig { setting, available, responder } => {
            let result = apply_config_change(actor, io, &setting, &available);
            let _ = responder.respond_with_result(result);
        }
        SessionCommand::Prompt { responder, .. } => {
            respond_prompt(responder, Err(SessionError::CommandChannel("prompt already in progress".to_string())));
        }
    }
}

fn apply_config_change(
    actor: &mut SessionActor,
    io: &SessionIo,
    setting: &ConfigSetting,
    available: &[LlmModel],
) -> Result<SetSessionConfigOptionResponse, acp::Error> {
    actor.config.apply_config_change(&actor.modes, available, setting)?;
    publish_snapshot(actor, io);

    let options = actor.get_config().config_options(available, io.oauth_credential_store.as_ref());
    Ok(SetSessionConfigOptionResponse::new(options))
}

async fn apply_switch(actor: &mut SessionActor, io: &SessionIo, switch: Switch) -> Result<(), SessionError> {
    match switch {
        Switch::Agent(agent_name) => {
            publish_snapshot(actor, io);
            if let Some(event) = actor.select_agent(&agent_name).await? {
                persist_event(actor, io, event);
            }
            publish_active_mcps(actor, io).await
        }
        Switch::Model(model) => {
            let parser = ModelProviderParser::default()
                .with_provider_connections(actor.active_provider_connections())
                .with_codex_provider(Arc::clone(&io.oauth_credential_store));
            let (provider, _) = parser.parse(&model).await.map_err(|e| SessionError::McpOperation(format!("{e}")))?;
            actor.send_active_command(Command::agent(AgentCommand::SwitchModel(provider))).await
        }
        Switch::None => Ok(()),
    }
}

async fn publish_active_mcps(actor: &SessionActor, io: &SessionIo) -> Result<(), SessionError> {
    send_mcp_server_status(&io.connection, actor.active_runtime()?.mcp_server_statuses());
    send_available_commands(&io.connection, io.session_id.clone(), actor.list_available_commands().await?);
    Ok(())
}

fn publish_snapshot(actor: &SessionActor, io: &SessionIo) {
    let _ = io.snapshot_tx.send(actor.get_config());
}

async fn on_runtime_event(actor: &mut SessionActor, io: &SessionIo, event: RuntimeEvent) -> Option<AgentEvent> {
    let from_active = match &event {
        RuntimeEvent::Agent { agent, .. } | RuntimeEvent::Mcp { agent, .. } => agent == actor.active_agent(),
    };
    if !from_active {
        return None;
    }

    match event {
        RuntimeEvent::Agent { message, .. } => {
            persist_event(actor, io, SessionEvent::Agent(message.clone()));
            forward_notification(&io.connection, &io.session_id, &message);
            Some(message)
        }
        RuntimeEvent::Mcp { event, .. } => {
            let refresh_commands = matches!(event, McpClientEvent::ConnectionReady(_));
            on_mcp_client_event(&io.connection, event);
            if refresh_commands {
                match actor.list_available_commands().await {
                    Ok(commands) => send_available_commands(&io.connection, io.session_id.clone(), commands),
                    Err(error) => error!("Failed to refresh available commands after MCP bootstrap: {error}"),
                }
            }
            None
        }
    }
}

fn respond_prompt(responder: Responder<PromptResponse>, result: Result<acp::StopReason, SessionError>) {
    let response = match result {
        Ok(stop_reason) => {
            info!("Prompt completed with stop reason: {:?}", stop_reason);
            Ok(PromptResponse::new(stop_reason))
        }
        Err(e) => {
            error!("Prompt failed: {e}");
            Err(acp::Error::internal_error())
        }
    };
    if let Err(e) = responder.respond_with_result(response) {
        warn!("failed to send prompt response: {e:?}");
    }
}

fn persist_event(actor: &mut SessionActor, io: &SessionIo, event: SessionEvent) {
    if !event.is_persisted() {
        return;
    }

    if let Err(e) = io.repository.append_recorded_event(&io.session_id.0, &event) {
        warn!("Failed to append session log entry: {e}");
    }

    actor.record_event(event);
}

fn send_mcp_server_status(connection: &ConnectionTo<Client>, servers: Vec<McpServerStatusEntry>) {
    if let Err(e) = connection
        .send_notification(McpNotification::ServerStatus { servers })
        .map_err(|e| AcpServerError::protocol("_aether/mcp_event", e))
    {
        error!("Failed to send updated MCP server status: {:?}", e);
    }
}

fn forward_notification(connection: &ConnectionTo<Client>, acp_session_id: &SessionId, msg: &AgentEvent) {
    if let Some(notification) = map_agent_event_to_session_notification(acp_session_id.clone(), msg) {
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

    if let AgentEvent::Tool(ToolEvent::Result { result_meta, .. }) = msg
        && let Some(plan_notif) = try_extract_plan_notification(acp_session_id.clone(), result_meta.as_ref())
        && let Err(e) =
            connection.send_notification(plan_notif).map_err(|e| AcpServerError::protocol("session/update", e))
    {
        error!("Failed to send plan notification: {:?}", e);
    }
}

fn send_agent_notification(
    connection: &ConnectionTo<Client>,
    notification: AgentExtNotification,
) -> Result<(), AcpServerError> {
    match notification {
        AgentExtNotification::ContextUsage(params) => send_extension_notification(connection, params),
        AgentExtNotification::ContextCompaction(params) => send_extension_notification(connection, params),
        AgentExtNotification::ContextCleared(params) => send_extension_notification(connection, params),
        AgentExtNotification::SubAgentProgress(params) => send_extension_notification(connection, params),
    }
}

fn send_extension_notification<N: JsonRpcNotification>(
    connection: &ConnectionTo<Client>,
    notification: N,
) -> Result<(), AcpServerError> {
    let method = notification.method().to_string();
    connection.send_notification(notification).map_err(|error| AcpServerError::protocol(method, error))
}

fn on_mcp_client_event(connection: &ConnectionTo<Client>, event: McpClientEvent) {
    match event {
        McpClientEvent::Elicitation(elicitation) => {
            spawn_elicitation_request(connection, elicitation);
        }
        McpClientEvent::UrlElicitationComplete(params) => {
            if let Err(e) = connection
                .send_notification(McpNotification::UrlElicitationComplete(params))
                .map_err(|e| AcpServerError::protocol("_aether/mcp_event", e))
            {
                error!("Failed to send URL elicitation complete notification: {:?}", e);
            }
        }
        McpClientEvent::ServerStatusesChanged(servers) => send_mcp_server_status(connection, servers),
        McpClientEvent::ConnectionReady(snapshot) => send_mcp_server_status(connection, snapshot.server_statuses),
        McpClientEvent::AuthenticationFailed { server, error } => {
            error!("MCP server authentication failed for '{server}': {error}");
        }
        McpClientEvent::ToolDefinitionsChanged(_) | McpClientEvent::ServerInstructionsUpdated { .. } => {}
    }
}

async fn on_elicitation_request(connection: &ConnectionTo<Client>, elicitation: ElicitationRequest) {
    let params = build_elicitation_params(&elicitation.server_name, &elicitation.request);

    let mcp_result = match connection
        .send_request(params)
        .block_task()
        .await
        .map_err(|e| AcpServerError::protocol("_aether/elicitation", e))
    {
        Ok(response) => {
            let mut result = ElicitResult::new(response.action);
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

fn spawn_elicitation_request(connection: &ConnectionTo<Client>, elicitation: ElicitationRequest) {
    let connection = connection.clone();
    if let Err(e) = connection.clone().spawn(async move {
        on_elicitation_request(&connection, elicitation).await;
        Ok(())
    }) {
        error!("Failed to spawn elicitation request handler: {e:?}");
    }
}

fn build_elicitation_params(server_name: &str, request: &ElicitRequestParams) -> ElicitationParams {
    ElicitationParams { server_name: server_name.to_string(), request: request.clone() }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::acp::model_config::ValidatedMode;
    use crate::acp::session_config_state::Pending;
    use ReasoningEffort as RE;

    const SONNET: &str = "anthropic:claude-sonnet-4-5";
    const DEEPSEEK: &str = "deepseek:deepseek-chat";

    fn available_models() -> Vec<LlmModel> {
        [SONNET, "anthropic:claude-opus-4-6", DEEPSEEK].into_iter().map(|s| s.parse().expect("valid model")).collect()
    }

    fn validated_modes() -> Modes {
        let m = |name: &str, model: &str, effort| ValidatedMode {
            name: name.into(),
            model: model.into(),
            reasoning_effort: effort,
        };
        Modes::new(vec![m("Planner", SONNET, Some(RE::High)), m("Coder", DEEPSEEK, None)])
    }

    fn apply(
        active: &str,
        effort: Option<RE>,
        mode: Option<&str>,
        setting: &ConfigSetting,
    ) -> (Result<(), acp::Error>, SessionConfigState) {
        let mut state = SessionConfigState::with_selection(active.into(), mode.map(Into::into), effort);
        let result = state.apply_config_change(&validated_modes(), &available_models(), setting);
        (result, state)
    }

    #[test]
    fn unsupported_reasoning_effort_is_rejected() {
        let (result, state) = apply(SONNET, Some(RE::High), None, &ConfigSetting::ReasoningEffort(Some(RE::Max)));

        assert!(result.is_err());
        assert_eq!(state.reasoning_effort, Some(RE::High));
    }

    #[test]
    fn model_change_clears_reasoning_effort_for_non_reasoning_model() {
        let (result, state) = apply(SONNET, Some(RE::High), None, &ConfigSetting::Model(DEEPSEEK.into()));

        assert!(result.is_ok());
        assert_eq!(state.reasoning_effort, None);
    }

    #[test]
    fn model_change_clamps_reasoning_effort_to_nearest_supported_level() {
        let (result, state) =
            apply("anthropic:claude-opus-4-6", Some(RE::Max), None, &ConfigSetting::Model(SONNET.into()));

        assert!(result.is_ok());
        assert_eq!(state.reasoning_effort, Some(RE::High));
    }

    #[test]
    fn new_state_has_no_pending_model_or_mode() {
        let s = SessionConfigState::with_selection(DEEPSEEK.into(), None, None);
        assert!(s.pending.is_none());
        assert!(s.reasoning_effort.is_none());
        assert!(s.selected_mode.is_none());
    }

    #[test]
    fn mode_selection_sets_pending_agent_and_reasoning() {
        let (res, s) = apply(DEEPSEEK, None, None, &ConfigSetting::Mode("Planner".into()));
        assert!(res.is_ok());
        assert_eq!(s.pending, Some(Pending::Mode("Planner".into())));
        assert_eq!(s.reasoning_effort, Some(RE::High));
        assert_eq!(s.selected_mode.as_deref(), Some("Planner"));
    }

    #[test]
    fn selecting_current_mode_does_not_set_pending_agent() {
        let (res, s) = apply(SONNET, Some(RE::High), Some("Planner"), &ConfigSetting::Mode("Planner".into()));
        assert!(res.is_ok());
        assert!(s.pending.is_none());
        assert_eq!(s.selected_mode.as_deref(), Some("Planner"));
    }

    #[test]
    fn begin_prompt_commits_pending_mode_as_agent_switch() {
        let mut s = SessionConfigState::with_selection(DEEPSEEK.into(), None, None);
        s.apply_config_change(&validated_modes(), &available_models(), &ConfigSetting::Mode("Planner".into()))
            .expect("mode switch should apply");

        let switch = s.begin_prompt(&validated_modes());

        assert!(matches!(switch, Switch::Agent(ref name) if name == "Planner"));
        assert_eq!(s.active_model, SONNET);
        assert!(s.pending.is_none());
    }

    #[test]
    fn take_agent_switch_commits_pending_mode() {
        let mut s = SessionConfigState::with_selection(SONNET.into(), None, None);
        s.apply_config_change(&validated_modes(), &available_models(), &ConfigSetting::Mode("Coder".into()))
            .expect("mode switch should apply");

        let switch = s.take_agent_switch(&validated_modes());

        assert!(matches!(switch, Switch::Agent(ref name) if name == "Coder"));
        assert_eq!(s.selected_mode.as_deref(), Some("Coder"));
        assert!(s.pending.is_none());
        assert_eq!(s.active_model, DEEPSEEK);
    }

    #[test]
    fn begin_prompt_returns_model_switch_for_explicit_model_override() {
        let mut s = SessionConfigState::with_selection(SONNET.into(), None, None);
        s.selected_mode = Some("Planner".into());
        s.reasoning_effort = Some(RE::Medium);
        s.apply_config_change(&validated_modes(), &available_models(), &ConfigSetting::Model(DEEPSEEK.into()))
            .expect("model switch should apply");

        let switch = s.begin_prompt(&validated_modes());

        assert!(matches!(switch, Switch::Model(ref model) if model == DEEPSEEK));
        assert_eq!(s.active_model, DEEPSEEK);
        assert!(s.pending.is_none());
        assert_eq!(s.selected_mode.as_deref(), Some("Planner"));
        assert_eq!(s.effective_model(&validated_modes()), DEEPSEEK);
    }

    #[test]
    fn model_change_preserves_selected_mode() {
        let modes = Modes::new(vec![
            ValidatedMode { name: "Planner".into(), model: SONNET.into(), reasoning_effort: None },
            ValidatedMode { name: "Coder".into(), model: DEEPSEEK.into(), reasoning_effort: None },
        ]);
        let mut s = SessionConfigState::with_selection(DEEPSEEK.into(), Some("Coder".into()), None);

        s.apply_config_change(&modes, &available_models(), &ConfigSetting::Model(SONNET.into()))
            .expect("model switch should apply");

        assert_eq!(s.pending, Some(Pending::Model(SONNET.into())));
        assert_eq!(s.selected_mode.as_deref(), Some("Coder"));
        assert_eq!(s.effective_model(&modes), SONNET);
    }

    #[test]
    fn unknown_mode_is_rejected() {
        let (res, _) = apply(DEEPSEEK, None, None, &ConfigSetting::Mode("Unknown".into()));
        assert!(res.is_err());
    }

    #[test]
    fn effort_and_model_changes_preserve_mode_selection() {
        let (res, s) = apply(SONNET, Some(RE::High), Some("Planner"), &ConfigSetting::ReasoningEffort(Some(RE::Low)));
        assert!(res.is_ok());
        assert_eq!(s.reasoning_effort, Some(RE::Low));
        assert_eq!(s.selected_mode.as_deref(), Some("Planner"));

        let (res, s) = apply(SONNET, Some(RE::Medium), Some("Planner"), &ConfigSetting::Model(DEEPSEEK.into()));
        assert!(res.is_ok());
        assert_eq!(s.pending, Some(Pending::Model(DEEPSEEK.into())));
        assert_eq!(s.selected_mode.as_deref(), Some("Planner"));
        assert_eq!(s.effective_model(&validated_modes()), DEEPSEEK);
    }

    #[test]
    fn test_build_elicitation_params_from_form() {
        let elicitation = ElicitRequestParams::FormElicitationParams {
            meta: None,
            message: "Pick a color".to_string(),
            requested_schema: rmcp::model::ElicitationSchema::builder().required_bool("approved").build().unwrap(),
        };

        let params = build_elicitation_params("test-server", &elicitation);
        assert_eq!(params.server_name, "test-server");
        match &params.request {
            ElicitRequestParams::FormElicitationParams { message, requested_schema, .. } => {
                assert_eq!(message, "Pick a color");
                assert_eq!(requested_schema.properties.len(), 1);
                assert!(requested_schema.properties.contains_key("approved"));
            }
            ElicitRequestParams::UrlElicitationParams { .. } => panic!("Expected Form, got Url"),
            _ => panic!("Expected Form elicitation"),
        }
    }

    #[test]
    fn test_build_elicitation_params_from_url() {
        let elicitation = ElicitRequestParams::UrlElicitationParams {
            meta: None,
            message: "Authorize GitHub".to_string(),
            url: "https://github.com/login/oauth".to_string(),
            elicitation_id: "el-123".to_string(),
        };

        let params = build_elicitation_params("github", &elicitation);
        assert_eq!(params.server_name, "github");
        match &params.request {
            ElicitRequestParams::UrlElicitationParams { message, url, elicitation_id, .. } => {
                assert_eq!(message, "Authorize GitHub");
                assert_eq!(url, "https://github.com/login/oauth");
                assert_eq!(elicitation_id, "el-123");
            }
            ElicitRequestParams::FormElicitationParams { .. } => panic!("Expected Url, got Form"),
            _ => panic!("Expected URL elicitation"),
        }
    }

    mod connection_tests {
        use super::*;
        use acp_utils::testing::test_connection;
        use tokio::sync::oneshot;
        use tokio::task::LocalSet;

        #[tokio::test(flavor = "current_thread")]
        async fn url_elicitation_complete_is_forwarded_as_mcp_notification() {
            LocalSet::new()
                .run_until(async {
                    let (cx, mut peer) = test_connection().await;
                    let event =
                        McpClientEvent::UrlElicitationComplete(mcp_utils::client::UrlElicitationCompleteParams {
                            server_name: "github".to_string(),
                            elicitation_id: "el-42".to_string(),
                        });

                    on_mcp_client_event(&cx, event);

                    let received = peer.next_mcp_notification().await;
                    assert!(matches!(received, McpNotification::UrlElicitationComplete(_)));
                })
                .await;
        }

        #[tokio::test(flavor = "current_thread")]
        async fn server_status_change_forwards_status_notification() {
            LocalSet::new()
                .run_until(async {
                    let (cx, mut peer) = test_connection().await;
                    let servers = vec![mcp_utils::client::McpServerStatusEntry::new(
                        "github",
                        mcp_utils::client::McpServerStatus::Connected { tool_count: 1 },
                    )];

                    on_mcp_client_event(&cx, McpClientEvent::ServerStatusesChanged(servers));

                    let received = peer.next_mcp_notification().await;
                    assert!(matches!(received, McpNotification::ServerStatus { .. }));
                })
                .await;
        }

        #[tokio::test(flavor = "current_thread")]
        async fn auth_failure_after_status_change_still_forwards_status() {
            LocalSet::new()
                .run_until(async {
                    let (cx, mut peer) = test_connection().await;
                    let servers = vec![mcp_utils::client::McpServerStatusEntry::new(
                        "github",
                        mcp_utils::client::McpServerStatus::Failed {
                            error: "authentication timed out after 3 minutes".to_string(),
                        },
                    )];

                    on_mcp_client_event(&cx, McpClientEvent::ServerStatusesChanged(servers));
                    on_mcp_client_event(
                        &cx,
                        McpClientEvent::AuthenticationFailed {
                            server: "github".to_string(),
                            error: "authentication timed out after 3 minutes".to_string(),
                        },
                    );

                    assert!(matches!(peer.next_mcp_notification().await, McpNotification::ServerStatus { .. }));
                })
                .await;
        }

        #[tokio::test(flavor = "current_thread")]
        async fn empty_server_status_change_forwards_clear_notification() {
            LocalSet::new()
                .run_until(async {
                    let (cx, mut peer) = test_connection().await;

                    on_mcp_client_event(&cx, McpClientEvent::ServerStatusesChanged(vec![]));

                    match peer.next_mcp_notification().await {
                        McpNotification::ServerStatus { servers } => assert!(servers.is_empty()),
                        other @ McpNotification::UrlElicitationComplete(_) => {
                            panic!("expected empty server status notification, got {other:?}")
                        }
                    }
                })
                .await;
        }

        #[tokio::test(flavor = "current_thread")]
        async fn connection_ready_forwards_server_status() {
            LocalSet::new()
                .run_until(async {
                    let (cx, mut peer) = test_connection().await;
                    let servers = vec![mcp_utils::client::McpServerStatusEntry::new(
                        "github",
                        mcp_utils::client::McpServerStatus::Connected { tool_count: 1 },
                    )];
                    let snapshot = mcp_utils::client::McpConnectionDetails {
                        instructions: std::collections::BTreeMap::new(),
                        tool_definitions: Vec::new(),
                        server_statuses: servers,
                    };

                    on_mcp_client_event(&cx, McpClientEvent::ConnectionReady(snapshot));

                    match peer.next_mcp_notification().await {
                        McpNotification::ServerStatus { servers } => assert_eq!(servers[0].name, "github"),
                        other @ McpNotification::UrlElicitationComplete(_) => {
                            panic!("expected server status notification, got {other:?}")
                        }
                    }
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
                        request: ElicitRequestParams::FormElicitationParams {
                            meta: None,
                            message: "Pick a color".to_string(),
                            requested_schema: rmcp::model::ElicitationSchema::builder()
                                .required_bool("approved")
                                .build()
                                .unwrap(),
                        },
                        response_sender: tx,
                    };

                    on_elicitation_request(&cx, elicitation).await;

                    let result = rx.await.expect("response forwarded");
                    assert_eq!(result.action, rmcp::model::ElicitationAction::Accept);
                    assert_eq!(result.content, Some(serde_json::json!({ "color": "red" })));

                    let received = peer.next_elicitation_request().await;
                    assert_eq!(received.server_name, "test-server");
                })
                .await;
        }

        #[tokio::test(flavor = "current_thread")]
        async fn url_elicitation_request_does_not_block_completion_notifications() {
            LocalSet::new()
                .run_until(async {
                    let (cx, mut peer) = test_connection().await;
                    let responder_rx = peer.capture_next_elicitation();
                    let (tx, rx) = oneshot::channel();
                    let elicitation = ElicitationRequest {
                        server_name: "github".to_string(),
                        request: ElicitRequestParams::UrlElicitationParams {
                            meta: None,
                            message: "Authorize".to_string(),
                            url: "https://example.com/oauth".to_string(),
                            elicitation_id: "el-1".to_string(),
                        },
                        response_sender: tx,
                    };

                    on_mcp_client_event(&cx, McpClientEvent::Elicitation(elicitation));
                    let responder = responder_rx.await.expect("URL elicitation request should reach peer");

                    on_mcp_client_event(
                        &cx,
                        McpClientEvent::UrlElicitationComplete(mcp_utils::client::UrlElicitationCompleteParams {
                            server_name: "github".to_string(),
                            elicitation_id: "el-1".to_string(),
                        }),
                    );

                    match peer.next_mcp_notification().await {
                        McpNotification::UrlElicitationComplete(params) => {
                            assert_eq!(params.server_name, "github");
                            assert_eq!(params.elicitation_id, "el-1");
                        }
                        other @ McpNotification::ServerStatus { .. } => {
                            panic!("expected URL completion notification, got {other:?}")
                        }
                    }

                    let _ = responder.respond(acp_utils::notifications::ElicitationResponse {
                        action: rmcp::model::ElicitationAction::Accept,
                        content: None,
                    });
                    let result = rx.await.expect("spawned URL request should forward response");
                    assert_eq!(result.action, rmcp::model::ElicitationAction::Accept);
                })
                .await;
        }

        #[tokio::test(flavor = "current_thread")]
        async fn form_elicitation_request_does_not_block_the_caller() {
            LocalSet::new()
                .run_until(async {
                    let (cx, peer) = test_connection().await;
                    let responder_rx = peer.capture_next_elicitation();
                    let (tx, rx) = oneshot::channel();
                    let elicitation = ElicitationRequest {
                        server_name: "test-server".to_string(),
                        request: ElicitRequestParams::FormElicitationParams {
                            meta: None,
                            message: "Pick a color".to_string(),
                            requested_schema: rmcp::model::ElicitationSchema::builder()
                                .required_bool("approved")
                                .build()
                                .unwrap(),
                        },
                        response_sender: tx,
                    };

                    on_mcp_client_event(&cx, McpClientEvent::Elicitation(elicitation));

                    let responder = responder_rx.await.expect("form elicitation request should reach peer");
                    let _ = responder.respond(acp_utils::notifications::ElicitationResponse {
                        action: rmcp::model::ElicitationAction::Accept,
                        content: Some(serde_json::json!({ "approved": true })),
                    });

                    let result = rx.await.expect("spawned form request should forward response");
                    assert_eq!(result.action, rmcp::model::ElicitationAction::Accept);
                })
                .await;
        }

        #[tokio::test(flavor = "current_thread")]
        async fn elicitation_request_surfaces_cancel_on_transport_error() {
            LocalSet::new()
                .run_until(async {
                    let (cx, _peer) = test_connection().await;
                    let (tx, rx) = oneshot::channel();
                    let elicitation = ElicitationRequest {
                        server_name: "test-server".to_string(),
                        request: ElicitRequestParams::UrlElicitationParams {
                            meta: None,
                            message: "Authorize".to_string(),
                            url: "https://example.com".to_string(),
                            elicitation_id: "el-1".to_string(),
                        },
                        response_sender: tx,
                    };

                    on_elicitation_request(&cx, elicitation).await;

                    let result = rx.await.expect("response forwarded");
                    assert_eq!(result.action, rmcp::model::ElicitationAction::Cancel);
                })
                .await;
        }
    }
}
