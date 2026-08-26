pub(crate) mod platform;
pub mod session_config_view;
pub(crate) mod session_model;
pub mod terminal;
pub mod workspace_status;

use crate::error::AppError;
use crate::session::workspace_status::WorkspaceStatus;
use acp_utils::client::{AcpClientError, AcpClientHandle, AcpEvent, TokioAcpAgent, connect_acp_client};
use agent_client_protocol::schema::ProtocolVersion;
use agent_client_protocol::schema::v1::{
    AuthMethod, ClientCapabilities, ElicitationCapabilities, ElicitationFormCapabilities, ElicitationUrlCapabilities,
    Implementation, InitializeRequest, NewSessionRequest, PromptCapabilities, SessionCapabilities, SessionConfigOption,
    SessionId,
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
    pub client_handle: AcpClientHandle,
    pub working_dir: PathBuf,
    pub workspace_status: WorkspaceStatus,
}

impl Session {
    pub async fn connect(agent_command: &str) -> Result<Self, AppError> {
        let working_dir = current_dir().unwrap_or_else(|_| PathBuf::from("."));
        let workspace_status = WorkspaceStatus::initial(&working_dir);
        let agent = TokioAcpAgent::from_str(agent_command).map_err(AcpClientError::InvalidAgentCommand)?;
        let init_request = InitializeRequest::new(ProtocolVersion::LATEST)
            .client_capabilities(client_capabilities())
            .client_info(Implementation::new("wisp", env!("CARGO_PKG_VERSION")));
        let client = connect_acp_client(agent, init_request).await?;
        let session_response = client.handle.new_session(NewSessionRequest::new(working_dir.clone())).await?;

        Ok(Self {
            session_id: session_response.session_id,
            agent_name: client.agent_name(),
            prompt_capabilities: client.prompt_capabilities().clone(),
            session_capabilities: client.session_capabilities().clone(),
            config_options: session_response.config_options.unwrap_or_default(),
            auth_methods: client.auth_methods().to_vec(),
            event_rx: client.event_rx,
            client_handle: client.handle,
            working_dir,
            workspace_status,
        })
    }
}

fn client_capabilities() -> ClientCapabilities {
    ClientCapabilities::new().elicitation(
        ElicitationCapabilities::new()
            .form(ElicitationFormCapabilities::new())
            .url(ElicitationUrlCapabilities::new()),
    )
}
