//! The per-session client state machine every frontend drives.
//!
//! A [`SessionCore`] owns everything about one agent session that is not
//! presentation: the session model, the conversation, the prompts in flight,
//! and the operation the client is in the middle of. Hosts feed it agent
//! events, command results, and user intents, then drain the [`Effect`]s those
//! produced with [`SessionCore::take_effects`].

mod acp_reducer;
mod effect;
mod foreground;
mod prompts;
mod session;

pub use effect::{Effect, Elicitation};
pub use foreground::ForegroundOperation;
pub use prompts::QueuedPrompt;

use crate::command::{AgentCommand, Command, CommandResult, GitReviewCommand, ReviewSource};
use crate::command_entry::CommandEntry;
use crate::session_model::SessionModel;
use crate::workspace_status::{WorkspaceAccess, WorkspaceStatus};
use acp_utils::conversation::Conversation;
use acp_utils::notifications::AetherCapabilities;
use agent_client_protocol::schema::v2 as acp;
use std::path::PathBuf;

/// One agent session's protocol state and the transitions over it.
pub struct SessionCore {
    session: SessionModel,
    conversation: Conversation,
    available_commands: Vec<CommandEntry>,
    foreground: ForegroundOperation,
    queued_prompts: Vec<QueuedPrompt>,
    /// How the work since the session was last settled ended, if any turn has.
    ended_turn: Option<TurnEnd>,
    client_name: String,
    /// What the transitions since the host last drained them owe it.
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
        let client_name = config.client_name.clone();
        Self {
            session: SessionModel::from_config(config, capabilities),
            conversation: Conversation::new(),
            available_commands: Vec::new(),
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

    /// The commands the agent advertised.
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

    /// The effects produced since the last call, in order.
    pub fn take_effects(&mut self) -> Vec<Effect> {
        std::mem::take(&mut self.effects)
    }

    /// Reduces the outcome of a command the host dispatched, handing back the
    /// results only the host reduces, such as git review state and listings.
    pub fn on_command_result<T>(&mut self, result: CommandResult<T>) -> Option<CommandResult<T>> {
        match result {
            CommandResult::Prompt { request_id, result } => self.finish_prompt(request_id, result),
            CommandResult::Cancel(Err(error))
            | CommandResult::AuthenticateMcp(Err(error))
            | CommandResult::BackgroundFailed(error) => self.notify(&error),
            CommandResult::NewSession(Ok(response)) => {
                self.on_new_session(response.session_id, response.config_options);
            }
            CommandResult::NewSession(Err(error)) => {
                self.foreground = ForegroundOperation::Idle;
                self.notify(&format!("Failed to create new session: {error}"));
            }
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
                        self.notify(&format!("Failed to update setting: {error}"));
                    }
                }
            }
            CommandResult::Cancel(Ok(()))
            | CommandResult::AuthenticateMcp(Ok(()))
            | CommandResult::ConfigOptionsUpdated { .. } => {}
            CommandResult::SubmissionPrepared(outcome) => self.finish_submission(outcome),
            CommandResult::WorkspaceResolved { cwd, status } => {
                if self.session.working_dir() == cwd {
                    self.session.set_workspace_status(status);
                }
            }
            CommandResult::SessionsListed(Err(error)) => self.notify(&format!("Failed to list sessions: {error}")),
            result @ CommandResult::WorkspacesListed(Ok(_)) => {
                self.foreground = ForegroundOperation::PickingWorkspace;
                return Some(result);
            }
            CommandResult::WorkspacesListed(Err(error)) => {
                self.abandon_workspace_move(&format!("Failed to list workspaces: {error}"));
            }
            CommandResult::WorkspaceMoved(Ok(response)) => self.on_workspace_moved(response.new_cwd),
            CommandResult::WorkspaceMoved(Err(error)) => {
                self.abandon_workspace_move(&format!("Workspace move failed: {error}"));
            }
            result @ (CommandResult::AuthenticationCompleted { .. }
            | CommandResult::FilesIndexed { .. }
            | CommandResult::GitReview(_)
            | CommandResult::GitReviewAction(_)
            | CommandResult::SessionsListed(Ok(_))
            | CommandResult::PromptSearchResults { .. }
            | CommandResult::SessionPreviewLoaded { .. }
            | CommandResult::Host(_)) => return Some(result),
        }
        None
    }

    /// Asks the agent to cancel the running turn, if there is one.
    pub fn cancel(&mut self) {
        if self.waiting_for_response() {
            self.dispatch(AgentCommand::Cancel { session_id: self.session.session_id().clone() });
        }
    }

    /// Selects `value` for `config_id`: the local state changes at once and the
    /// agent is asked to apply it, confirming or correcting it in its reply.
    pub fn set_config_option(&mut self, config_id: &str, value: &str) {
        self.session.update_config_option_value(config_id, value);
        self.request_config_option(config_id, value);
    }

    pub fn authenticate(&mut self, method_id: String) {
        self.dispatch(AgentCommand::Authenticate { method_id });
    }

    pub fn authenticate_mcp_server(&mut self, server_name: String) {
        self.dispatch(AgentCommand::AuthenticateMcpServer {
            session_id: self.session.session_id().clone(),
            server_name,
        });
    }

    /// Starts a git review of the session's working tree. Its state arrives as
    /// [`CommandResult::GitReview`] until it is closed.
    pub fn open_git_review(&mut self) {
        let source = if self.session.capabilities().git_diff {
            ReviewSource::Agent { session_id: self.session.session_id().0.to_string() }
        } else if self.session.workspace_access() == WorkspaceAccess::Local {
            ReviewSource::Local(self.session.working_dir().to_path_buf())
        } else {
            ReviewSource::Unavailable("the agent serves no diff of its remote workspace".to_owned())
        };
        self.open_git_review_from(source);
    }

    pub fn open_git_review_from(&mut self, source: ReviewSource) {
        self.effects.push(Effect::Dispatch(Command::GitReview(GitReviewCommand::Open(source))));
    }

    pub fn close_git_review(&mut self) {
        self.effects.push(Effect::Dispatch(Command::GitReview(GitReviewCommand::Close)));
    }

    /// Asks the agent where the session's workspace is checked out.
    pub fn refresh_workspace_status(&mut self) {
        self.resolve_workspace(self.session.working_dir().to_path_buf());
    }

    /// Appends a notice for information outside the agent's own output.
    pub fn notify(&mut self, message: &str) {
        self.conversation.append_notice(format!("[{}] {message}", self.client_name));
    }

    fn dispatch(&mut self, command: AgentCommand) {
        self.effects.push(Effect::Dispatch(Command::Agent(command)));
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
}

/// Ordered so a cancellation outweighs any turn that finished alongside it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum TurnEnd {
    Finished,
    Cancelled,
}
