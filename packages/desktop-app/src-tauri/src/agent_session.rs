use crate::{AppEvent, bridge_event};
use acp_utils::client::{AcpEvent, AcpPromptHandle, TokioAcpAgent, spawn_acp_session};
use acp_utils::notifications::{ElicitationAction, ElicitationResponse};
use agent_client_protocol::schema::ProtocolVersion;
use agent_client_protocol::schema::v1::{
    Implementation, InitializeRequest, NewSessionRequest, SessionConfigOption, SessionId,
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
    pub(crate) prompt_handle: AcpPromptHandle,
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

        let acp_session = spawn_acp_session(agent, init, NewSessionRequest::new(cwd.clone()))
            .await
            .map_err(|error| error.to_string())?;

        let connection_id = Uuid::new_v4().to_string();
        let session_id = acp_session.session_id.clone();
        let event_connection_id = connection_id.clone();
        let event_session_id = session_id.clone();
        let (ended_tx, ended_rx) = watch::channel(false);
        let event_task =
            spawn_event_task(acp_session.event_rx, event_session_id, event_connection_id, events, ended_tx);

        Ok((
            Self {
                connection_id,
                session_id,
                agent_name: acp_session.agent_name,
                config_options: acp_session.config_options,
                prompt_handle: acp_session.prompt_handle,
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
) -> JoinHandle<()> {
    tokio::spawn(async move {
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
                    let _ = responder.respond(ElicitationResponse { action: ElicitationAction::Cancel, content: None });
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
