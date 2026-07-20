pub(crate) mod platform;
pub mod session_config_view;
pub(crate) mod session_loading_buffer;
pub(crate) mod session_model;
pub mod terminal;
pub mod workspace_status;

use crate::error::AppError;
use crate::session::workspace_status::WorkspaceStatus;
use acp_utils::client::{AcpClientError, AcpEvent, AcpPromptHandle, TokioAcpAgent, spawn_acp_session};
use agent_client_protocol::schema::ProtocolVersion;
use agent_client_protocol::schema::v1::{
    AuthMethod, Implementation, InitializeRequest, NewSessionRequest, PromptCapabilities, SessionCapabilities,
    SessionConfigOption, SessionId,
};
use std::env::current_dir;
use std::path::PathBuf;
use std::str::FromStr;
use tokio::sync::mpsc;

pub struct Session {
    pub session_id: SessionId,
    pub agent_name: String,
    pub prompt_capabilities: PromptCapabilities,
    pub session_capabilities: SessionCapabilities,
    pub config_options: Vec<SessionConfigOption>,
    pub auth_methods: Vec<AuthMethod>,
    pub event_rx: mpsc::UnboundedReceiver<AcpEvent>,
    pub prompt_handle: AcpPromptHandle,
    pub working_dir: PathBuf,
    pub workspace_status: WorkspaceStatus,
}

impl Session {
    pub async fn connect(agent_command: &str) -> Result<Self, AppError> {
        let working_dir = current_dir().unwrap_or_else(|_| PathBuf::from("."));
        let workspace_status = WorkspaceStatus::initial(&working_dir);
        let agent = TokioAcpAgent::from_str(agent_command).map_err(AcpClientError::InvalidAgentCommand)?;
        let init_request = InitializeRequest::new(ProtocolVersion::LATEST)
            .client_info(Implementation::new("wisp", env!("CARGO_PKG_VERSION")));
        let acp_session = spawn_acp_session(agent, init_request, NewSessionRequest::new(working_dir.clone())).await?;

        Ok(Self {
            session_id: acp_session.session_id,
            agent_name: acp_session.agent_name,
            prompt_capabilities: acp_session.prompt_capabilities,
            session_capabilities: acp_session.session_capabilities,
            config_options: acp_session.config_options,
            auth_methods: acp_session.auth_methods,
            event_rx: acp_session.event_rx,
            prompt_handle: acp_session.prompt_handle,
            working_dir,
            workspace_status,
        })
    }
}
