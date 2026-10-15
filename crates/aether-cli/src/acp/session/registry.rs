use agent_client_protocol::schema::v2::SessionId;
use std::future::Future;
use tokio::sync::{Mutex, MutexGuard, mpsc};

use super::actor::{SessionCommand, SessionHandle};

/// One live top-level session, not the saved-conversation store.
/// Lifecycle requests hold the lock from stopping the old actor until its replacement is registered.
pub(crate) struct SessionRegistry {
    active: Mutex<Option<Entry>>,
}

pub(crate) struct ActiveSession<'a>(MutexGuard<'a, Option<Entry>>);

impl SessionRegistry {
    pub(crate) fn new() -> Self {
        Self { active: Mutex::new(None) }
    }

    pub(crate) async fn lock(&self) -> ActiveSession<'_> {
        ActiveSession(self.active.lock().await)
    }

    pub(crate) async fn session_id(&self) -> Option<SessionId> {
        self.active.lock().await.as_ref().map(|entry| entry.session_id.clone())
    }

    pub(crate) async fn lookup(&self, session_id: Option<&str>) -> Option<mpsc::Sender<SessionCommand>> {
        self.lock().await.lookup(session_id)
    }

    pub(crate) async fn stop(&self) {
        self.stop_matching(None).await;
    }

    pub(crate) async fn stop_matching(&self, session_id: Option<&str>) {
        self.lock().await.stop_matching(session_id).await;
    }
}

impl ActiveSession<'_> {
    pub(crate) fn lookup(&self, session_id: Option<&str>) -> Option<mpsc::Sender<SessionCommand>> {
        self.0.as_ref().filter(|entry| entry.matches(session_id)).map(|entry| entry.handle.command_sender())
    }

    pub(crate) async fn replace<T>(
        &mut self,
        session_id: SessionId,
        start: impl Future<Output = Result<SessionHandle, T>>,
    ) -> Result<mpsc::Sender<SessionCommand>, T> {
        self.stop_matching(None).await;
        let handle = start.await?;
        let commands = handle.command_sender();
        *self.0 = Some(Entry { session_id, handle });
        Ok(commands)
    }

    pub(crate) async fn stop_matching(&mut self, session_id: Option<&str>) {
        if let Some(entry) = self.0.as_mut().filter(|entry| entry.matches(session_id)) {
            entry.handle.shutdown().await;
            *self.0 = None;
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
