use agent_client_protocol::schema::v1::{SessionId, SessionUpdate};
use std::collections::HashMap;

#[derive(Default)]
pub struct SessionLoadingBuffer {
    pending: HashMap<SessionId, Vec<SessionUpdate>>,
}

impl SessionLoadingBuffer {
    pub fn begin_load(&mut self, session_id: SessionId) {
        self.pending.insert(session_id, Vec::new());
    }

    pub fn push(&mut self, session_id: &SessionId, update: SessionUpdate) -> Option<SessionUpdate> {
        match self.pending.get_mut(session_id) {
            Some(queue) => {
                queue.push(update);
                None
            }
            None => Some(update),
        }
    }

    pub fn take(&mut self, session_id: &SessionId) -> Vec<SessionUpdate> {
        self.pending.remove(session_id).unwrap_or_default()
    }

    pub fn clear(&mut self) {
        self.pending.clear();
    }
}
