use super::{Effect, ForegroundOperation, SessionCore};
use crate::command::AgentCommand;
use acp_utils::notifications::WorkspaceMoveTarget;
use agent_client_protocol::schema::v2::{self as acp, SessionId};
use std::path::{Path, PathBuf};

impl SessionCore {
    /// Replaces the session with a new one in the same working directory,
    /// carrying the current model and mode selections over.
    pub fn new_session(&mut self) -> Vec<Effect> {
        if !self.can_start_foreground_operation() {
            self.raise_notice("Cannot clear while a prompt or session transition is in progress");
            return self.take_effects();
        }
        self.reset_conversation();
        self.foreground = ForegroundOperation::CreatingSession {
            previous_selections: self
                .session
                .config_options()
                .iter()
                .filter_map(|option| {
                    option.select().map(|select| (option.id.clone(), select.current_value.to_string()))
                })
                .collect(),
        };
        self.dispatch(AgentCommand::NewSession { cwd: self.session.working_dir().to_path_buf() });
        self.take_effects()
    }

    /// Asks the agent for its sessions. They arrive as [`Effect::SessionsListed`].
    pub fn list_sessions(&mut self) -> Vec<Effect> {
        self.dispatch(AgentCommand::ListSessions);
        self.take_effects()
    }

    pub fn preview_session(&mut self, session_id: String) -> Vec<Effect> {
        self.dispatch(AgentCommand::SessionPreview { session_id });
        self.take_effects()
    }

    /// Switches to a saved session, replaying its history from the start into
    /// an emptied conversation.
    pub fn resume_session(&mut self, session_id: &SessionId, cwd: &Path) -> Vec<Effect> {
        if !self.can_start_foreground_operation() {
            self.raise_notice("Cannot resume while a prompt or session transition is in progress");
            return self.take_effects();
        }
        self.reset_conversation();
        self.session.set_session(session_id.clone(), Vec::new());
        self.foreground =
            ForegroundOperation::ResumingSession { session_id: session_id.clone(), cwd: cwd.to_path_buf() };
        self.dispatch(AgentCommand::ResumeSession { session_id: session_id.clone(), cwd: cwd.to_path_buf() });
        self.effects.push(Effect::ResumeStarted);
        self.take_effects()
    }

    /// Starts moving the session to another workspace by listing the
    /// candidates, which arrive as [`Effect::WorkspacesListed`].
    pub fn begin_workspace_move(&mut self) -> Vec<Effect> {
        if !self.can_start_foreground_operation() {
            self.raise_notice("Cannot move workspace while a prompt is running or another move is in progress");
            return self.take_effects();
        }
        self.foreground = ForegroundOperation::ListingWorkspaces;
        self.dispatch(AgentCommand::ListWorkspaces { session_id: self.session.session_id().0.to_string() });
        self.take_effects()
    }

    /// Moves the session to the workspace the user picked.
    pub fn move_workspace(&mut self, target: WorkspaceMoveTarget) -> Vec<Effect> {
        self.foreground = ForegroundOperation::MovingWorkspace;
        self.dispatch(AgentCommand::MoveWorkspace { session_id: self.session.session_id().0.to_string(), target });
        self.take_effects()
    }

    /// The user dismissed the workspace list without picking one.
    pub fn cancel_workspace_pick(&mut self) {
        if matches!(self.foreground, ForegroundOperation::PickingWorkspace) {
            self.foreground = ForegroundOperation::Idle;
        }
    }

    pub(super) fn on_new_session(&mut self, session_id: SessionId, config_options: Vec<acp::SessionConfigOption>) {
        if !matches!(self.foreground, ForegroundOperation::CreatingSession { .. })
            && !self.can_start_foreground_operation()
        {
            return;
        }
        let previous_selections = match std::mem::take(&mut self.foreground) {
            ForegroundOperation::CreatingSession { previous_selections } => previous_selections,
            _ => Vec::new(),
        };
        self.effects.push(Effect::SessionCreated);
        self.session.set_session(session_id, config_options);
        self.restore_config_selections(&previous_selections);
    }

    pub(super) fn on_resume_result(
        &mut self,
        session_id: &SessionId,
        result: Result<acp::ResumeSessionResponse, String>,
    ) {
        let cwd = match &self.foreground {
            ForegroundOperation::ResumingSession { session_id: expected, cwd }
            | ForegroundOperation::LoadingWorkspaceSession { session_id: expected, cwd }
                if expected == session_id =>
            {
                cwd.clone()
            }
            _ => return,
        };
        let response = match result {
            Ok(response) => response,
            Err(error) => {
                self.foreground = ForegroundOperation::Idle;
                self.raise_notice(&format!("Failed to resume session: {error}"));
                return;
            }
        };
        if self.session.working_dir() != cwd {
            self.session.set_working_dir(cwd.clone());
            self.resolve_workspace(cwd);
        }
        self.session.update_config_options(response.config_options);
        if matches!(self.foreground, ForegroundOperation::LoadingWorkspaceSession { .. }) {
            self.raise_notice(&format!("Moved to {}", self.session.working_dir().display()));
        }
        self.effects.push(Effect::SessionResumed);
        self.foreground = ForegroundOperation::Idle;
    }

    pub(super) fn on_workspace_moved(&mut self, new_cwd: PathBuf) {
        self.effects.push(Effect::WorkspaceMoved);
        self.raise_notice(&format!("Moved to {}", new_cwd.display()));
        let session_id = self.session.session_id().clone();
        self.reset_conversation();
        self.foreground =
            ForegroundOperation::LoadingWorkspaceSession { session_id: session_id.clone(), cwd: new_cwd.clone() };
        self.dispatch(AgentCommand::ResumeSession { session_id, cwd: new_cwd.clone() });
        self.session.set_working_dir(new_cwd.clone());
        self.resolve_workspace(new_cwd);
    }

    /// Reports why a workspace move could not proceed and leaves move mode.
    pub(super) fn abandon_workspace_move(&mut self, message: &str) {
        self.raise_notice(message);
        self.foreground = ForegroundOperation::Idle;
    }

    /// Queues a config change for the agent and session the core is attached to.
    pub(super) fn request_config_option(&mut self, config_id: &str, value: &str) {
        self.dispatch(AgentCommand::SetConfigOption {
            conversation_id: self.conversation.id(),
            session_id: self.session.session_id().clone(),
            config_id: config_id.to_string(),
            value: value.into(),
        });
    }

    pub(super) fn resolve_workspace(&mut self, cwd: PathBuf) {
        self.dispatch(AgentCommand::FetchWorkspaceStatus { session_id: self.session.session_id().0.to_string(), cwd });
    }

    /// Re-applies the previous session's selections to a freshly created one, so
    /// a new session keeps the model and mode the user had chosen.
    fn restore_config_selections(&mut self, previous: &[(String, String)]) {
        for (config_id, value) in previous {
            let Some(option) = self.session.config_options().iter().find(|option| option.id == *config_id) else {
                continue;
            };
            let Some(select) = option.select() else {
                continue;
            };
            if select.current_value != value {
                self.request_config_option(config_id, value);
            }
        }
    }
}
