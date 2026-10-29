mod acp_reducer;
mod effect;
mod foreground;
mod prompts;
mod transitions;

pub use effect::Effect;
pub use foreground::ForegroundOperation;
pub use prompts::QueuedPrompt;

use crate::command::{AgentCommand, Command, CommandResult, GitReviewCommand, Reply, ReviewSource};
use crate::composer::prompt_search::PromptSearchState;
use crate::elicitation::{Elicitation, ElicitationAnswer, ElicitationId, PendingElicitation};
use crate::runtime::Input;
use crate::session_model::{SessionModel, SessionSetting, WorkspaceAccess, WorkspaceStatus};
use acp_utils::conversation::{Conversation, NoticeLevel};
use agent_client_protocol::schema::v2 as acp;
use clankerdiff_client::{ClientState, DiffReviewEvent};
use std::path::PathBuf;
use std::sync::Arc;

pub struct SessionCore {
    session: SessionModel,
    conversation: Conversation,
    available_commands: Vec<CommandEntry>,
    foreground: ForegroundOperation,
    queued_prompts: Vec<QueuedPrompt>,
    ended_turn: Option<TurnEnd>,
    elicitation: Option<PendingElicitation>,
    elicitations: u64,
    git_review: GitReview,
    prompt_search: PromptSearchState,
    effects: Vec<Effect>,
}

pub struct SessionCoreConfig {
    pub initialize_response: acp::InitializeResponse,
    pub session_response: acp::NewSessionResponse,
    pub working_dir: PathBuf,
    pub workspace_status: WorkspaceStatus,
    pub workspace_access: WorkspaceAccess,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandEntry {
    pub name: String,
    pub description: String,
    pub has_input: bool,
    pub hint: Option<String>,
}

impl SessionCore {
    pub fn new(config: SessionCoreConfig) -> Self {
        let session = SessionModel::from_config(config);
        let conversation = Conversation::new(session.session_id().clone());
        Self {
            session,
            conversation,
            available_commands: Vec::new(),
            foreground: ForegroundOperation::Idle,
            queued_prompts: Vec::new(),
            ended_turn: None,
            elicitation: None,
            elicitations: 0,
            git_review: GitReview::Closed,
            prompt_search: PromptSearchState::default(),
            effects: Vec::new(),
        }
    }

    pub fn session(&self) -> &SessionModel {
        &self.session
    }

    pub fn conversation(&self) -> &Conversation {
        &self.conversation
    }

    pub fn available_commands(&self) -> &[CommandEntry] {
        &self.available_commands
    }

    pub fn foreground(&self) -> &ForegroundOperation {
        &self.foreground
    }

    pub fn queued_prompts(&self) -> &[QueuedPrompt] {
        &self.queued_prompts
    }

    pub fn elicitation(&self) -> Option<(ElicitationId, &Elicitation)> {
        self.elicitation.as_ref().map(|pending| (pending.id, &pending.elicitation))
    }

    pub fn git_review(&self) -> Option<&Arc<ClientState>> {
        match &self.git_review {
            GitReview::Open(state) => Some(state),
            GitReview::Closed | GitReview::Opening => None,
        }
    }

    pub fn prompt_search(&self) -> &PromptSearchState {
        &self.prompt_search
    }

    pub fn waiting_for_response(&self) -> bool {
        !self.conversation.turn().is_idle() || !self.queued_prompts.is_empty()
    }

    pub fn is_agent_busy(&self) -> bool {
        self.waiting_for_response() || self.conversation.any_running()
    }

    pub fn take_effects(&mut self) -> Vec<Effect> {
        std::mem::take(&mut self.effects)
    }

    pub fn on_input(&mut self, input: Input) {
        match input {
            Input::Event(event) => self.on_acp_event(event),
            Input::Result(result) => self.on_command_result(result),
        }
    }

    pub fn on_command_result(&mut self, result: CommandResult) {
        match result {
            CommandResult::Prompt { request_id, result } => self.finish_prompt(request_id, result),
            CommandResult::Notified(Err(error)) => self.report_error(&error.to_string()),
            CommandResult::BackgroundFailed(error) => {
                if let Some(submission) = self.foreground.take_prepared_prompt() {
                    self.effects.push(Effect::RestoreSubmission(submission));
                }
                self.report_error(&format!("Background task failed: {error}"));
            }
            CommandResult::NewSession(Ok(response)) => {
                self.on_new_session(response.session_id, response.config_options);
            }
            CommandResult::NewSession(Err(error)) => {
                self.foreground = ForegroundOperation::Idle;
                self.report_error(&format!("Failed to create new session: {error}"));
            }
            CommandResult::ResumeSession { session_id, result } => self.on_resume_result(&session_id, result),
            CommandResult::ConfigOptionsUpdated { conversation_id, result }
                if conversation_id == self.conversation.id() =>
            {
                match result {
                    Ok(response) => {
                        self.session.update_config_options(response.config_options);
                        self.settings_changed(SessionSetting::ConfigOptions);
                    }
                    Err(error) => {
                        tracing::warn!("set_session_config_option failed: {error}");
                        self.report_error(&format!("Failed to update setting: {error}"));
                    }
                }
            }
            CommandResult::Notified(Ok(())) | CommandResult::ConfigOptionsUpdated { .. } => {}
            CommandResult::SubmissionPrepared(outcome) => self.finish_submission(outcome),
            CommandResult::WorkspaceResolved { cwd, status } => {
                if self.session.working_dir() == cwd {
                    self.session.set_workspace_status(status);
                }
            }
            CommandResult::SessionsListed(Ok(response)) => self.reply(Reply::SessionsListed(response.sessions)),
            CommandResult::SessionsListed(Err(error)) => {
                self.report_error(&format!("Failed to list sessions: {error}"));
            }
            CommandResult::WorkspacesListed(Ok(response)) => {
                self.foreground = ForegroundOperation::PickingWorkspace;
                self.effects.push(Effect::WorkspacesListed(response.workspaces));
            }
            CommandResult::WorkspacesListed(Err(error)) => {
                self.abandon_workspace_move(&format!("Failed to list workspaces: {error}"));
            }
            CommandResult::WorkspaceMoved(Ok(response)) => self.on_workspace_moved(response.new_cwd),
            CommandResult::WorkspaceMoved(Err(error)) => {
                self.abandon_workspace_move(&format!("Workspace move failed: {error}"));
            }
            CommandResult::PromptSearchResults { query, result } => {
                if self.prompt_search.on_response(&query, result) {
                    self.effects.push(Effect::PromptSearchUpdated);
                }
            }
            CommandResult::GitReview(state) => {
                if self.git_review != GitReview::Closed {
                    self.git_review = GitReview::Open(state);
                    self.effects.push(Effect::GitReviewChanged);
                }
            }
            CommandResult::Reply(reply) => self.reply(reply),
        }
    }

