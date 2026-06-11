use crate::cli::Cli;
use crate::error::AppError;
use crate::settings::{StatusLineSettings, WispSettings};
use crate::workspace::WorkspaceRuntime;
use acp_utils::client::{AcpEvent, AcpPromptHandle, spawn_acp_session};
use agent_client_protocol::schema::{
    AuthMethod, Implementation, InitializeRequest, NewSessionRequest, PromptCapabilities, ProtocolVersion,
    SessionCapabilities, SessionConfigOption, SessionId,
};
use std::env::current_dir;
use tokio::sync::mpsc;
use tui::Theme;

#[doc = include_str!("docs/runtime_state.md")]
pub struct RuntimeState {
    pub session_id: SessionId,
    pub agent_name: String,
    pub prompt_capabilities: PromptCapabilities,
    pub session_capabilities: SessionCapabilities,
    pub config_options: Vec<SessionConfigOption>,
    pub auth_methods: Vec<AuthMethod>,
    pub theme: Theme,
    pub settings: WispSettings,
    pub event_rx: mpsc::UnboundedReceiver<AcpEvent>,
    pub prompt_handle: AcpPromptHandle,
    pub workspace: WorkspaceRuntime,
}

impl RuntimeState {
    pub async fn new(agent_command: &str, settings: WispSettings) -> Result<Self, AppError> {
        let cwd = current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
        let workspace = WorkspaceRuntime::resolve(cwd.clone()).await;
        let new_session_request = NewSessionRequest::new(cwd.clone());
        let init_request = InitializeRequest::new(ProtocolVersion::LATEST)
            .client_info(Implementation::new("wisp", env!("CARGO_PKG_VERSION")));

        let session =
            spawn_acp_session(agent_command, init_request, new_session_request).await.map_err(AppError::Acp)?;

        let theme = crate::settings::load_theme(&settings);

        Ok(Self {
            session_id: session.session_id,
            agent_name: session.agent_name,
            prompt_capabilities: session.prompt_capabilities,
            session_capabilities: session.session_capabilities,
            config_options: session.config_options,
            auth_methods: session.auth_methods,
            theme,
            settings,
            event_rx: session.event_rx,
            prompt_handle: session.prompt_handle,
            workspace,
        })
    }

    pub async fn from_cli(cli: &Cli) -> Result<Self, AppError> {
        Self::new(&cli.agent, crate::settings::load_or_create_settings(StatusLineSettings::defaults())).await
    }
}
