//! The per-session client state machine every frontend drives.
//!
//! A [`SessionCore`] owns everything about one agent session that is not
//! presentation: the session model, the conversation, the prompts in flight,
//! and the operation the client is in the middle of. Hosts feed it agent
//! events, command results, and user intents, and react to the [`Effect`]s it
//! returns.

mod acp_reducer;
mod effect;
mod foreground;
mod prompts;
mod session;

pub use effect::{Effect, Elicitation};
pub use foreground::ForegroundOperation;
pub use prompts::QueuedPrompt;

use crate::command::{AgentCommand, Command, CommandResult, GitReviewCommand};
use crate::command_entry::{CommandEntry, builtin_commands};
use crate::plan_tracker::PlanTracker;
use crate::session_model::SessionModel;
use crate::workspace_status::{WorkspaceAccess, WorkspaceStatus};
use acp_utils::client::AcpEvent;
use acp_utils::conversation::Conversation;
use acp_utils::notifications::AetherCapabilities;
use agent_client_protocol::schema::v2 as acp;
use std::path::PathBuf;
use std::time::Instant;

/// One agent session's protocol state and the transitions over it.
pub struct SessionCore {
    session: SessionModel,
    conversation: Conversation,
    plan_tracker: PlanTracker,
    available_commands: Vec<CommandEntry>,
    foreground: ForegroundOperation,
    queued_prompts: Vec<QueuedPrompt>,
    /// How the work since the session was last settled ended, if any turn has.
    ended_turn: Option<TurnEnd>,
    client_name: String,
    /// What the current transition owes its host, drained when it returns.
    effects: Vec<Effect>,
}

/// Everything a [`SessionCore`] starts from: a freshly connected session.
pub struct SessionCoreConfig {
    pub initialize_response: acp::InitializeResponse,
    pub session_response: acp::NewSessionResponse,
    pub working_dir: PathBuf,
    pub workspace_status: WorkspaceStatus,
    pub workspace_access: WorkspaceAccess,
    /// Labels the notices the core appends, as in `[wisp] Failed to send prompt`.
    pub client_name: String,
}

impl SessionCore {
    pub fn new(config: SessionCoreConfig) -> Self {
        let capabilities = AetherCapabilities::from_meta(
            config.initialize_response.capabilities.session.as_ref().and_then(|session| session.meta.as_ref()),
        );
        let available_commands = builtin_commands(&capabilities);
        let client_name = config.client_name.clone();
        Self {
            session: SessionModel::from_config(config, capabilities),
            conversation: Conversation::new(),
            plan_tracker: PlanTracker::default(),
            available_commands,
            foreground: ForegroundOperation::Idle,
            queued_prompts: Vec::new(),
            ended_turn: None,
            client_name,
            effects: Vec::new(),
        }
    }

    pub fn session(&self) -> &SessionModel {
        &self.session
    }

    pub fn conversation(&self) -> &Conversation {
        &self.conversation
    }

    pub fn plan_tracker(&self) -> &PlanTracker {
        &self.plan_tracker
    }

    /// The builtin commands followed by the ones the agent advertised.
    pub fn available_commands(&self) -> &[CommandEntry] {
        &self.available_commands
    }

    pub fn foreground(&self) -> &ForegroundOperation {
        &self.foreground
    }

    /// Prompts sent to the agent that it has not answered yet, oldest first.
    pub fn queued_prompts(&self) -> &[QueuedPrompt] {
        &self.queued_prompts
    }

    pub fn waiting_for_response(&self) -> bool {
        !self.conversation.turn().is_idle() || !self.queued_prompts.is_empty()
    }

    /// Either the prompt or one of its tool calls is still running.
    pub fn is_agent_busy(&self) -> bool {
        self.waiting_for_response() || self.conversation.any_running()
    }

    /// Whether a session transition may start: nothing in the foreground and
    /// no prompt in flight.
    pub fn can_start_foreground_operation(&self) -> bool {
        self.foreground.is_idle() && !self.waiting_for_response()
    }

