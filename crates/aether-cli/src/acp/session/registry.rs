use agent_client_protocol::schema::v2::{McpServer, SessionId};
use std::path::PathBuf;
use tokio::sync::{Mutex, mpsc};

use super::actor::{SessionCommand, SessionHandle};

#[derive(Clone)]
pub(crate) struct SessionInputs {
    pub cwd: PathBuf,
    pub mcp_servers: Vec<McpServer>,
}

#[derive(Clone)]
pub(crate) struct ActiveSession {
    pub session_id: SessionId,
    pub inputs: SessionInputs,
}

/// One live top-level session, not the saved-conversation store.
/// Lifecycle requests join the old actor before registering its replacement.
pub(crate) struct SessionRegistry {
    active: Mutex<Option<(ActiveSession, SessionHandle)>>,
}

impl SessionRegistry {
    pub(crate) fn new() -> Self {
        Self { active: Mutex::new(None) }
    }

    pub(crate) async fn register(&self, session_id: &SessionId, inputs: SessionInputs, handle: SessionHandle) {
        let mut active = self.active.lock().await;
        assert!(active.is_none(), "previous session must be joined before registration");
        *active = Some((ActiveSession { session_id: session_id.clone(), inputs }, handle));
    }

    pub(crate) async fn active_session(&self) -> Option<ActiveSession> {
        self.active.lock().await.as_ref().map(|(session, _)| session.clone())
    }

    pub(crate) async fn lookup(&self, session_id: Option<&str>) -> Option<mpsc::Sender<SessionCommand>> {
        let active = self.active.lock().await;
        let (_, handle) = active
            .as_ref()
            .filter(|(session, _)| session_id.is_none_or(|requested| session.session_id.0.as_ref() == requested))?;
        Some(handle.command_sender())
    }

    pub(crate) async fn stop(&self) {
        self.stop_matching(None).await;
    }

    pub(crate) async fn stop_matching(&self, session_id: Option<&str>) {
        let mut active = self.active.lock().await;
        if let Some((_, handle)) = active
            .as_mut()
            .filter(|(session, _)| session_id.is_none_or(|requested| session.session_id.0.as_ref() == requested))
        {
            handle.cancel();
            handle.join().await;
            *active = None;
        }
    }
}
