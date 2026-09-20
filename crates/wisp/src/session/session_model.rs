use crate::session::session_config_view::{LocalConfigKind, LocalConfigOption};
use crate::session::workspace_status::WorkspaceStatus;
use crate::session::WorkspaceAccess;
use acp_utils::notifications::{AetherCapabilities, McpServerStatus, McpServerStatusEntry};
use agent_client_protocol::schema::v2::{self as acp, SessionId};
use std::path::{Path, PathBuf};

pub struct SessionModel {
    session_id: SessionId,
    agent_name: String,
    working_dir: PathBuf,
    workspace_access: WorkspaceAccess,
    workspace_status: WorkspaceStatus,
    prompt_capabilities: acp::PromptCapabilities,
    capabilities: AetherCapabilities,
    config_options: Vec<LocalConfigOption>,
    auth_methods: Vec<acp::AuthMethod>,
    server_statuses: Vec<McpServerStatusEntry>,
}

impl SessionModel {
    pub fn from_config(config: crate::app::AppConfig, capabilities: AetherCapabilities) -> Self {
        let crate::app::AppConfig {
            initialize_response,
            session_response,
            working_dir,
            workspace_status,
            workspace_access,
            ..
        } = config;
        let workspace_status = match workspace_access {
            WorkspaceAccess::Local => workspace_status,
            WorkspaceAccess::Remote => WorkspaceStatus::initial(&working_dir),
        };
        Self {
            workspace_access,
            session_id: session_response.session_id,
            agent_name: initialize_response.info.title.unwrap_or(initialize_response.info.name),
            working_dir,
            workspace_status,
            prompt_capabilities: initialize_response.capabilities.session.and_then(|session| session.prompt).unwrap_or_default(),
            capabilities,
            config_options: session_response.config_options.into_iter().map(LocalConfigOption::from_acp).collect(),
            auth_methods: initialize_response.auth_methods,
            server_statuses: Vec::new(),
        }
    }

    pub fn session_id(&self) -> &SessionId {
        &self.session_id
    }

    pub fn agent_name(&self) -> &str {
        &self.agent_name
    }

    pub fn workspace_access(&self) -> WorkspaceAccess {
        self.workspace_access
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
        self.config_options = config_options.into_iter().map(LocalConfigOption::from_acp).collect();
    }

    pub fn update_config_option_value(&mut self, config_id: &str, value: &str) {
        if let Some(option) = self.config_options.iter_mut().find(|option| option.id == config_id)
            && let LocalConfigKind::Select { current_value, .. } = &mut option.kind
        {
            current_value.clear();
            current_value.push_str(value);
        }
    }

    pub fn set_session(&mut self, session_id: SessionId, config_options: Vec<acp::SessionConfigOption>) {
        self.session_id = session_id;
        self.update_config_options(config_options);
    }

    pub fn set_working_dir(&mut self, working_dir: PathBuf) {
        if self.workspace_access == WorkspaceAccess::Remote {
            self.workspace_status = WorkspaceStatus::initial(&working_dir);
        }
        self.working_dir = working_dir;
    }

    pub fn set_workspace_status(&mut self, workspace_status: WorkspaceStatus) {
        self.workspace_status = workspace_status;
    }
}