    /// Reduces one agent event.
    pub fn on_acp_event(&mut self, event: AcpEvent) -> Vec<Effect> {
        self.reduce_acp_event(event);
        self.take_effects()
    }

    /// Reduces the outcome of a command the host dispatched. Host results
    /// (`CommandResult::Host`) are the host's own to reduce and are ignored.
    pub fn on_command_result<T>(&mut self, result: CommandResult<T>) -> Vec<Effect> {
        match result {
            CommandResult::Prompt { request_id, result } => self.finish_prompt(request_id, result),
            CommandResult::Cancel(result) | CommandResult::AuthenticateMcp(result) => {
                if let Err(error) = result {
                    self.raise_notice(&error);
                }
            }
            CommandResult::NewSession(result) => match result {
                Ok(response) => self.on_new_session(response.session_id, response.config_options),
                Err(error) => {
                    self.foreground = ForegroundOperation::Idle;
                    self.raise_notice(&format!("Failed to create new session: {error}"));
                }
            },
            CommandResult::ResumeSession { session_id, result } => self.on_resume_result(&session_id, result),
            CommandResult::ConfigOptionsUpdated { conversation_id, result }
                if conversation_id == self.conversation.id() =>
            {
                match result {
                    Ok(response) => {
                        self.session.update_config_options(response.config_options);
                        self.effects.push(Effect::ConfigOptionsChanged);
                    }
                    Err(error) => {
                        tracing::warn!("set_session_config_option failed: {error}");
                        self.raise_notice(&format!("Failed to update setting: {error}"));
                    }
                }
            }
            CommandResult::ConfigOptionsUpdated { .. } | CommandResult::Host(_) => {}
            CommandResult::AuthenticationCompleted { method_id, result } => {
                if let Err(error) = &result {
                    tracing::warn!("Provider authentication failed for {method_id}: {error}");
                }
                self.effects.push(Effect::AuthenticationFinished { method_id, result: result.map(drop) });
            }
            CommandResult::FilesIndexed { request_id, files } => {
                self.effects.push(Effect::FilesIndexed { request_id, files });
            }
            CommandResult::GitReview(state) => self.effects.push(Effect::GitReviewState(state)),
            CommandResult::GitReviewAction(result) => self.effects.push(Effect::GitReviewActionFinished(result)),
            CommandResult::SubmissionPrepared(outcome) => self.finish_submission(outcome),
            CommandResult::WorkspaceResolved { cwd, status } => {
                if self.session.working_dir() == cwd {
                    self.session.set_workspace_status(status);
                }
            }
            CommandResult::SessionsListed(Ok(response)) => {
                let current_id = self.session.session_id();
                let others = response.sessions.into_iter().filter(|session| session.session_id != *current_id);
                self.effects.push(Effect::SessionsListed(others.collect()));
            }
            CommandResult::SessionsListed(Err(error)) => {
                self.raise_notice(&format!("Failed to list sessions: {error}"));
            }
            CommandResult::PromptSearchResults { query, result } => {
                self.effects.push(Effect::PromptSearchResults { query, result });
            }
            CommandResult::SessionPreviewLoaded { session_id, result } => {
                self.effects.push(Effect::SessionPreviewLoaded { session_id, result });
            }
            CommandResult::WorkspacesListed(Ok(response)) => {
                self.effects.push(Effect::WorkspacesListed(response.workspaces));
                self.foreground = ForegroundOperation::PickingWorkspace;
            }
            CommandResult::WorkspacesListed(Err(error)) => {
                self.abandon_workspace_move(&format!("Failed to list workspaces: {error}"));
            }
            CommandResult::WorkspaceMoved(Ok(response)) => self.on_workspace_moved(response.new_cwd),
            CommandResult::WorkspaceMoved(Err(error)) => {
                self.abandon_workspace_move(&format!("Workspace move failed: {error}"));
            }
            CommandResult::BackgroundFailed(error) => self.raise_notice(&error),
        }
        self.take_effects()
    }

