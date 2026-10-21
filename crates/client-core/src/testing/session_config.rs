use crate::session_core::SessionCoreConfig;
use crate::workspace_status::{WorkspaceAccess, WorkspaceStatus};
use acp_utils::notifications::AetherCapabilities;
use agent_client_protocol::schema::ProtocolVersion;
use agent_client_protocol::schema::v2 as acp;
use std::path::PathBuf;

/// Builds the [`SessionCoreConfig`] of a freshly connected test session:
/// session `test-session` with an agent named `aether`, working in `.` on the
/// `main` branch of `~/code/demo`, unless told otherwise.
pub struct SessionCoreConfigBuilder {
    working_dir: PathBuf,
    workspace_access: WorkspaceAccess,
    workspace_status: WorkspaceStatus,
    capabilities: AetherCapabilities,
    session_capabilities: Option<acp::SessionCapabilities>,
    prompt_capabilities: acp::PromptCapabilities,
    config_options: Vec<acp::SessionConfigOption>,
    auth_methods: Vec<acp::AuthMethod>,
    client_name: String,
}

impl Default for SessionCoreConfigBuilder {
    fn default() -> Self {
        Self {
            working_dir: PathBuf::from("."),
            workspace_access: WorkspaceAccess::Local,
            workspace_status: WorkspaceStatus {
                display_dir: "~/code/demo".to_string(),
                git_ref: Some("main".to_string()),
            },
            capabilities: AetherCapabilities::default(),
            session_capabilities: None,
            prompt_capabilities: acp::PromptCapabilities::new(),
            config_options: Vec::new(),
            auth_methods: Vec::new(),
            client_name: "test".to_string(),
        }
    }
}

impl SessionCoreConfigBuilder {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn working_dir(mut self, working_dir: impl Into<PathBuf>) -> Self {
        self.working_dir = working_dir.into();
        self
    }

    pub fn remote_workspace(mut self) -> Self {
        self.workspace_access = WorkspaceAccess::Remote;
        self
    }

    pub fn workspace_status(mut self, workspace_status: WorkspaceStatus) -> Self {
        self.workspace_status = workspace_status;
        self
    }

    pub fn prompt_capabilities(mut self, capabilities: acp::PromptCapabilities) -> Self {
        self.prompt_capabilities = capabilities;
        self
    }

    pub fn config_options(mut self, options: Vec<acp::SessionConfigOption>) -> Self {
        self.config_options = options;
        self
    }

    pub fn auth_methods(mut self, methods: Vec<acp::AuthMethod>) -> Self {
        self.auth_methods = methods;
        self
    }

    /// Overrides the session capabilities wholesale, for tests that care about
    /// metadata the individual toggles do not cover.
    pub fn session_capabilities(mut self, capabilities: acp::SessionCapabilities) -> Self {
        self.session_capabilities = Some(capabilities);
        self
    }

    pub fn prompt_search(mut self) -> Self {
        self.capabilities.prompt_search = true;
        self
    }

    pub fn session_preview(mut self) -> Self {
        self.capabilities.session_preview = true;
        self
    }

    pub fn workspace_move(mut self) -> Self {
        self.capabilities.workspace_move = true;
        self
    }

    pub fn client_name(mut self, client_name: impl Into<String>) -> Self {
        self.client_name = client_name.into();
        self
    }

    pub fn build(&self) -> SessionCoreConfig {
        let session_capabilities = self
            .session_capabilities
            .clone()
            .unwrap_or_else(|| acp::SessionCapabilities::new().meta(Some(self.capabilities.clone().to_meta())));
        SessionCoreConfig {
            initialize_response: acp::InitializeResponse::new(
                ProtocolVersion::V2,
                acp::Implementation::new("aether", "test"),
            )
            .capabilities(
                acp::AgentCapabilities::new().session(session_capabilities.prompt(self.prompt_capabilities.clone())),
            )
            .auth_methods(self.auth_methods.clone()),
            session_response: acp::NewSessionResponse::new("test-session").config_options(self.config_options.clone()),
            working_dir: self.working_dir.clone(),
            workspace_status: self.workspace_status.clone(),
            workspace_access: self.workspace_access,
            client_name: self.client_name.clone(),
        }
    }
}
