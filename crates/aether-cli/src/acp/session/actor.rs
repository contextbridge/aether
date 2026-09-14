use crate::acp::protocol::notify;
use acp_utils::elicitation;
use acp_utils::notifications::McpNotification;
use aether_auth::OAuthCredentialStorage;
use aether_core::events::{AgentCommand, AgentEvent, Command, MessageEvent, TurnOutcome};
use aether_core::mcp::McpHandle;
use aether_sessions::model::{SessionControlEvent, SessionEvent, UserEvent, last_session_usage};
use aether_sessions::transcript::conversation_messages_from_events;
use agent_client_protocol::schema::v2::{self as acp, PromptResponse, SessionId, SetSessionConfigOptionResponse};
use agent_client_protocol::{Client, ConnectionTo, Error, Responder};
use futures::{FutureExt, StreamExt, future::BoxFuture, stream::FuturesUnordered};
use llm::catalog::LlmModel;
use llm::parser::ModelProviderParser;
use llm::{ChatMessage, ContentBlock, ProviderConnectionOverrides};
use mcp_utils::client::{ElicitationRequest, McpClientEvent, McpServerStatusEntry, cancel_result};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::{mpsc, oneshot};
use tokio::task::{JoinHandle, JoinSet};
use tokio_util::sync::CancellationToken;
use tracing::{error, info, warn};

use super::agent_key::AgentKey;
use super::agents::SessionAgents;
use super::config::{SessionConfigState, Switch};
use super::config_setting::ConfigSetting;
use super::error::SessionError;
use super::model::Modes;
use super::runtime::{AgentRuntime, RuntimeFactory};
use super::slash_commands::{expand_slash_command_in_content, send_available_commands};
use crate::acp::protocol::commands::map_mcp_prompt_to_available_command;
use crate::acp::protocol::content::map_user_message;
use crate::acp::protocol::events::{NotificationMode, project_agent_event};
use crate::acp::protocol::replay::replay_to_client;
use crate::acp::state::validate_prompt_support;
use crate::slash_commands::dedupe_commands_by_name;
use aether_sessions::SessionStore;

/// Capacity of the per-session command channel feeding the actor loop.
const SESSION_COMMAND_CHANNEL_CAPACITY: usize = 50;

/// A command routed to a single session's actor. The actor is the only consumer
/// of these, so per-session state never needs an additional lock.
pub(crate) enum SessionCommand {
    Prompt {
        content: Vec<ContentBlock>,
        display_content: Vec<ContentBlock>,
        responder: Responder<PromptResponse>,
    },
    Cancel,
    Attach {
        connection: ConnectionTo<Client>,
        replay: bool,
        available: Vec<LlmModel>,
        cwd: PathBuf,
        mcp_servers: Vec<acp::McpServer>,
        reply: oneshot::Sender<Result<Vec<acp::SessionConfigOption>, Error>>,
    },
    Detach {
        reply: oneshot::Sender<()>,
    },
    SetConfig {
        setting: ConfigSetting,
        available: Vec<LlmModel>,
        responder: Responder<SetSessionConfigOptionResponse>,
    },
    AuthenticateMcp {
        server_name: String,
    },
    RefreshConfigOptions {
        available: Vec<LlmModel>,
    },
}

/// Handle the [`SessionRegistry`](crate::acp::session::registry::SessionRegistry) keeps
/// for the active session: its command channel and joined shutdown.
pub(crate) struct SessionHandle {
    cmd_tx: mpsc::Sender<SessionCommand>,
    cancel: CancellationToken,
    join: JoinHandle<()>,
}

impl SessionHandle {
    /// Clone the command channel so callers can route without holding the
    /// registry lock across an await.
    pub(crate) fn command_sender(&self) -> mpsc::Sender<SessionCommand> {
        self.cmd_tx.clone()
    }

    pub(crate) async fn shutdown(&mut self) {
        self.cancel.cancel();
        let _ = (&mut self.join).await;
    }
}

impl Drop for SessionHandle {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}

pub(crate) struct SessionIo {
    connection: Option<ConnectionTo<Client>>,
    session_id: SessionId,
}

impl SessionIo {
    pub(crate) fn new(connection: ConnectionTo<Client>, session_id: SessionId) -> Self {
        Self { connection: Some(connection), session_id }
    }

    pub(crate) fn send_update(&self, update: acp::SessionUpdate) {
        self.send(acp::UpdateSessionNotification::new(self.session_id.clone(), update));
    }

    pub(crate) fn send(&self, notification: impl agent_client_protocol::JsonRpcNotification) {
        if let Some(connection) = &self.connection {
            notify(connection, notification);
        }
    }
}