    pub fn cancel(&mut self) {
        self.dismiss_elicitation();
        if self.waiting_for_response() {
            self.dispatch(AgentCommand::Cancel { session_id: self.session.session_id().clone() });
        }
    }

    pub fn answer_elicitation(&mut self, id: ElicitationId, answer: ElicitationAnswer) {
        if let Some(pending) = self.elicitation.take_if(|pending| pending.id == id) {
            pending.answer(answer);
        }
    }

    pub fn set_config_option(&mut self, config_id: &str, value: &str) {
        if self.session.update_config_option_value(config_id, value) {
            self.settings_changed(SessionSetting::ConfigOptions);
        }
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

    pub fn open_git_review(&mut self) {
        let source = if self.session.capabilities().git_diff {
            ReviewSource::Agent { session_id: self.session.session_id().clone() }
        } else if self.session.workspace_access() == WorkspaceAccess::Local {
            ReviewSource::Local(self.session.working_dir().to_path_buf())
        } else {
            ReviewSource::Unavailable("the agent serves no diff of its remote workspace".to_owned())
        };
        self.open_git_review_from(source);
    }

    pub fn open_git_review_from(&mut self, source: ReviewSource) {
        self.git_review = GitReview::Opening;
        self.effects.push(Effect::Dispatch(Command::GitReview(GitReviewCommand::Open(source))));
    }

    pub fn send_review_event(&mut self, event: DiffReviewEvent) {
        if self.git_review != GitReview::Closed {
            self.effects.push(Effect::Dispatch(Command::GitReview(GitReviewCommand::Event(event))));
        }
    }

    pub fn close_git_review(&mut self) {
        if std::mem::replace(&mut self.git_review, GitReview::Closed) != GitReview::Closed {
            self.effects.push(Effect::Dispatch(Command::GitReview(GitReviewCommand::Close)));
        }
    }

    pub fn refresh_workspace_status(&mut self) {
        self.resolve_workspace(self.session.working_dir().to_path_buf());
    }

    pub fn notify(&mut self, message: &str) {
        self.conversation.append_notice(NoticeLevel::Info, message);
    }

    pub fn report_error(&mut self, message: &str) {
        self.conversation.append_notice(NoticeLevel::Error, message);
    }

    fn reply(&mut self, mut reply: Reply) {
        match &mut reply {
            Reply::SessionsListed(sessions) => {
                sessions.retain(|session| session.session_id != *self.session.session_id());
            }
            Reply::AuthenticationCompleted { method_id, result: Err(error) } => {
                tracing::warn!("Provider authentication failed for {method_id}: {error}");
            }
            _ => {}
        }
        self.effects.push(Effect::Reply(reply));
    }

    fn dispatch(&mut self, command: AgentCommand) {
        self.effects.push(Effect::Dispatch(Command::Agent(command)));
    }

    fn dismiss_elicitation(&mut self) {
        if self.elicitation.take().is_some() {
            self.effects.push(Effect::ElicitationDismissed);
        }
    }

    fn reset_conversation(&mut self, session_id: acp::SessionId) {
        self.conversation.reset(session_id);
        self.ended_turn = None;
        self.foreground.take_prepared_prompt();
    }

    fn can_start_foreground_operation(&self) -> bool {
        self.foreground.is_idle() && !self.waiting_for_response()
    }

    fn settings_changed(&mut self, setting: SessionSetting) {
        self.effects.push(Effect::SessionSettingsChanged(setting));
    }

    fn end_turn(&mut self, end: TurnEnd) {
        self.ended_turn = self.ended_turn.max(Some(end));
    }

    fn announce_if_settled(&mut self) {
        if !self.waiting_for_response() && self.ended_turn.take() == Some(TurnEnd::Finished) {
            self.effects.push(Effect::TurnFinished);
        }
    }
}

#[derive(Debug, PartialEq)]
enum GitReview {
    Closed,
    Opening,
    Open(Arc<ClientState>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum TurnEnd {
    Finished,
    Cancelled,
}
