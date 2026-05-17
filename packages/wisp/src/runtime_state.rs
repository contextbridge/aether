use crate::cli::Cli;
use crate::error::AppError;
use crate::settings::load_or_create_settings;
use crate::workspace_status::WorkspaceStatus;
use acp_utils::client::{AcpEvent, AcpPromptHandle, spawn_acp_session};
use agent_client_protocol::schema::{
    AuthMethod, Implementation, InitializeRequest, NewSessionRequest, PromptCapabilities, ProtocolVersion,
    SessionConfigOption, SessionId,
};
use std::env::current_dir;
use tokio::sync::mpsc;
use tui::Theme;

#[doc = include_str!("docs/runtime_state.md")]
pub struct RuntimeState {
    pub session_id: SessionId,
    pub agent_name: String,
    pub prompt_capabilities: PromptCapabilities,
    pub config_options: Vec<SessionConfigOption>,
    pub auth_methods: Vec<AuthMethod>,
    pub theme: Theme,
    pub event_rx: mpsc::UnboundedReceiver<AcpEvent>,
    pub prompt_handle: AcpPromptHandle,
    pub working_dir: std::path::PathBuf,
    pub workspace_status: WorkspaceStatus,
}

impl RuntimeState {
    pub async fn new(agent_command: &str) -> Result<Self, AppError> {
        let cwd = current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
        let workspace_status = WorkspaceStatus::resolve(&cwd).await;
        let new_session_request = NewSessionRequest::new(cwd.clone());
        let init_request = InitializeRequest::new(ProtocolVersion::LATEST)
            .client_info(Implementation::new("wisp", env!("CARGO_PKG_VERSION")));

        let session =
            spawn_acp_session(agent_command, init_request, new_session_request).await.map_err(AppError::Acp)?;

        let settings = load_or_create_settings();

        Ok(Self {
            session_id: session.session_id,
            agent_name: session.agent_name,
            prompt_capabilities: session.prompt_capabilities,
            config_options: session.config_options,
            auth_methods: session.auth_methods,
            theme: crate::settings::load_theme(&settings),
            event_rx: session.event_rx,
            prompt_handle: session.prompt_handle,
            working_dir: cwd,
            workspace_status,
        })
    }

    pub async fn from_cli(cli: &Cli) -> Result<Self, AppError> {
        Self::new(&cli.agent).await
    }
}
