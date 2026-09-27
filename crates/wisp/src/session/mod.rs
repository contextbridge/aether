pub(crate) mod platform;
pub mod session_config_view;
pub(crate) mod session_model;
pub mod terminal;
pub mod workspace_status;

use crate::error::AppError;
use crate::session::workspace_status::WorkspaceStatus;
use acp_utils::client::{AcpClient, AcpClientError, connect_acp_client, initialize_request};
use acp_utils::notifications::{RemoteServerInfo, SessionPreviewParams};
use agent_client_protocol::schema::v2::{
    Implementation, NewSessionRequest, NewSessionResponse, ReplayFrom, ReplayFromStart, ResumeSessionRequest,
    SessionId,
};
use agent_client_protocol::{AcpAgent, Client, ConnectTo};
use std::env::current_dir;
use std::path::PathBuf;
use std::str::FromStr;

pub use workspace_status::WorkspaceAccess;

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
        let client = connect_acp_client(transport, initialize_request(client_info())).await?;
        let remote = RemoteServerInfo::from_meta(client.initialize_response.meta.as_ref())
            .ok_or(AppError::MissingRemoteContract)?;
        let (selected, working_dir) = match (requested_session, remote.session_id) {
            (Some(requested), live) if live.as_ref() != Some(&requested) => {
                let cwd = client.handle.request(SessionPreviewParams { session_id: requested.to_string() }).await?.cwd;
                (Some(requested), cwd)
            }
            (requested, live) => (requested.or(live), remote.cwd),
        };
        let response = if let Some(id) = selected {
            let resumed = client.handle
                .resume_session(
                    ResumeSessionRequest::new(id.clone(), working_dir.clone())
                        .replay_from(ReplayFrom::Start(ReplayFromStart::new())),
                )
                .await?;
            NewSessionResponse::new(id).config_options(resumed.config_options)
        } else {
            client.handle.new_session(NewSessionRequest::new(working_dir.clone())).await?
        };
        let workspace_status = WorkspaceStatus::initial(&working_dir);
        Ok(Self { client, response, working_dir, workspace_status, workspace_access: WorkspaceAccess::Remote })
    }

    pub async fn connect(agent_command: &str) -> Result<Self, AppError> {
        let working_dir = current_dir().unwrap_or_else(|_| PathBuf::from("."));
        let agent = AcpAgent::from_str(agent_command).map_err(AcpClientError::InvalidAgentCommand)?;
        Self::connect_to(agent, working_dir).await
    }

    pub async fn connect_to(agent: impl ConnectTo<Client> + 'static, working_dir: PathBuf) -> Result<Self, AppError> {
        let workspace_status = WorkspaceStatus::initial(&working_dir);
        let client = connect_acp_client(agent, initialize_request(client_info())).await?;
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

fn client_info() -> Implementation {
    Implementation::new("wisp", env!("CARGO_PKG_VERSION"))
}
