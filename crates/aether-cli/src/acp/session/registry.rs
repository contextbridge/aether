use agent_client_protocol::schema::v2::SessionId;
use tokio::sync::{Mutex, mpsc};

use super::actor::{ConfigSnapshot, SessionCommand, SessionHandle};

/// One live top-level session, not the saved-conversation store.
/// `AcpState` serializes lifecycle operations and joins the old actor before registration.
pub(crate) struct SessionRegistry {
    active: Mutex<Option<(String, SessionHandle)>>,
}

impl SessionRegistry {
    pub(crate) fn new() -> Self {
        Self { active: Mutex::new(None) }
    }

    pub(crate) async fn register(&self, session_id: &SessionId, handle: SessionHandle) {
        let mut active = self.active.lock().await;
        assert!(active.is_none(), "previous session must be joined before registration");
        *active = Some((session_id.0.to_string(), handle));
    }

    pub(crate) async fn lookup(&self, session_id: &str) -> Option<mpsc::Sender<SessionCommand>> {
        let active = self.active.lock().await;
        let (_, handle) = active.as_ref().filter(|(id, _)| id == session_id)?;
        Some(handle.command_sender())
    }

    pub(crate) async fn lookup_with_snapshot(
        &self,
        session_id: &str,
    ) -> Option<(mpsc::Sender<SessionCommand>, ConfigSnapshot)> {
        let active = self.active.lock().await;
        let (_, handle) = active.as_ref().filter(|(id, _)| id == session_id)?;
        Some((handle.command_sender(), handle.config_snapshot()))
    }

    pub(crate) async fn stop(&self) {
        let active = self.active.lock().await.take();
        if let Some((_, handle)) = active {
            handle.cancel();
            handle.join().await;
        }
    }

    pub(crate) async fn config_snapshot(&self) -> Option<(String, ConfigSnapshot)> {
        self.active.lock().await.as_ref().map(|(id, handle)| (id.clone(), handle.config_snapshot()))
    }
}