pub(crate) struct SessionActorInit {
    pub session_id: SessionId,
    pub cwd: PathBuf,
    pub mcp_servers: Vec<acp::McpServer>,
    pub connection: ConnectionTo<Client>,
    pub repository: Arc<SessionStore>,
    pub oauth_credential_store: Arc<dyn OAuthCredentialStorage>,
    pub active_agent: AgentKey,
    pub specs: SessionAgents,
    pub runtime_factory: Arc<dyn RuntimeFactory>,
    pub transcript: Vec<SessionEvent>,
    pub replay: bool,
    pub modes: Modes,
    pub config: SessionConfigState,
}

/// The mutable per-session state. The actor loop is the only owner; all mutation
/// is serialized through the command channel.
pub(crate) struct SessionActor {
    io: SessionIo,
    cwd: PathBuf,
    mcp_servers: Vec<acp::McpServer>,
    repository: Arc<SessionStore>,
    oauth_credential_store: Arc<dyn OAuthCredentialStorage>,
    cancel: CancellationToken,
    active_agent: AgentKey,
    specs: SessionAgents,
    runtimes: HashMap<AgentKey, AgentRuntime>,
    runtime_factory: Arc<dyn RuntimeFactory>,
    transcript: Vec<SessionEvent>,
    config: SessionConfigState,
    modes: Modes,
    turn: TurnState,
    preparation: JoinSet<Vec<ContentBlock>>,
    command_refresh: Option<BoxFuture<'static, Result<Vec<acp::AvailableCommand>, SessionError>>>,
    authentications: FuturesUnordered<BoxFuture<'static, Result<(), SessionError>>>,
}

#[derive(Default)]
enum TurnState {
    #[default]
    Idle,
    Preparing {
        responder: Box<Responder<PromptResponse>>,
        display_content: Vec<ContentBlock>,
    },
    Running,
}

/// List a runtime's MCP prompts as ACP available commands, de-duplicated by
/// name. Used at actor startup and after agent switches.
async fn available_commands_for(mcp: McpHandle) -> Result<Vec<acp::AvailableCommand>, SessionError> {
    let prompts = mcp.list_prompts().await.map_err(SessionError::McpOperation)?;
    let prompt_commands = prompts.iter().map(map_mcp_prompt_to_available_command).collect();
    Ok(dedupe_commands_by_name(prompt_commands))
}

impl SessionActor {
    pub(crate) async fn spawn(init: SessionActorInit) -> Result<SessionHandle, SessionError> {
        let cancel = CancellationToken::new();
        let mut actor = SessionActor {
            io: SessionIo::new(init.connection, init.session_id),
            cwd: init.cwd,
            mcp_servers: init.mcp_servers,
            repository: init.repository,
            oauth_credential_store: init.oauth_credential_store,
            cancel: cancel.clone(),
            active_agent: init.active_agent,
            specs: init.specs,
            runtimes: HashMap::new(),
            runtime_factory: init.runtime_factory,
            transcript: init.transcript,
            config: init.config,
            modes: init.modes,
            turn: TurnState::Idle,
            preparation: JoinSet::new(),
            command_refresh: None,
            authentications: FuturesUnordered::new(),
        };
        actor.ensure_active_running().await?;
        if init.replay {
            replay_to_client(&actor.transcript, &actor.io);
        }
        let (cmd_tx, mut cmd_rx) = mpsc::channel(SESSION_COMMAND_CHANNEL_CAPACITY);
        let join = tokio::spawn(async move {
            if let Ok(runtime) = actor.active_runtime() {
                send_mcp_server_status(&actor.io, runtime.mcp_server_statuses());
            }
            actor.refresh_available_commands();
            let shutdown = actor.cancel.clone();
            loop {
                let runtime = actor.runtimes.get_mut(&actor.active_agent).expect("active runtime is running");
                tokio::select! {
                    biased;
                    () = shutdown.cancelled() => {
                        actor.cancel_turn().await;
                        break;
                    }
                    Some(cmd) = cmd_rx.recv() => {
                        actor.on_session_command(cmd).await;
                    }
                    Some(content) = actor.preparation.join_next(), if !actor.preparation.is_empty() => {
                        match content {
                            Ok(content) => actor.accept_prompt(content).await,
                            Err(error) => {
                                error!("Prompt preparation task failed: {error}");
                                if let TurnState::Preparing { responder, .. } = std::mem::take(&mut actor.turn) {
                                    let _ = responder.respond_with_error(Error::internal_error());
                                }
                            }
                        }
                    }
                    Some(result) = actor.authentications.next(), if !actor.authentications.is_empty() => {
                        if let Err(error) = result {
                            error!("MCP server authentication failed: {error}");
                        }
                    }
                    result = async { actor.command_refresh.as_mut().unwrap().await }, if actor.command_refresh.is_some() => {
                        actor.command_refresh = None;
                        match result {
                            Ok(commands) => send_available_commands(&actor.io, commands),
                            Err(error) => error!("Failed to refresh available commands: {error}"),
                        }
                    }
                    Some(message) = runtime.agent_rx.recv() => {
                        actor.record_agent_event(&message);
                        if matches!(actor.turn, TurnState::Running)
                            && let Some(outcome) = message.turn_outcome()
                        {
                            actor.finish_turn(turn_result(outcome)).await;
                        }
                    }
                    Some(event) = runtime.event_rx.recv() => {
                        let refresh_commands = matches!(event, McpClientEvent::ConnectionReady(_));
                        on_mcp_client_event(&actor.io, event);
                        if refresh_commands {
                            actor.refresh_available_commands();
                        }
                    }
                    else => break,
                }
            }
            for runtime in actor.runtimes.into_values() {
                runtime.shutdown().await;
            }
        });

        Ok(SessionHandle { cmd_tx, cancel, join })
    }