    /// Advances the clock completed plan entries expire against.
    pub fn on_tick(&mut self, now: Instant) {
        self.plan_tracker.on_tick(now);
        self.plan_tracker.sync(self.conversation.plan(), Instant::now());
    }

    /// Asks the agent to cancel the running turn, if there is one.
    pub fn cancel(&mut self) -> Vec<Effect> {
        if self.waiting_for_response() {
            self.dispatch(AgentCommand::Cancel { session_id: self.session.session_id().clone() });
        }
        self.take_effects()
    }

    /// Selects `value` for `config_id`: the local state changes at once and the
    /// agent is asked to apply it, confirming or correcting it in its reply.
    pub fn set_config_option(&mut self, config_id: &str, value: &str) -> Vec<Effect> {
        self.session.update_config_option_value(config_id, value);
        self.request_config_option(config_id, value);
        self.take_effects()
    }

    pub fn authenticate(&mut self, method_id: String) -> Vec<Effect> {
        self.dispatch(AgentCommand::Authenticate { method_id });
        self.take_effects()
    }

    pub fn authenticate_mcp_server(&mut self, server_name: String) -> Vec<Effect> {
        self.dispatch(AgentCommand::AuthenticateMcpServer {
            session_id: self.session.session_id().clone(),
            server_name,
        });
        self.take_effects()
    }

    /// Starts a git review of the session's working tree. Review state
    /// arrives as [`Effect::GitReviewState`] until it is closed.
    pub fn open_git_review(&mut self) -> Vec<Effect> {
        self.effects.push(Effect::Dispatch(Command::GitReview(GitReviewCommand::Open {
            session_id: self.session.session_id().0.to_string(),
        })));
        self.take_effects()
    }

    pub fn close_git_review(&mut self) -> Vec<Effect> {
        self.effects.push(Effect::Dispatch(Command::GitReview(GitReviewCommand::Close)));
        self.take_effects()
    }

    /// Asks the agent where the session's workspace is checked out.
    pub fn refresh_workspace_status(&mut self) -> Vec<Effect> {
        self.resolve_workspace(self.session.working_dir().to_path_buf());
        self.take_effects()
    }

    /// Appends the host's own notice for information outside the agent's
    /// output. Unlike the notices the core raises, it is not echoed back as an
    /// [`Effect::Notice`].
    pub fn notify(&mut self, message: &str) {
        self.conversation.append_notice(format!("[{}] {message}", self.client_name));
    }

    fn dispatch(&mut self, command: AgentCommand) {
        self.effects.push(Effect::Dispatch(Command::Agent(command)));
    }

    /// A notice the core raised itself, which hosts may also surface outside
    /// the conversation.
    fn raise_notice(&mut self, message: &str) {
        self.notify(message);
        self.effects.push(Effect::Notice(message.to_string()));
    }

    /// Drops all conversation state atomically before starting a new session.
    fn reset_conversation(&mut self) {
        self.conversation.clear();
        self.ended_turn = None;
        self.foreground.drop_prepared_prompt();
    }

    fn end_turn(&mut self, end: TurnEnd) {
        self.ended_turn = self.ended_turn.max(Some(end));
    }

    /// A turn's idle update and its prompt's answer arrive in either order, so
    /// a finished turn is announced only once the session has settled: the
    /// turn is idle and no prompt is left queued.
    fn announce_if_settled(&mut self) {
        if !self.waiting_for_response() && self.ended_turn.take() == Some(TurnEnd::Finished) {
            self.effects.push(Effect::TurnFinished);
        }
    }

    fn take_effects(&mut self) -> Vec<Effect> {
        self.plan_tracker.sync(self.conversation.plan(), Instant::now());
        std::mem::take(&mut self.effects)
    }
}

/// Ordered so a cancellation outweighs any turn that finished alongside it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum TurnEnd {
    Finished,
    Cancelled,
}
