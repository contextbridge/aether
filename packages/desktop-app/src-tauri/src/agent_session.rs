use crate::{AppEvent, bridge_event};
use acp_utils::client::{AcpClientHandle, AcpEvent, TokioAcpAgent, connect_acp_client};
use agent_client_protocol::schema::ProtocolVersion;
use agent_client_protocol::schema::v1::{
    CreateElicitationResponse, ElicitationAction, Implementation, InitializeRequest, LoadSessionRequest,
    NewSessionRequest, SessionConfigOption, SessionId,
};
use std::path::PathBuf;
use tauri::ipc::Channel;
use tokio::sync::{mpsc, watch};
use tokio::task::JoinHandle;
use uuid::Uuid;

pub(crate) struct AgentSession {
    pub(crate) connection_id: String,
    pub(crate) session_id: SessionId,
    pub(crate) agent_name: String,
    pub(crate) config_options: Vec<SessionConfigOption>,
    pub(crate) client_handle: AcpClientHandle,
    pub(crate) cwd: PathBuf,
    pub(crate) event_task: JoinHandle<()>,
}

impl AgentSession {
    pub(crate) async fn spawn(
        program: String,
        args: Vec<String>,
        cwd: PathBuf,
        events: Channel<AppEvent>,
    ) -> Result<(Self, watch::Receiver<bool>), String> {
        let agent = TokioAcpAgent::from_command(program, args);
        let init = InitializeRequest::new(ProtocolVersion::LATEST)
            .client_info(Implementation::new("aether-desktop", env!("CARGO_PKG_VERSION")));

        let client = connect_acp_client(agent, init).await.map_err(|error| error.to_string())?;
        let new_session =
            client.handle.new_session(NewSessionRequest::new(cwd.clone())).await.map_err(|error| error.to_string())?;

        let connection_id = Uuid::new_v4().to_string();
        let session_id = new_session.session_id.clone();
        let event_connection_id = connection_id.clone();
        let event_session_id = session_id.clone();
        let (ended_tx, ended_rx) = watch::channel(false);
        let agent_name = client.agent_name();
        let client_handle = client.handle;
        let event_rx = client.event_rx;
        let event_task = spawn_event_task(event_rx, event_session_id, event_connection_id, events, ended_tx, None);

        Ok((
            Self {
                connection_id,
                session_id,
                agent_name,
                config_options: new_session.config_options.unwrap_or_default(),
                client_handle,
                cwd,
                event_task,
            },
            ended_rx,
        ))
    }

    pub(crate) async fn spawn_loaded(
        program: String,
        args: Vec<String>,
        session_id: SessionId,
        cwd: PathBuf,
        events: Channel<AppEvent>,
    ) -> Result<(Self, watch::Receiver<bool>), String> {
        let agent = TokioAcpAgent::from_command(program, args);
        let init = InitializeRequest::new(ProtocolVersion::LATEST)
            .client_info(Implementation::new("aether-desktop", env!("CARGO_PKG_VERSION")));
        let client = connect_acp_client(agent, init).await.map_err(|error| error.to_string())?;
        let request = LoadSessionRequest::new(session_id, cwd.clone());
        let loaded = client.handle.load_session(request).await.map_err(|error| error.to_string())?;

        let connection_id = Uuid::new_v4().to_string();
        let session_id = loaded.session_id.clone();
        let (ended_tx, ended_rx) = watch::channel(false);
        let agent_name = client.agent_name();
        let client_handle = client.handle;
        let event_rx = client.event_rx;
        let event_task = spawn_event_task(
            event_rx,
            session_id.clone(),
            connection_id.clone(),
            events,
            ended_tx,
            Some(loaded.replay),
        );

        Ok((
            Self {
                connection_id,
                session_id,
                agent_name,
                config_options: loaded.response.config_options.unwrap_or_default(),
                client_handle,
                cwd,
                event_task,
            },
            ended_rx,
        ))
    }
}

impl Drop for AgentSession {
    fn drop(&mut self) {
        self.event_task.abort();
    }
}

fn spawn_event_task(
    mut event_rx: mpsc::UnboundedReceiver<AcpEvent>,
    session_id: SessionId,
    connection_id: String,
    events: Channel<AppEvent>,
    ended_tx: watch::Sender<bool>,
    replay: Option<Vec<agent_client_protocol::schema::v1::SessionNotification>>,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        if let Some(replay) = replay {
            for notification in replay {
                let event = AcpEvent::SessionUpdate {
                    session_id: notification.session_id,
                    update: Box::new(notification.update),
                };
                if let Some(output) = bridge_event(session_id.0.to_string(), connection_id.clone(), event)
                    && events.send(output).is_err()
                {
                    let _ = ended_tx.send(true);
                    return;
                }
            }
            if events
                .send(AppEvent::HistoryLoaded {
                    session_id: session_id.0.to_string(),
                    connection_id: connection_id.clone(),
                })
                .is_err()
            {
                let _ = ended_tx.send(true);
                return;
            }
        }

        loop {
            let Some(event) = event_rx.recv().await else {
                break;
            };
            match event {
                AcpEvent::ConnectionClosed => {
                    let _ = events.send(AppEvent::ConnectionClosed {
                        session_id: session_id.0.to_string(),
                        connection_id: connection_id.clone(),
                        error: None,
                    });
                    break;
                }
                AcpEvent::ElicitationRequest { responder, .. } => {
                    let _ = responder.respond(CreateElicitationResponse::new(ElicitationAction::Cancel));
                }
                event => {
                    if let Some(output) = bridge_event(session_id.0.to_string(), connection_id.clone(), event)
                        && events.send(output).is_err()
                    {
                        break;
                    }
                }
            }
        }
        let _ = ended_tx.send(true);
    })
}