    fn refresh_available_commands(&mut self) {
        if let Ok(runtime) = self.active_runtime() {
            self.command_refresh = Some(available_commands_for(runtime.mcp().clone()).boxed());
        }
    }

    fn active_runtime(&self) -> Result<&AgentRuntime, SessionError> {
        self.runtimes.get(&self.active_agent).ok_or(SessionError::ActiveRuntimeNotRunning)
    }

    fn active_provider_connections(&self) -> ProviderConnectionOverrides {
        self.specs.get(&self.active_agent).map(|spec| spec.provider_connections.clone()).unwrap_or_default()
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

    async fn sync_active_conversation(&mut self) -> Result<(), SessionError> {
        let messages = conversation_messages_from_events(&self.transcript);
        self.active_runtime()?.replace_conversation(messages).await
    }

    async fn send_active_command(&mut self, command: Command) -> Result<(), SessionError> {
        self.active_runtime()?.send_agent_command(command).await
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
        let usage_seed = last_session_usage(&self.transcript).cloned();
        let runtime = tokio::select! {
            biased;
            () = self.cancel.cancelled() => return Err(SessionError::Cancelled),
            runtime = self.runtime_factory.spawn(target.clone(), spec, messages, usage_seed) => runtime?,
        };
        self.runtimes.insert(target.clone(), runtime);
        Ok(())
    }

    fn record_event(&mut self, event: SessionEvent) {
        self.transcript.push(event);
    }
}

impl SessionActor {
    async fn on_session_command(&mut self, cmd: SessionCommand) {
        match cmd {
            SessionCommand::Prompt { responder, .. } if !matches!(self.turn, TurnState::Idle) => {
                let _ = responder.respond_with_error(Error::invalid_request());
            }
            SessionCommand::Prompt { content, display_content, responder } => {
                self.start_prompt(content, display_content, responder).await;
            }
            SessionCommand::Attach { connection, cwd, mcp_servers, replay, available, reply } => {
                if self.cwd != cwd || self.mcp_servers != mcp_servers {
                    let _ = reply
                        .send(Err(Error::invalid_params().data("live session cwd and MCP servers cannot be changed")));
                    return;
                }
                self.io.connection = Some(connection);
                if replay {
                    replay_to_client(&self.transcript, &self.io);
                }
                let state = match self.turn {
                    TurnState::Running => acp::StateUpdate::Running(acp::RunningStateUpdate::new()),
                    TurnState::Idle | TurnState::Preparing { .. } => {
                        acp::StateUpdate::Idle(acp::IdleStateUpdate::new())
                    }
                };
                self.io.send_update(acp::SessionUpdate::StateUpdate(state));
                let _ = self.publish_active_mcps();
                let options = self.config.config_options(&self.modes, &available, self.oauth_credential_store.as_ref());
                let _ = reply.send(Ok(options));
            }
            SessionCommand::Detach { reply } => {
                self.io.connection = None;
                let _ = reply.send(());
            }
            SessionCommand::Cancel => self.cancel_turn().await,
            SessionCommand::SetConfig { setting, available, responder } => {
                let result = if matches!(self.turn, TurnState::Idle) {
                    self.apply_idle_config_change(&setting, &available).await
                } else {
                    self.apply_config_change(&setting, &available)
                };
                let _ = responder.respond_with_result(result);
            }
            SessionCommand::RefreshConfigOptions { available } => {
                let options = self.config.config_options(&self.modes, &available, self.oauth_credential_store.as_ref());
                self.io.send_update(acp::SessionUpdate::ConfigOptionUpdate(acp::ConfigOptionUpdate::new(options)));
            }
            SessionCommand::AuthenticateMcp { server_name } => {
                if let Ok(runtime) = self.active_runtime() {
                    let mcp = runtime.mcp().clone();
                    self.authentications.push(
                        async move { mcp.authenticate_server(&server_name).await.map_err(SessionError::McpOperation) }
                            .boxed(),
                    );
                }
            }
        }
    }

