use agent_client_protocol::schema::v1::SessionId;
use std::collections::HashMap;
use tokio::sync::{Mutex, mpsc};

use super::actor::{ConfigSnapshot, SessionCommand, SessionHandle};

pub(crate) struct SessionRegistry {
    sessions: Mutex<HashMap<String, SessionHandle>>,
}

impl SessionRegistry {
    pub(crate) fn new() -> Self {
        Self { sessions: Mutex::new(HashMap::new()) }
    }

    pub(crate) async fn register(&self, session_id: &SessionId, handle: SessionHandle) {
        if let Some(old) = self.sessions.lock().await.insert(session_id.0.to_string(), handle) {
            old.cancel();
        }
    }

    pub(crate) async fn lookup(&self, session_id: &str) -> Option<(mpsc::Sender<SessionCommand>, ConfigSnapshot)> {
        let sessions = self.sessions.lock().await;
        let handle = sessions.get(session_id)?;
        Some((handle.command_sender(), handle.config_snapshot()))
    }

    pub(crate) async fn shutdown_all(&self) {
        let handles: Vec<SessionHandle> = self.sessions.lock().await.drain().map(|(_, handle)| handle).collect();
        for handle in &handles {
            handle.cancel();
        }
        futures::future::join_all(handles.into_iter().map(SessionHandle::join)).await;
    }

    pub(crate) async fn config_snapshots(&self) -> Vec<(String, ConfigSnapshot)> {
        let sessions = self.sessions.lock().await;
        sessions.iter().map(|(id, handle)| (id.clone(), handle.config_snapshot())).collect()
    }
}
