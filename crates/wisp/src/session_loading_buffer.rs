use agent_client_protocol::schema::{SessionId, SessionUpdate};
use std::collections::HashMap;

#[derive(Default)]
pub(crate) struct SessionLoadingBuffer {
    pending: HashMap<SessionId, Vec<SessionUpdate>>,
}

impl SessionLoadingBuffer {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    pub(crate) fn begin_load(&mut self, session_id: SessionId) {
        self.pending.insert(session_id, Vec::new());
    }

    pub(crate) fn push(&mut self, session_id: &SessionId, update: SessionUpdate) -> Option<SessionUpdate> {
        match self.pending.get_mut(session_id) {
            Some(queue) => {
                queue.push(update);
                None
            }
            None => Some(update),
        }
    }

    pub(crate) fn take(&mut self, session_id: &SessionId) -> Vec<SessionUpdate> {
        self.pending.remove(session_id).unwrap_or_default()
    }

    pub(crate) fn remove(&mut self, session_id: &SessionId) {
        self.pending.remove(session_id);
    }

    pub(crate) fn clear(&mut self) {
        self.pending.clear();
    }
}