    async fn start_prompt(
        &mut self,
        content: Vec<ContentBlock>,
        display_content: Vec<ContentBlock>,
        responder: Responder<PromptResponse>,
    ) {
        if let Err(error) = validate_prompt_support(&self.config.effective_model(&self.modes), &content) {
            let _ = responder.respond_with_error(error);
            return;
        }
        match self.prepare_prompt_runtime().await {
            Ok(mcp) => {
                self.preparation.spawn(async move { expand_slash_command_in_content(&mcp, content).await });
                self.turn = TurnState::Preparing { responder: Box::new(responder), display_content };
            }
            Err(error) => {
                error!("Prompt preparation failed: {error}");
                let _ = responder.respond_with_error(Error::internal_error());
            }
        }
    }

    async fn cancel_turn(&mut self) {
        if matches!(self.turn, TurnState::Running) {
            let _ = self.send_active_command(Command::cancel()).await;
            if self.cancel.is_cancelled() {
                self.finish_turn(Ok(acp::StopReason::Cancelled)).await;
            }
        } else if let TurnState::Preparing { responder, .. } = std::mem::take(&mut self.turn) {
            self.preparation.shutdown().await;
            let _ = responder.respond(PromptResponse::new());
            self.finish_turn(Ok(acp::StopReason::Cancelled)).await;
        }
    }

    async fn accept_prompt(&mut self, content: Vec<ContentBlock>) {
        let TurnState::Preparing { responder, display_content } = std::mem::take(&mut self.turn) else { return };
        let message_id = llm::MessageId::new();
        let user = map_user_message(message_id.to_string().into(), &display_content);
        let event = SessionEvent::User(UserEvent::Message {
            message_id: message_id.clone(),
            display_content: (display_content != content).then_some(display_content),
            content: content.clone(),
        });
        if let Err(error) = self.repository.append_event(&self.io.session_id.0, &event) {
            error!("Failed to persist prompt: {error}");
            let _ = responder.respond_with_error(Error::internal_error());
            return;
        }
        self.record_event(event);
        let _ = responder.respond(PromptResponse::new());
        self.io.send_update(acp::SessionUpdate::UserMessage(user));
        self.io.send_update(acp::SessionUpdate::StateUpdate(acp::StateUpdate::Running(acp::RunningStateUpdate::new())));
        self.turn = TurnState::Running;
        if let Err(error) = self.send_active_command(Command::with_message_id(message_id, content)).await {
            self.finish_turn(Err(error)).await;
        }
    }

    async fn finish_turn(&mut self, result: Result<acp::StopReason, SessionError>) {
        self.turn = TurnState::Idle;
        let reason = match result {
            Ok(reason) => {
                info!("Turn completed, stop reason: {reason:?}");
                reason
            }
            Err(error) => {
                error!("Accepted prompt failed: {error}");
                let message = AgentEvent::Message(MessageEvent::Text {
                    message_id: llm::MessageId::new(),
                    chunk: format!("Error: {error}"),
                    is_complete: true,
                });
                self.record_agent_event(&message);
                acp::StopReason::EndTurn
            }
        };
        self.io.send_update(acp::SessionUpdate::StateUpdate(acp::StateUpdate::Idle(
            acp::IdleStateUpdate::new().stop_reason(reason),
        )));
        if !self.cancel.is_cancelled() {
            let _ = self.apply_deferred_agent_switch().await;
        }
    }

    async fn prepare_prompt_runtime(&mut self) -> Result<McpHandle, SessionError> {
        let switch = self.config.begin_prompt(&self.modes);
        self.apply_switch(switch).await?;

        self.send_active_command(Command::agent(AgentCommand::SetReasoningEffort(self.config.reasoning_effort)))
            .await?;

        Ok(self.active_runtime()?.mcp().clone())
    }

    async fn apply_deferred_agent_switch(&mut self) -> Result<(), SessionError> {
        let switch = self.config.take_agent_switch(&self.modes);
        self.apply_switch(switch).await.inspect_err(|error| error!("Failed to activate selected mode: {error}"))
    }

