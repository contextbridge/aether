use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use tokio_util::sync::{CancellationToken, WaitForCancellationFuture};
use uuid::Uuid;

#[derive(Clone, Default)]
pub struct SubAgentSessions {
    live: Arc<Mutex<HashMap<String, CancellationToken>>>,
}

pub struct SubAgentSession {
    id: String,
    cancel: CancellationToken,
    sessions: SubAgentSessions,
}

impl SubAgentSessions {
    pub fn open(&self) -> SubAgentSession {
        let id = Uuid::new_v4().to_string();
        let cancel = CancellationToken::new();
        self.live().insert(id.clone(), cancel.clone());
        SubAgentSession { id, cancel, sessions: self.clone() }
    }

    pub fn cancel(&self, session_id: &str) -> bool {
        self.live().get(session_id).inspect(|cancel| cancel.cancel()).is_some()
    }

    fn live(&self) -> MutexGuard<'_, HashMap<String, CancellationToken>> {
        self.live.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

impl SubAgentSession {
    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn cancelled(&self) -> WaitForCancellationFuture<'_> {
        self.cancel.cancelled()
    }
}

impl Drop for SubAgentSession {
    fn drop(&mut self) {
        self.sessions.live().remove(&self.id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::FutureExt;

    #[test]
    fn cancelling_reaches_only_the_named_live_session() {
        let sessions = SubAgentSessions::default();
        let first = sessions.open();
        let second = sessions.open();

        assert!(sessions.cancel(first.id()));
        assert!(first.cancelled().now_or_never().is_some());
        assert!(second.cancelled().now_or_never().is_none());

        let id = second.id().to_string();
        drop(second);
        assert!(!sessions.cancel(&id), "a finished sub-agent is no longer addressable");
    }
}
