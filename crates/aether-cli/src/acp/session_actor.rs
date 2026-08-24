use acp_utils::elicitation;
use acp_utils::notifications::McpNotification;
use acp_utils::server::AcpServerError;
use aether_auth::OAuthCredentialStorage;
use aether_core::context::ext::conversation_messages_from_events;
use aether_core::events::{AgentCommand, AgentEvent, Command, ToolEvent, TurnOutcome};
use aether_core::session::{SessionControlEvent, SessionEvent, UserEvent};
use agent_client_protocol::schema::v1::{self as acp, PromptResponse, SessionId, SetSessionConfigOptionResponse};
use agent_client_protocol::{Client, ConnectionTo, Responder};
use llm::catalog::LlmModel;
use llm::parser::ModelProviderParser;
use llm::{ChatMessage, ContentBlock, ProviderConnectionOverrides, ReasoningEffort};
use mcp_utils::client::{ElicitationRequest, McpClientEvent, McpServerStatusEntry, cancel_result};
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
            on_mcp_client_event(&io.connection, &io.session_id, event);
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
    if let Some(notification) = map_agent_event_to_session_notification(acp_session_id.clone(), msg)
        && let Err(e) =
            connection.send_notification(notification).map_err(|e| AcpServerError::protocol("session/update", e))
    {
        error!("Failed to send session notification: {:?}", e);
    }
    if let Some(agent_notif) = try_into_agent_notification(msg)
        && let Err(e) = send_agent_notification(connection, &agent_notif)
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

#[allow(clippy::result_large_err)]
fn send_agent_notification(
    connection: &ConnectionTo<Client>,
    notification: &AgentExtNotification,
) -> Result<(), AcpServerError> {
    let method = notification.method().to_string();
    let untyped = notification.to_untyped().map_err(|error| AcpServerError::protocol(method.clone(), error))?;
    connection.send_notification(untyped).map_err(|error| AcpServerError::protocol(method, error))
}

fn on_mcp_client_event(connection: &ConnectionTo<Client>, session_id: &SessionId, event: McpClientEvent) {
    match event {
        McpClientEvent::Elicitation(elicitation) => spawn_elicitation_request(connection, session_id, *elicitation),
        McpClientEvent::ElicitationComplete { server_name, elicitation_id } => {
            if let Err(error) = connection
                .send_notification(elicitation::build_acp_elicitation_completion_notification(
                    session_id,
                    &server_name,
                    &elicitation_id,
                ))
                .map_err(|error| AcpServerError::protocol("elicitation/complete", error))
            {
                error!("Failed to send elicitation completion: {error:?}");
            }
        }
        McpClientEvent::ServerStatusesChanged(servers) => send_mcp_server_status(connection, servers),
        McpClientEvent::ConnectionReady(snapshot) => send_mcp_server_status(connection, snapshot.server_statuses()),
        McpClientEvent::AuthenticationFailed { server, error } => {
            error!("MCP server authentication failed for '{server}': {error}");
        }
    }
}

async fn on_elicitation_request(
    connection: &ConnectionTo<Client>,
    session_id: &SessionId,
    elicitation: ElicitationRequest,
) {
    let result = async {
        let request =
            elicitation::map_mcp_elicitation_request_to_acp(&elicitation.server_name, session_id, &elicitation.request)
                .map_err(|error| error.to_string())?;
        let response = connection.send_request(request).block_task().await.map_err(|error| format!("{error:?}"))?;
        elicitation::map_acp_elicitation_response_to_mcp(response).map_err(|error| error.to_string())
    }
    .await
    .unwrap_or_else(|error| {
        error!("ACP elicitation failed: {error}");
        cancel_result()
    });

    if elicitation.response_sender.send(result).is_err() {
        error!("Failed to send elicitation response: receiver dropped");
    }
}

