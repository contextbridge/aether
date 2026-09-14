pub(crate) mod platform;
pub mod session_config_view;
pub(crate) mod session_model;
pub mod terminal;
pub mod workspace_status;

use crate::error::AppError;
use crate::session::workspace_status::WorkspaceStatus;
use acp_utils::agent::TokioAcpAgent;
use acp_utils::client::{AcpClient, AcpClientError, connect_acp_client};
use acp_utils::notifications::{RemoteServerInfo, SessionPreviewParams};
use agent_client_protocol::schema::ProtocolVersion;
use agent_client_protocol::schema::v2::{
    ClientCapabilities, ElicitationCapabilities, ElicitationFormCapabilities, ElicitationUrlCapabilities,
    Implementation, InitializeRequest, NewSessionRequest, NewSessionResponse, ResumeSessionRequest, SessionId,
};
use agent_client_protocol::{Client, ConnectTo};
use std::env::current_dir;
use std::path::PathBuf;
use std::str::FromStr;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum WorkspaceAccess {
    #[default]
    Local,
    Remote,
}

impl WorkspaceAccess {
    pub fn display_path(self, path: &std::path::Path) -> String {
        match self {
            Self::Local => workspace_status::home_relative_path(path),
            Self::Remote => format!("remote: {}", path.display()),
        }
    }
}

pub struct Session {
    pub client: AcpClient,
    pub response: NewSessionResponse,
    pub working_dir: PathBuf,
    pub workspace_status: WorkspaceStatus,
    pub workspace_access: WorkspaceAccess,
}

impl Session {
    /// Initialize against a remote host, resuming its live session unless explicitly overridden.
    pub async fn connect_remote_to(
        transport: impl ConnectTo<Client> + 'static,
        requested_session: Option<SessionId>,
    ) -> Result<Self, AppError> {
        let init_request =
            InitializeRequest::new(ProtocolVersion::V2, Implementation::new("wisp", env!("CARGO_PKG_VERSION")))
                .capabilities(client_capabilities());
        let client = connect_acp_client(transport, init_request).await?;
        let remote = RemoteServerInfo::from_meta(client.initialize_response.meta.as_ref())
            .ok_or(AppError::MissingRemoteContract)?;
        let selected = requested_session.or_else(|| remote.session_id.clone());
        let working_dir = if let Some(id) = selected.as_ref().filter(|id| Some(*id) != remote.session_id.as_ref()) {
            client.handle.request(SessionPreviewParams { session_id: id.to_string() }).await?.cwd
        } else {
            remote.cwd
        };
        let response = if let Some(id) = selected {
            let resumed = client.handle
                .resume_session_with_replay(ResumeSessionRequest::new(id.clone(), working_dir.clone()))
                .await?;
            NewSessionResponse::new(id).config_options(resumed.config_options)
        } else {
            client.handle.new_session(NewSessionRequest::new(working_dir.clone())).await?
        };
        let workspace_status = WorkspaceStatus::remote(&working_dir);
        Ok(Self { client, response, working_dir, workspace_status, workspace_access: WorkspaceAccess::Remote })
    }

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
            workspace_access: WorkspaceAccess::Local,
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
