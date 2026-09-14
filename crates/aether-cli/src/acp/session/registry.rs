use agent_client_protocol::schema::v2::SessionId;
use tokio::sync::{Mutex, mpsc};

use super::actor::{SessionCommand, SessionHandle};

/// One live top-level session, not the saved-conversation store.
/// Lifecycle requests join the old actor before registering its replacement.
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

    pub(crate) async fn lookup(&self, session_id: Option<&str>) -> Option<mpsc::Sender<SessionCommand>> {
        let active = self.active.lock().await;
        let (_, handle) = active.as_ref().filter(|(id, _)| session_id.is_none_or(|requested| id == requested))?;
        Some(handle.command_sender())
    }

    pub(crate) async fn stop(&self) {
        let mut active = self.active.lock().await;
        if let Some((_, handle)) = active.as_mut() {
            handle.cancel();
            handle.join().await;
        }
        *active = None;
    }
}
