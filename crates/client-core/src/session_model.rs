use crate::session_config_view::{LocalConfigKind, LocalConfigOption, LocalConfigView};
use crate::session_core::SessionCoreConfig;
use acp_utils::config_option_id::ConfigOptionId;
use acp_utils::notifications::{AetherCapabilities, McpServerStatus, McpServerStatusEntry};
use agent_client_protocol::schema::v2::{self as acp, SessionId};
use std::path::{Path, PathBuf};

pub use acp_utils::notifications::WorkspaceStatusResponse as WorkspaceStatus;

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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionSetting {
    ConfigOptions,
    AuthMethods,
    ServerStatuses,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum WorkspaceAccess {
    #[default]
    Local,
    Remote,
}

pub fn initial_workspace_status(cwd: &Path) -> WorkspaceStatus {
    WorkspaceStatus { display_dir: cwd.display().to_string(), git_ref: None }
}

impl SessionModel {
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

    pub fn media_support_error(&self, blocks: &[acp::ContentBlock]) -> Option<String> {
        let requires_image = blocks.iter().any(|block| matches!(block, acp::ContentBlock::Image(_)));
        let requires_audio = blocks.iter().any(|block| matches!(block, acp::ContentBlock::Audio(_)));

        if !requires_image && !requires_audio {
            return None;
        }

        if requires_image && self.prompt_capabilities.image.is_none() {
            return Some("ACP agent does not support image input.".to_string());
        }
        if requires_audio && self.prompt_capabilities.audio.is_none() {
            return Some("ACP agent does not support audio input.".to_string());
        }

        let config = LocalConfigView::new(&self.config_options);
        let model = config.select(ConfigOptionId::Model)?;
        let selected_count = model.current_values().count();
        if selected_count == 0 {
            return None;
        }
        let selected_meta = config.selected_model_metadata();

        if selected_meta.len() != selected_count {
            return Some("Current model selection is missing prompt capability metadata.".into());
        }

        if requires_image && selected_meta.iter().any(|meta| !meta.supports_image) {
            return Some("Current model selection does not support image input.".to_string());
        }
        if requires_audio && selected_meta.iter().any(|meta| !meta.supports_audio) {
            return Some("Current model selection does not support audio input.".to_string());
        }

        None
    }

    pub(crate) fn from_config(config: SessionCoreConfig) -> Self {
        let SessionCoreConfig {
            initialize_response,
            session_response,
            working_dir,
            workspace_status,
            workspace_access,
        } = config;
        Self {
            capabilities: AetherCapabilities::of(&initialize_response.capabilities),
            workspace_access,
            session_id: session_response.session_id,
            agent_name: initialize_response.info.title.unwrap_or(initialize_response.info.name),
            working_dir,
            workspace_status,
            prompt_capabilities: initialize_response
                .capabilities
                .session
                .and_then(|session| session.prompt)
                .unwrap_or_default(),
            config_options: session_response.config_options.into_iter().map(LocalConfigOption::from_acp).collect(),
            auth_methods: initialize_response.auth_methods,
            server_statuses: Vec::new(),
        }
    }

    pub(crate) fn update_server_statuses(&mut self, statuses: &[McpServerStatusEntry]) {
        self.server_statuses = statuses.to_vec();
    }

    pub(crate) fn set_auth_methods(&mut self, auth_methods: &[acp::AuthMethod]) {
        self.auth_methods = auth_methods.to_vec();
    }

    pub(crate) fn update_config_options(&mut self, config_options: Vec<acp::SessionConfigOption>) {
        self.config_options = config_options.into_iter().map(LocalConfigOption::from_acp).collect();
    }

    pub(crate) fn update_config_option_value(&mut self, config_id: &str, value: &str) -> bool {
        let select = self.config_options.iter_mut().find(|option| option.id == config_id).and_then(|option| {
            match &mut option.kind {
                LocalConfigKind::Select(select) => Some(select),
                LocalConfigKind::Boolean { .. } => None,
            }
        });
        let Some(select) = select else {
            return false;
        };
        value.clone_into(&mut select.current_value);
        true
    }

    pub(crate) fn set_session(&mut self, session_id: SessionId, config_options: Vec<acp::SessionConfigOption>) {
        self.session_id = session_id;
        self.update_config_options(config_options);
    }

    pub(crate) fn set_working_dir(&mut self, working_dir: PathBuf) {
        if self.workspace_access == WorkspaceAccess::Remote {
            self.workspace_status = initial_workspace_status(&working_dir);
        }
        self.working_dir = working_dir;
    }

    pub(crate) fn set_workspace_status(&mut self, workspace_status: WorkspaceStatus) {
        self.workspace_status = workspace_status;
    }
}
