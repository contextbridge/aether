use super::connection::McpConnectAttempt;
use std::collections::HashMap;
use tokio::task::{AbortHandle, JoinError, JoinSet};

#[derive(Default)]
pub struct McpConnectionAttemptManager {
    set: JoinSet<McpConnectAttempt>,
    by_server: HashMap<String, AbortHandle>,
}

impl McpConnectionAttemptManager {
    /// Spawn a fresh auth task for `server`, aborting any in-flight task for
    /// the same server so the user can retry a stuck OAuth flow.
    pub fn spawn(&mut self, server: String, fut: impl Future<Output = McpConnectAttempt> + Send + 'static) {
        if let Some(prior) = self.by_server.remove(&server) {
            prior.abort();
        }
        let handle = self.set.spawn(fut);
        self.by_server.insert(server, handle);
    }

    pub fn is_empty(&self) -> bool {
        self.set.is_empty()
    }

    pub async fn join_next(&mut self) -> Option<Result<McpConnectAttempt, JoinError>> {
        let joined = self.set.join_next().await;
        if let Some(Ok(attempt)) = &joined {
            self.by_server.remove(&attempt.name);
        }
        joined
    }

    pub async fn shutdown(&mut self) {
        self.set.abort_all();
        while self.set.join_next().await.is_some() {}
        self.by_server.clear();
    }
}
