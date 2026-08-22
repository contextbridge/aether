use crate::session::session_config_view::{LocalConfigKind, LocalConfigOption};
use crate::session::session_loading_buffer::SessionLoadingBuffer;
use crate::session::workspace_status::WorkspaceStatus;
use acp_utils::notifications::{AetherCapabilities, McpServerStatus, McpServerStatusEntry};
use agent_client_protocol::schema::v1::{self as acp, SessionId, SessionUpdate};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkspaceMoveState {
    Idle,
    Listing,
    Picking,
    Moving,
    LoadingSession,
}

impl WorkspaceMoveState {
    pub fn is_idle(self) -> bool {
        matches!(self, Self::Idle)
    }
}

pub struct SessionModel {
    session_id: SessionId,
    agent_name: String,
    working_dir: PathBuf,
    workspace_status: WorkspaceStatus,
    prompt_capabilities: acp::PromptCapabilities,
    capabilities: AetherCapabilities,
    config_options: Vec<LocalConfigOption>,
    auth_methods: Vec<acp::AuthMethod>,
    loading_buffer: SessionLoadingBuffer,
    workspace_move_state: WorkspaceMoveState,
    server_statuses: Vec<McpServerStatusEntry>,
}

impl SessionModel {
    pub fn from_config(config: crate::app::AppConfig, capabilities: AetherCapabilities) -> Self {
        let crate::app::AppConfig {
            session_id,
            agent_name,
            working_dir,
            workspace_status,
            prompt_capabilities,
            config_options,
            auth_methods,
            ..
        } = config;
        Self {
            session_id,
            agent_name,
            working_dir,
            workspace_status,
            prompt_capabilities,
            capabilities,
            config_options: config_options.into_iter().map(LocalConfigOption::from_acp).collect(),
            auth_methods,
            loading_buffer: SessionLoadingBuffer::default(),
            workspace_move_state: WorkspaceMoveState::Idle,
            server_statuses: Vec::new(),
        }
    }

    pub fn session_id(&self) -> &SessionId {
        &self.session_id
    }

    pub fn agent_name(&self) -> &str {
        &self.agent_name
    }

    pub fn working_dir(&self) -> &Path {
        &self.working_dir
    }

    pub fn prompt_capabilities(&self) -> &acp::PromptCapabilities {
        &self.prompt_capabilities
    }

    pub fn capabilities(&self) -> &AetherCapabilities {
        &self.capabilities
    }

    pub fn config_options(&self) -> &[LocalConfigOption] {
        &self.config_options
    }

    pub fn auth_methods(&self) -> &[acp::AuthMethod] {
        &self.auth_methods
    }

    pub fn workspace_status(&self) -> &WorkspaceStatus {
        &self.workspace_status
    }

    pub fn workspace_move_state(&self) -> WorkspaceMoveState {
        self.workspace_move_state
    }

    pub fn begin_workspace_listing(&mut self) {
        self.workspace_move_state = WorkspaceMoveState::Listing;
    }

    pub fn begin_workspace_picking(&mut self) {
        self.workspace_move_state = WorkspaceMoveState::Picking;
    }

    pub fn begin_workspace_move(&mut self) {
        self.workspace_move_state = WorkspaceMoveState::Moving;
    }

    /// The move landed; the session is being reloaded in the new workspace.
    pub fn begin_workspace_load(&mut self) {
        self.workspace_move_state = WorkspaceMoveState::LoadingSession;
    }

    /// Ends the move flow, wherever it was: a finished load, a failure, or the
    /// user backing out.
    pub fn end_workspace_move(&mut self) {
        self.workspace_move_state = WorkspaceMoveState::Idle;
    }

    /// Leaves picking mode when the picker closes. Later phases survive an
    /// overlay close, because the move is already in flight.
    pub fn cancel_workspace_picking(&mut self) {
        if self.workspace_move_state == WorkspaceMoveState::Picking {
            self.workspace_move_state = WorkspaceMoveState::Idle;
        }
    }

    /// Stops waiting on a workspace-move session load that will never land.
    pub fn abandon_workspace_load(&mut self) {
        if self.workspace_move_state == WorkspaceMoveState::LoadingSession {
            self.workspace_move_state = WorkspaceMoveState::Idle;
        }
    }

    pub fn server_statuses(&self) -> &[McpServerStatusEntry] {
        &self.server_statuses
    }

    pub fn unhealthy_server_count(&self) -> usize {
        self.server_statuses.iter().filter(|server| !matches!(server.status, McpServerStatus::Connected { .. })).count()
    }

    pub fn update_server_statuses(&mut self, statuses: &[McpServerStatusEntry]) {
        self.server_statuses = statuses.to_vec();
    }

    pub fn set_auth_methods(&mut self, auth_methods: &[acp::AuthMethod]) {
        self.auth_methods = auth_methods.to_vec();
    }

    /// Reconciliation policy: the agent's update replaces local state
    /// wholesale. Optimistic edits made through `update_config_option_value`
    /// are either confirmed or corrected by the next update.
    pub fn update_config_options(&mut self, config_options: Vec<acp::SessionConfigOption>) {
        let next = config_options.into_iter().map(LocalConfigOption::from_acp).collect::<Vec<_>>();
        self.config_options = next;
    }

    pub fn update_config_option_value(&mut self, config_id: &str, value: &str) {
        if let Some(option) = self.config_options.iter_mut().find(|option| option.id == config_id)
            && let LocalConfigKind::Select { current_value, .. } = &mut option.kind
        {
            current_value.clear();
            current_value.push_str(value);
        }
    }

    pub fn begin_load(&mut self, session_id: SessionId) {
        self.loading_buffer.begin_load(session_id);
    }

    pub fn buffer_update(&mut self, session_id: &SessionId, update: SessionUpdate) -> Option<SessionUpdate> {
        self.loading_buffer.push(session_id, update)
    }

    pub fn take_buffered_updates(&mut self, session_id: &SessionId) -> Vec<SessionUpdate> {
        self.loading_buffer.take(session_id)
    }

    pub fn clear_loads(&mut self) {
        self.loading_buffer.clear();
    }

    pub fn set_session(&mut self, session_id: SessionId, config_options: Vec<acp::SessionConfigOption>) {
        self.session_id = session_id;
        self.config_options = config_options.into_iter().map(LocalConfigOption::from_acp).collect();
    }

    pub fn set_working_dir(&mut self, working_dir: PathBuf) {
        self.working_dir = working_dir;
    }

    pub fn set_workspace_status(&mut self, workspace_status: WorkspaceStatus) {
        self.workspace_status = workspace_status;
    }
}
