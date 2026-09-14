pub(crate) mod platform;
pub mod session_config_view;
pub(crate) mod session_model;
pub mod terminal;
pub mod workspace_status;

use crate::error::AppError;
use crate::session::workspace_status::WorkspaceStatus;
use acp_utils::agent::TokioAcpAgent;
use acp_utils::client::{AcpClient, AcpClientError, connect_acp_client};
use agent_client_protocol::schema::ProtocolVersion;
use agent_client_protocol::schema::v2::{
    ClientCapabilities, ElicitationCapabilities, ElicitationFormCapabilities, ElicitationUrlCapabilities,
    Implementation, InitializeRequest, NewSessionRequest, NewSessionResponse,
};
use agent_client_protocol::{Client, ConnectTo};
use std::env::current_dir;
use std::path::PathBuf;
use std::str::FromStr;

pub struct Session {
    pub client: AcpClient,
    pub response: NewSessionResponse,
    pub working_dir: PathBuf,
    pub workspace_status: WorkspaceStatus,
}

impl Session {
    pub async fn connect(agent_command: &str) -> Result<Self, AppError> {
        let working_dir = current_dir().unwrap_or_else(|_| PathBuf::from("."));
        let agent = TokioAcpAgent::from_str(agent_command).map_err(AcpClientError::InvalidAgentCommand)?;
        Self::connect_to(agent, working_dir).await
    }

    pub async fn connect_to(agent: impl ConnectTo<Client> + 'static, working_dir: PathBuf) -> Result<Self, AppError> {
        let workspace_status = WorkspaceStatus::initial(&working_dir);
        let init_request =
            InitializeRequest::new(ProtocolVersion::V2, Implementation::new("wisp", env!("CARGO_PKG_VERSION")))
                .capabilities(client_capabilities());
        let client = connect_acp_client(agent, init_request).await?;
        let session_response = client.handle.new_session(NewSessionRequest::new(working_dir.clone())).await?;

        Ok(Self {
            client,
            response: session_response,
            working_dir,
            workspace_status,
        })
    }
}

fn client_capabilities() -> ClientCapabilities {
    ClientCapabilities::new().elicitation(
        ElicitationCapabilities::new().form(ElicitationFormCapabilities::new()).url(ElicitationUrlCapabilities::new()),
    )
}