    async fn apply_idle_config_change(
        &mut self,
        setting: &ConfigSetting,
        available: &[LlmModel],
    ) -> Result<SetSessionConfigOptionResponse, Error> {
        self.apply_config_change(setting, available)?;
        self.apply_deferred_agent_switch().await.map_err(|_| Error::internal_error())?;
        let options = self.config.config_options(&self.modes, available, self.oauth_credential_store.as_ref());
        Ok(SetSessionConfigOptionResponse::new(options))
    }

    fn apply_config_change(
        &mut self,
        setting: &ConfigSetting,
        available: &[LlmModel],
    ) -> Result<SetSessionConfigOptionResponse, Error> {
        self.config.apply_config_change(&self.modes, available, setting)?;

        let options = self.config.config_options(&self.modes, available, self.oauth_credential_store.as_ref());
        Ok(SetSessionConfigOptionResponse::new(options))
    }

    async fn apply_switch(&mut self, switch: Switch) -> Result<(), SessionError> {
        match switch {
            Switch::Agent(agent_name) => {
                if let Some(event) = self.select_agent(&agent_name).await? {
                    self.persist_event(event);
                }
                self.publish_active_mcps()
            }
            Switch::Model(model) => {
                let parser = ModelProviderParser::default()
                    .with_provider_connections(self.active_provider_connections())
                    .with_codex_provider(Arc::clone(&self.oauth_credential_store));
                let (provider, _) = parser.parse(&model).await?;
                self.send_active_command(Command::agent(AgentCommand::SwitchModel(provider))).await
            }
            Switch::None => Ok(()),
        }
    }

    fn publish_active_mcps(&mut self) -> Result<(), SessionError> {
        send_mcp_server_status(&self.io, self.active_runtime()?.mcp_server_statuses());
        self.refresh_available_commands();
        Ok(())
    }

    fn record_agent_event(&mut self, message: &AgentEvent) {
        self.persist_event(SessionEvent::Agent(message.clone()));
        forward_notification(&self.io, message);
    }

    fn persist_event(&mut self, event: SessionEvent) {
        if !event.is_persisted() {
            return;
        }

        if let Err(e) = self.repository.append_event(&self.io.session_id.0, &event) {
            warn!("Failed to append session log entry: {e}");
        }

        self.record_event(event);
    }
}

fn turn_result(outcome: &TurnOutcome) -> Result<acp::StopReason, SessionError> {
    match outcome {
        TurnOutcome::Completed => Ok(acp::StopReason::EndTurn),
        TurnOutcome::Cancelled => Ok(acp::StopReason::Cancelled),
        TurnOutcome::Failed { error } => Err(SessionError::TurnFailed(error.clone())),
    }
}

fn send_mcp_server_status(io: &SessionIo, servers: Vec<McpServerStatusEntry>) {
    io.send(McpNotification::ServerStatus { servers });
}

fn forward_notification(io: &SessionIo, msg: &AgentEvent) {
    project_agent_event(msg, NotificationMode::Live, io);
}

fn on_mcp_client_event(io: &SessionIo, event: McpClientEvent) {
    match event {
        McpClientEvent::Elicitation(elicitation) => {
            if let Some(connection) = &io.connection {
                spawn_elicitation_request(connection, &io.session_id, *elicitation);
            } else {
                let _ = elicitation.response_sender.send(cancel_result());
            }
        }
        McpClientEvent::ElicitationComplete { server_name, elicitation_id } => {
            io.send(elicitation::build_acp_elicitation_completion_notification(
                &io.session_id,
                &server_name,
                &elicitation_id,
            ));
        }
        McpClientEvent::ServerStatusesChanged(servers) => send_mcp_server_status(io, servers),
        McpClientEvent::ConnectionReady(snapshot) => send_mcp_server_status(io, snapshot.server_statuses()),
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
    use crate::acp::session::config::Pending;
    use crate::acp::session::model::ValidatedMode;
    use llm::ReasoningEffort as RE;

    const SONNET: &str = "anthropic:claude-sonnet-4-5";
    const DEEPSEEK: &str = "deepseek:deepseek-v4-flash";

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
    ) -> (Result<(), Error>, SessionConfigState) {
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
    fn model_change_preserves_reasoning_effort_for_reasoning_model() {
        let (result, state) = apply(SONNET, Some(RE::High), None, &ConfigSetting::Model(DEEPSEEK.into()));

        assert!(result.is_ok());
        assert_eq!(state.reasoning_effort, Some(RE::High));
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
            on_mcp_client_event(&SessionIo::new(connection.clone(), SessionId::new("session-1")), event);
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
