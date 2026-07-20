use crate::error::AppError;
use crate::settings::UiSettings;
use crate::workspace_status::WorkspaceStatus;
use acp_utils::client::{AcpClientError, AcpEvent, AcpPromptHandle, TokioAcpAgent, spawn_acp_session};
use agent_client_protocol::schema::{Implementation, InitializeRequest, NewSessionRequest, ProtocolVersion, SessionId};
use std::env::current_dir;
use std::path::PathBuf;
use std::str::FromStr;
use tokio::sync::mpsc;

pub struct Session {
    pub session_id: SessionId,
    pub agent_name: String,
    pub settings: UiSettings,
    pub event_rx: mpsc::UnboundedReceiver<AcpEvent>,
    pub prompt_handle: AcpPromptHandle,
    pub working_dir: PathBuf,
    pub workspace_status: WorkspaceStatus,
}

impl Session {
    pub async fn connect(agent_command: &str, settings: UiSettings) -> Result<Self, AppError> {
        let working_dir = current_dir().unwrap_or_else(|_| PathBuf::from("."));
        let workspace_status = WorkspaceStatus::resolve(&working_dir);
        let agent = TokioAcpAgent::from_str(agent_command).map_err(AcpClientError::InvalidAgentCommand)?;
        let init_request = InitializeRequest::new(ProtocolVersion::LATEST)
            .client_info(Implementation::new("wisp-next", env!("CARGO_PKG_VERSION")));
        let acp_session = spawn_acp_session(agent, init_request, NewSessionRequest::new(working_dir.clone())).await?;

        Ok(Self {
            session_id: acp_session.session_id,
            agent_name: acp_session.agent_name,
            settings,
            event_rx: acp_session.event_rx,
            prompt_handle: acp_session.prompt_handle,
            working_dir,
            workspace_status,
        })
    }
}
