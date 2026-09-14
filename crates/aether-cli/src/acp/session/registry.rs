use agent_client_protocol::schema::v2::SessionId;
use tokio::sync::{Mutex, mpsc};

use super::actor::{SessionCommand, SessionHandle};

/// One live top-level session, not the saved-conversation store.
/// Lifecycle requests join the old actor before registering its replacement.
pub(crate) struct SessionRegistry {
    active: Mutex<Option<Entry>>,
}

impl SessionRegistry {
    pub(crate) fn new() -> Self {
        Self { active: Mutex::new(None) }
    }

    pub(crate) async fn register(&self, session_id: &SessionId, handle: SessionHandle) {
        let mut active = self.active.lock().await;
        assert!(active.is_none(), "previous session must be joined before registration");
        *active = Some(Entry { session_id: session_id.clone(), handle });
    }

    pub(crate) async fn session_id(&self) -> Option<SessionId> {
        self.active.lock().await.as_ref().map(|entry| entry.session_id.clone())
    }

    pub(crate) async fn lookup(&self, session_id: Option<&str>) -> Option<mpsc::Sender<SessionCommand>> {
        self.active
            .lock()
            .await
            .as_ref()
            .filter(|entry| entry.matches(session_id))
            .map(|entry| entry.handle.command_sender())
    }

    pub(crate) async fn stop(&self) {
        self.stop_matching(None).await;
    }

    pub(crate) async fn stop_matching(&self, session_id: Option<&str>) {
        let mut active = self.active.lock().await;
        if let Some(entry) = active.as_mut().filter(|entry| entry.matches(session_id)) {
            entry.handle.shutdown().await;
            *active = None;
        }
    }
}

struct Entry {
    session_id: SessionId,
    handle: SessionHandle,
}

impl Entry {
    fn matches(&self, id: Option<&str>) -> bool {
        id.is_none_or(|requested| self.session_id.0.as_ref() == requested)
    }
}
