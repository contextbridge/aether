use crate::error::ClientError;
use crate::workspace_status::{WorkspaceAccess, WorkspaceStatus, initial_workspace_status};
use acp_utils::client::{AcpClient, AcpClientError, connect_acp_client, initialize_request};
use acp_utils::notifications::{RemoteServerInfo, SessionPreviewParams};
use agent_client_protocol::schema::v2::{
    Implementation, NewSessionRequest, NewSessionResponse, ReplayFrom, ReplayFromStart, ResumeSessionRequest, SessionId,
};
use agent_client_protocol::{AcpAgent, Client, ConnectTo};
use std::env::current_dir;
use std::path::{Path, PathBuf};
use std::str::FromStr;

/// An initialized agent connection with an open session.
pub struct Session {
    pub client: AcpClient,
    pub response: NewSessionResponse,
    pub working_dir: PathBuf,
    pub workspace_status: WorkspaceStatus,
    pub workspace_access: WorkspaceAccess,
}

/// An initialized local agent connection that has not opened a session yet.
///
/// Hosts resuming a saved thread start here, so the agent never creates a
/// throwaway session before the resume request replaces it.
pub struct Connection {
    pub client: AcpClient,
    pub working_dir: PathBuf,
}

impl Session {
    /// Initialize against a remote host, resuming its live session unless explicitly overridden.
    pub async fn connect_remote_to(
        client_info: Implementation,
        transport: impl ConnectTo<Client> + 'static,
        requested_session: Option<SessionId>,
    ) -> Result<Self, ClientError> {
        let client = connect_acp_client(transport, initialize_request(client_info)).await?;
        let remote = RemoteServerInfo::from_meta(client.initialize_response.meta.as_ref())
            .ok_or(ClientError::MissingRemoteContract)?;
        let (selected, working_dir) = match (requested_session, remote.session_id) {
            (Some(requested), live) if live.as_ref() != Some(&requested) => {
                let cwd = client.handle.request(SessionPreviewParams { session_id: requested.to_string() }).await?.cwd;
                (Some(requested), cwd)
            }
            (requested, live) => (requested.or(live), remote.cwd),
        };
        let response = match selected {
            Some(id) => resume(&client, id, &working_dir).await?,
            None => client.handle.request(NewSessionRequest::new(working_dir.clone())).await?,
        };
        let workspace_status = initial_workspace_status(&working_dir);
        Ok(Self { client, response, working_dir, workspace_status, workspace_access: WorkspaceAccess::Remote })
    }

    /// Spawn `agent_command` in the current directory and open a new session.
    pub async fn connect(client_info: Implementation, agent_command: &str) -> Result<Self, ClientError> {
        let working_dir = current_dir().unwrap_or_else(|_| PathBuf::from("."));
        let agent = AcpAgent::from_str(agent_command).map_err(AcpClientError::InvalidAgentCommand)?;
        Self::connect_to(client_info, agent, working_dir).await
    }

    pub async fn connect_to(
        client_info: Implementation,
        agent: impl ConnectTo<Client> + 'static,
        working_dir: PathBuf,
    ) -> Result<Self, ClientError> {
        Connection::connect_to(client_info, agent, working_dir).await?.new_session().await
    }
}

impl Connection {
    /// Initialize the agent connection only; no session is created.
    pub async fn connect_to(
        client_info: Implementation,
        agent: impl ConnectTo<Client> + 'static,
        working_dir: PathBuf,
    ) -> Result<Self, ClientError> {
        let client = connect_acp_client(agent, initialize_request(client_info)).await?;
        Ok(Self { client, working_dir })
    }

    /// Open a new session in the connection's working directory.
    pub async fn new_session(self) -> Result<Session, ClientError> {
        let Self { client, working_dir } = self;
        let workspace_status = initial_workspace_status(&working_dir);
        let response = client.handle.request(NewSessionRequest::new(working_dir.clone())).await?;
        Ok(Session { client, response, working_dir, workspace_status, workspace_access: WorkspaceAccess::Local })
    }

    /// Resume the saved session `session_id` in the connection's working directory.
    ///
    /// The agent replays the session's history from the start as session updates. They wait on the
    /// client's event channel, ahead of any later event, until the host reduces them into the
    /// conversation of the returned session.
    pub async fn resume_session(self, session_id: SessionId) -> Result<Session, ClientError> {
        let Self { client, working_dir } = self;
        let workspace_status = initial_workspace_status(&working_dir);
        let response = resume(&client, session_id, &working_dir).await?;
        Ok(Session { client, response, working_dir, workspace_status, workspace_access: WorkspaceAccess::Local })
    }
}

/// Resumes `session_id` with its history replayed from the start, answering as a new session
/// would so that hosts start a resumed session exactly like a fresh one.
async fn resume(
    client: &AcpClient,
    session_id: SessionId,
    working_dir: &Path,
) -> Result<NewSessionResponse, ClientError> {
    let request = ResumeSessionRequest::new(session_id.clone(), working_dir.to_path_buf())
        .replay_from(ReplayFrom::Start(ReplayFromStart::new()));
    let resumed = client.handle.request(request).await?;
    Ok(NewSessionResponse::new(session_id).config_options(resumed.config_options))
}
