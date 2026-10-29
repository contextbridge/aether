use super::{Effect, ForegroundOperation, SessionCore};
use crate::command::AgentCommand;
use crate::session_model::SessionSetting;
use acp_utils::client::AcpClientError;
use acp_utils::notifications::WorkspaceMoveTarget;
use agent_client_protocol::schema::v2::{self as acp, SessionId};
use std::path::{Path, PathBuf};

impl SessionCore {
    pub fn new_session(&mut self) {
        if !self.can_start_foreground_operation() {
            self.notify("Cannot clear while a prompt or session transition is in progress");
            return;
        }
        self.reset_conversation(self.session.session_id().clone());
        self.foreground = ForegroundOperation::CreatingSession {
            previous_selections: self
                .session
                .config_options()
                .iter()
                .filter_map(|option| option.select().map(|select| (option.id.clone(), select.current_value.clone())))
                .collect(),
        };
        self.dispatch(AgentCommand::NewSession { cwd: self.session.working_dir().to_path_buf() });
    }

    pub fn list_sessions(&mut self) {
        self.dispatch(AgentCommand::ListSessions);
    }

    pub fn preview_session(&mut self, session_id: SessionId) {
        self.dispatch(AgentCommand::SessionPreview { session_id });
    }

    pub fn resume_session(&mut self, session_id: &SessionId, cwd: &Path) {
        if !self.can_start_foreground_operation() {
            self.notify("Cannot resume while a prompt or session transition is in progress");
            return;
        }
        self.reset_conversation(session_id.clone());
        self.set_session(session_id.clone(), Vec::new());
        self.foreground = ForegroundOperation::ResumingSession {
            session_id: session_id.clone(),
            cwd: cwd.to_path_buf(),
            moved: false,
        };
        self.dispatch(AgentCommand::ResumeSession { session_id: session_id.clone(), cwd: cwd.to_path_buf() });
        self.effects.push(Effect::SessionChanged);
    }

    pub fn begin_workspace_move(&mut self) {
        if !self.can_start_foreground_operation() {
            self.notify("Cannot move workspace while a prompt is running or another move is in progress");
            return;
        }
        self.foreground = ForegroundOperation::ListingWorkspaces;
        self.dispatch(AgentCommand::ListWorkspaces { session_id: self.session.session_id().clone() });
    }

    pub fn move_workspace(&mut self, target: WorkspaceMoveTarget) {
        self.foreground = ForegroundOperation::MovingWorkspace;
        self.dispatch(AgentCommand::MoveWorkspace { session_id: self.session.session_id().clone(), target });
    }

    pub fn cancel_workspace_pick(&mut self) {
        if matches!(self.foreground, ForegroundOperation::PickingWorkspace) {
            self.foreground = ForegroundOperation::Idle;
        }
    }

    pub(super) fn on_new_session(&mut self, session_id: SessionId, config_options: Vec<acp::SessionConfigOption>) {
        let ForegroundOperation::CreatingSession { previous_selections } = std::mem::take(&mut self.foreground) else {
            return;
        };
        self.effects.push(Effect::SessionChanged);
        self.conversation.reset(session_id.clone());
        self.set_session(session_id, config_options);
        self.restore_config_selections(&previous_selections);
    }

    pub(super) fn on_resume_result(
        &mut self,
        session_id: &SessionId,
        result: Result<acp::ResumeSessionResponse, AcpClientError>,
    ) {
        let (cwd, moved) = match &self.foreground {
            ForegroundOperation::ResumingSession { session_id: expected, cwd, moved } if expected == session_id => {
                (cwd.clone(), *moved)
            }
            _ => return,
        };
        let response = match result {
            Ok(response) => response,
            Err(error) => {
                self.foreground = ForegroundOperation::Idle;
                self.report_error(&format!("Failed to resume session: {error}"));
                return;
            }
        };
        if self.session.working_dir() != cwd {
            self.change_working_dir(cwd);
        }
        self.session.update_config_options(response.config_options);
        self.settings_changed(SessionSetting::ConfigOptions);
        self.conversation.unconfirm_sub_agents();
        if moved {
            self.notify(&format!("Moved to {}", self.session.working_dir().display()));
        }
        self.foreground = ForegroundOperation::Idle;
    }

    pub(super) fn on_workspace_moved(&mut self, new_cwd: PathBuf) {
        self.effects.push(Effect::SessionChanged);
        let session_id = self.session.session_id().clone();
        self.reset_conversation(session_id.clone());
        self.foreground =
            ForegroundOperation::ResumingSession { session_id: session_id.clone(), cwd: new_cwd.clone(), moved: true };
        self.dispatch(AgentCommand::ResumeSession { session_id, cwd: new_cwd.clone() });
        self.change_working_dir(new_cwd);
    }

    pub(super) fn abandon_workspace_move(&mut self, message: &str) {
        self.report_error(message);
        self.foreground = ForegroundOperation::Idle;
    }

    pub(super) fn request_config_option(&mut self, config_id: &str, value: &str) {
        self.dispatch(AgentCommand::SetConfigOption {
            conversation_id: self.conversation.id(),
            session_id: self.session.session_id().clone(),
            config_id: config_id.to_string(),
            value: value.into(),
        });
    }

    pub(super) fn resolve_workspace(&mut self, cwd: PathBuf) {
        self.dispatch(AgentCommand::FetchWorkspaceStatus { session_id: self.session.session_id().clone(), cwd });
    }

    fn change_working_dir(&mut self, cwd: PathBuf) {
        self.session.set_working_dir(cwd.clone());
        self.resolve_workspace(cwd);
    }

    fn set_session(&mut self, session_id: SessionId, config_options: Vec<acp::SessionConfigOption>) {
        self.session.set_session(session_id, config_options);
        self.settings_changed(SessionSetting::ConfigOptions);
    }

    fn restore_config_selections(&mut self, previous: &[(String, String)]) {
        for (config_id, value) in previous {
            let Some(option) = self.session.config_options().iter().find(|option| option.id == *config_id) else {
                continue;
            };
            let Some(select) = option.select() else {
                continue;
            };
            if select.current_value != *value {
                self.request_config_option(config_id, value);
            }
        }
    }
}
