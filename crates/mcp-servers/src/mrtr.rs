use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use tokio::sync::Mutex;

/// Bound on concurrently pending MRTR continuations per server. When the table
/// is full the oldest entry is evicted; the client observes that as an invalid
/// request-state error on its next retry rather than unbounded memory growth.
const DEFAULT_PENDING_CAPACITY: usize = 64;

/// Bounded in-memory table of pending MRTR continuations for the built-in
/// in-process servers, keyed by an opaque cryptographically random token.
///
/// Each `InputRequiredResult` stores its continuation state here and hands the
/// client only the random token; the retry echoes the token back and the server
/// looks the state up again. This is the simplest safe continuation mechanism
/// for in-process servers. If a server later runs behind replicated HTTP
/// instances, it must move to signed continuation state or shared storage.
#[derive(Clone)]
pub(crate) struct PendingRequestTable<T> {
    inner: Arc<Mutex<Inner<T>>>,
    capacity: usize,
}

struct Inner<T> {
    entries: HashMap<String, T>,
    order: VecDeque<String>,
}

impl<T> PendingRequestTable<T> {
    pub fn new() -> Self {
        Self::with_capacity(DEFAULT_PENDING_CAPACITY)
    }

    pub fn with_capacity(capacity: usize) -> Self {
        // A zero capacity would bypass the eviction path in `insert` and grow
        // without bound; clamp so the table always honors its bound.
        let capacity = capacity.max(1);
        Self { inner: Arc::new(Mutex::new(Inner { entries: HashMap::new(), order: VecDeque::new() })), capacity }
    }

    /// Insert `pending` and return a fresh opaque token. When the table is at
    /// capacity the oldest entry is evicted first.
    pub async fn insert(&self, pending: T) -> String {
        let mut inner = self.inner.lock().await;
        if inner.entries.len() >= self.capacity
            && let Some(oldest) = inner.order.pop_front()
        {
            inner.entries.remove(&oldest);
        }
        let token = new_token();
        inner.entries.insert(token.clone(), pending);
        inner.order.push_back(token.clone());
        token
    }

    /// Remove and return the pending continuation for `token`, if present.
    pub async fn take(&self, token: &str) -> Option<T> {
        let mut inner = self.inner.lock().await;
        let entry = inner.entries.remove(token)?;
        inner.order.retain(|candidate| candidate != token);
        Some(entry)
    }

    #[cfg(test)]
    pub async fn len(&self) -> usize {
        self.inner.lock().await.entries.len()
    }

    #[cfg(test)]
    pub async fn is_empty(&self) -> bool {
        self.len().await == 0
    }
}

impl<T> Default for PendingRequestTable<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T> std::fmt::Debug for PendingRequestTable<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PendingRequestTable").field("capacity", &self.capacity).finish_non_exhaustive()
    }
}

/// Cryptographically random opaque token (UUID v4, backed by the OS CSPRNG).
fn new_token() -> String {
    uuid::Uuid::new_v4().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn insert_and_take_round_trip_state() {
        let table = PendingRequestTable::new();
        let token = table.insert("payload".to_string()).await;
        assert_eq!(table.len().await, 1);
        assert_eq!(table.take(&token).await.as_deref(), Some("payload"));
        assert!(table.is_empty().await);
        assert_eq!(table.take(&token).await, None, "tokens are single-use");
    }

    #[tokio::test]
    async fn tokens_are_opaque_and_distinct() {
        let table = PendingRequestTable::new();
        let first = table.insert(()).await;
        let second = table.insert(()).await;
        assert_ne!(first, second);
        assert!(!first.is_empty() && !second.is_empty());
    }

    #[tokio::test]
    async fn table_evicts_oldest_entry_when_full() {
        let table = PendingRequestTable::with_capacity(2);
        let oldest = table.insert("oldest".to_string()).await;
        let second = table.insert("second".to_string()).await;
        let third = table.insert("third".to_string()).await;

        assert_eq!(table.len().await, 2, "capacity bounds the table");
        assert_eq!(table.take(&oldest).await, None, "oldest entry was evicted");
        assert_eq!(table.take(&second).await.as_deref(), Some("second"));
        assert_eq!(table.take(&third).await.as_deref(), Some("third"));
    }
}
