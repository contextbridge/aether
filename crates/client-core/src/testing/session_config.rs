use crate::session_core::SessionCoreConfig;
use crate::session_model::{WorkspaceAccess, WorkspaceStatus, initial_workspace_status};
use acp_utils::notifications::{AetherCapabilities, AetherMeta};
use acp_utils::testing::FakeAgent;
use agent_client_protocol::schema::ProtocolVersion;
use agent_client_protocol::schema::v2 as acp;
use std::path::PathBuf;

pub struct SessionCoreConfigBuilder {
    working_dir: PathBuf,
    workspace_access: WorkspaceAccess,
    workspace_status: Option<WorkspaceStatus>,
    capabilities: AetherCapabilities,
    prompt_capabilities: acp::PromptCapabilities,
    config_options: Vec<acp::SessionConfigOption>,
    auth_methods: Vec<acp::AuthMethod>,
}

impl Default for SessionCoreConfigBuilder {
    fn default() -> Self {
        Self {
            working_dir: PathBuf::from("."),
            workspace_access: WorkspaceAccess::Local,
            workspace_status: None,
            capabilities: AetherCapabilities::default(),
            prompt_capabilities: acp::PromptCapabilities::new(),
            config_options: Vec::new(),
            auth_methods: Vec::new(),
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
        self.workspace_status = Some(workspace_status);
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

    pub fn git_diff(mut self) -> Self {
        self.capabilities.git_diff = true;
        self
    }

    pub fn agent(&self) -> FakeAgent {
        let config = self.build();
        FakeAgent::default()
            .config_options(self.config_options.clone())
            .workspace_status(&config.session_response.session_id.0, config.workspace_status)
    }

    pub fn build(&self) -> SessionCoreConfig {
        let session_capabilities = acp::SessionCapabilities::new()
            .prompt(self.prompt_capabilities.clone())
            .meta(Some(self.capabilities.to_meta()));
        SessionCoreConfig {
            initialize_response: acp::InitializeResponse::new(
                ProtocolVersion::V2,
                acp::Implementation::new("aether", "test"),
            )
            .capabilities(acp::AgentCapabilities::new().session(session_capabilities))
            .auth_methods(self.auth_methods.clone()),
            session_response: acp::NewSessionResponse::new("test-session").config_options(self.config_options.clone()),
            working_dir: self.working_dir.clone(),
            workspace_status: self.workspace_status.clone().unwrap_or_else(|| match self.workspace_access {
                WorkspaceAccess::Local => {
                    WorkspaceStatus { display_dir: "~/code/demo".to_string(), git_ref: Some("main".to_string()) }
                }
                WorkspaceAccess::Remote => initial_workspace_status(&self.working_dir),
            }),
            workspace_access: self.workspace_access,
        }
    }
}