fn spawn_elicitation_request(
    connection: &ConnectionTo<Client>,
    session_id: &SessionId,
    elicitation: ElicitationRequest,
) {
    let connection = connection.clone();
    let session_id = session_id.clone();
    if let Err(e) = connection.clone().spawn(async move {
        on_elicitation_request(&connection, &session_id, elicitation).await;
        Ok(())
    }) {
        error!("Failed to spawn elicitation request handler: {e:?}");
    }
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

    mod connection_tests {
        use super::*;
        use acp_utils::elicitation::source_mcp_server_name;
        use acp_utils::testing::test_connection;
        use rmcp::model::ElicitRequestParams;
        use tokio::sync::oneshot;
        use tokio::task::LocalSet;

        fn dispatch_event(connection: &ConnectionTo<Client>, event: McpClientEvent) {
            on_mcp_client_event(connection, &SessionId::new("session-1"), event);
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

                    dispatch_event(&cx, McpClientEvent::ServerStatusesChanged(servers));

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

                    dispatch_event(&cx, McpClientEvent::ServerStatusesChanged(servers));
                    dispatch_event(
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

                    dispatch_event(&cx, McpClientEvent::ServerStatusesChanged(vec![]));

                    let McpNotification::ServerStatus { servers } = peer.next_mcp_notification().await;
                    assert!(servers.is_empty());
                })
                .await;
        }

        #[tokio::test(flavor = "current_thread")]
        async fn status_event_forwards_server_status() {
            LocalSet::new()
                .run_until(async {
                    let (cx, mut peer) = test_connection().await;
                    let servers = vec![mcp_utils::client::McpServerStatusEntry::new(
                        "github",
                        mcp_utils::client::McpServerStatus::Connected { tool_count: 1 },
                    )];
                    dispatch_event(&cx, McpClientEvent::ServerStatusesChanged(servers));

                    let McpNotification::ServerStatus { servers } = peer.next_mcp_notification().await;
                    assert_eq!(servers[0].name, "github");
                })
                .await;
        }

        #[tokio::test(flavor = "current_thread")]
        async fn elicitation_completion_forwards_native_acp_notification() {
            LocalSet::new()
                .run_until(async {
                    let (cx, mut peer) = test_connection().await;

                    dispatch_event(
                        &cx,
                        McpClientEvent::ElicitationComplete {
                            server_name: "github".to_string(),
                            elicitation_id: "el-1".to_string(),
                        },
                    );

                    let completion = peer.next_elicitation_completion().await;
                    assert_eq!(&*completion.elicitation_id.0, r#"["session-1","github","el-1"]"#);
                })
                .await;
        }

        #[tokio::test(flavor = "current_thread")]
        async fn elicitation_request_forwards_response_from_peer() {
            LocalSet::new()
                .run_until(async {
                    let (cx, mut peer) = test_connection().await;
                    peer.queue_elicitation_response(
                        serde_json::from_value(serde_json::json!({
                            "action": "accept",
                            "content": { "color": "red" }
                        }))
                        .unwrap(),
                    );

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

                    on_elicitation_request(&cx, &SessionId::new("session-1"), elicitation).await;

                    let result = rx.await.expect("response forwarded");
                    assert_eq!(result.action, rmcp::model::ElicitationAction::Accept);
                    assert_eq!(result.content, Some(serde_json::json!({ "color": "red" })));

                    let received = peer.next_elicitation_request().await;
                    assert_eq!(source_mcp_server_name(received.meta.as_ref()), Some("test-server"));
                    let acp::ElicitationMode::Form(form) = received.mode else { panic!("expected form") };
                    let acp::ElicitationScope::Session(scope) = form.scope else { panic!("expected session scope") };
                    assert_eq!(&*scope.session_id.0, "session-1");
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

                    on_elicitation_request(&cx, &SessionId::new("session-1"), elicitation).await;

                    let result = rx.await.expect("response forwarded");
                    assert_eq!(result.action, rmcp::model::ElicitationAction::Cancel);
                })
                .await;
        }
    }
}
