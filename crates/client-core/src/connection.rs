use crate::runtime::SessionRuntime;
use crate::session_core::SessionCoreConfig;
use crate::session_model::{WorkspaceAccess, initial_workspace_status};
use acp_utils::client::{AcpClient, AcpClientError, connect_acp_client, initialize_request};
use acp_utils::notifications::{RemoteServerInfo, SessionPreviewParams};
use agent_client_protocol::schema::v2::{
    Implementation, NewSessionRequest, NewSessionResponse, ReplayFrom, ReplayFromStart, ResumeSessionRequest, SessionId,
};
use agent_client_protocol::{Client, ConnectTo};
use std::path::{Path, PathBuf};

pub struct Connection {
    client: AcpClient,
}

pub struct Session {
    client: AcpClient,
    response: NewSessionResponse,
    working_dir: PathBuf,
    workspace_access: WorkspaceAccess,
}

#[derive(Debug, thiserror::Error)]
pub enum ConnectionError {
    #[error("Server did not advertise the Aether remote-server contract; connect to `aether server`")]
    MissingRemoteContract,
    #[error("{0}")]
    Acp(#[from] AcpClientError),
}

impl Connection {
    pub async fn connect_to(
        client_info: Implementation,
        agent: impl ConnectTo<Client> + 'static,
    ) -> Result<Self, ConnectionError> {
        let client = connect_acp_client(agent, initialize_request(client_info)).await?;
        Ok(Self { client })
    }

    pub async fn open(self, session_id: Option<SessionId>, working_dir: PathBuf) -> Result<Session, ConnectionError> {
        self.open_with_access(session_id, working_dir, WorkspaceAccess::Local).await
    }

    pub async fn open_with_access(
        self,
        session_id: Option<SessionId>,
        working_dir: PathBuf,
        workspace_access: WorkspaceAccess,
    ) -> Result<Session, ConnectionError> {
        if self.remote_server().is_some() {
            return self.open_remote(session_id).await;
        }
        self.start(session_id, working_dir, workspace_access).await
    }

    pub async fn open_remote(self, requested_session: Option<SessionId>) -> Result<Session, ConnectionError> {
        let remote = self.remote_server().ok_or(ConnectionError::MissingRemoteContract)?;
        let (selected, working_dir) = match (requested_session, remote.session_id) {
            (Some(requested), live) if live.as_ref() != Some(&requested) => {
                let preview = SessionPreviewParams { session_id: requested.to_string() };
                (Some(requested), self.client.handle.request(preview).await?.cwd)
            }
            (requested, live) => (requested.or(live), remote.cwd),
        };
        self.start(selected, working_dir, WorkspaceAccess::Remote).await
    }

    fn remote_server(&self) -> Option<RemoteServerInfo> {
        RemoteServerInfo::from_meta(self.client.initialize_response.meta.as_ref())
    }

    async fn start(
        self,
        session_id: Option<SessionId>,
        working_dir: PathBuf,
        workspace_access: WorkspaceAccess,
    ) -> Result<Session, ConnectionError> {
        let Self { client } = self;
        let response = match session_id {
            Some(session_id) => resume(&client, session_id, working_dir.clone()).await?,
            None => client.handle.request(NewSessionRequest::new(working_dir.clone())).await?,
        };
        Ok(Session { client, response, working_dir, workspace_access })
    }
}

impl Session {
    pub fn session_id(&self) -> &SessionId {
        &self.response.session_id
    }

    pub fn working_dir(&self) -> &Path {
        &self.working_dir
    }

    pub fn client(&self) -> &AcpClient {
        &self.client
    }

    pub fn into_core(self) -> (SessionCoreConfig, SessionRuntime) {
        let Self { client, response, working_dir, workspace_access } = self;
        let config = SessionCoreConfig {
            initialize_response: client.initialize_response,
            session_response: response,
            workspace_status: initial_workspace_status(&working_dir),
            working_dir,
            workspace_access,
        };
        (config, SessionRuntime::new(client.handle, client.event_rx))
    }
}

pub(crate) fn resume_request(session_id: SessionId, cwd: PathBuf) -> ResumeSessionRequest {
    ResumeSessionRequest::new(session_id, cwd).replay_from(ReplayFrom::Start(ReplayFromStart::new()))
}

async fn resume(
    client: &AcpClient,
    session_id: SessionId,
    cwd: PathBuf,
) -> Result<NewSessionResponse, ConnectionError> {
    let resumed = client.handle.request(resume_request(session_id.clone(), cwd)).await?;
    Ok(NewSessionResponse::new(session_id).config_options(resumed.config_options))
}
