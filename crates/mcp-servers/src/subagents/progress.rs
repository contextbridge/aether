use aether_core::events::SubAgentProgressPayload;
use rmcp::model::{ProgressNotificationParam, ProgressToken};
use rmcp::{Peer, RoleServer};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

#[derive(Clone)]
pub struct SubAgentProgressSink {
    peer: Peer<RoleServer>,
    token: ProgressToken,
    counter: Arc<AtomicU64>,
}

impl SubAgentProgressSink {
    pub fn new(peer: Peer<RoleServer>, token: ProgressToken) -> Self {
        Self { peer, token, counter: Arc::new(AtomicU64::new(0)) }
    }

    pub async fn send(&self, payload: SubAgentProgressPayload) {
        let message = serde_json::to_string(&payload).unwrap_or_default();
        #[allow(clippy::cast_precision_loss)]
        let progress = self.counter.fetch_add(1, Ordering::Relaxed) as f64;
        let notification = ProgressNotificationParam::new(self.token.clone(), progress).with_message(message);
        let _ = self.peer.notify_progress(notification).await;
    }
}
