use lsp_types::{PublishDiagnosticsParams, Uri};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, PoisonError, RwLock};
use std::time::Duration;
use tokio::sync::Notify;

const DIAGNOSTICS_SETTLE_DURATION: Duration = Duration::from_millis(600);

#[derive(Clone, Default)]
pub(crate) struct DiagnosticsStore {
    state: Arc<RwLock<HashMap<Uri, (PublishDiagnosticsParams, u64)>>>,
    notify: Arc<Notify>,
    version: Arc<AtomicU64>,
}

impl DiagnosticsStore {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    pub(crate) fn publish(&self, diagnostics: PublishDiagnosticsParams) {
        let uri = diagnostics.uri.clone();
        let version = self.version.fetch_add(1, Ordering::Relaxed) + 1;
        self.state.write().unwrap_or_else(PoisonError::into_inner).insert(uri, (diagnostics, version));
        self.notify.notify_waiters();
    }

    pub(crate) fn current_uri_version(&self, uri: &Uri) -> u64 {
        self.state
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .get(uri)
            .map(|(_, version)| *version)
            .unwrap_or_default()
    }

    pub(crate) fn get(&self, uri: Option<&Uri>) -> Vec<PublishDiagnosticsParams> {
        let state = self.state.read().unwrap_or_else(PoisonError::into_inner);
        if let Some(uri) = uri {
            state.get(uri).map(|(diagnostics, _)| diagnostics.clone()).into_iter().collect()
        } else {
            state.values().map(|(diagnostics, _)| diagnostics.clone()).collect()
        }
    }

    pub(crate) fn forget_uri(&self, uri: &Uri) {
        self.state.write().unwrap_or_else(PoisonError::into_inner).remove(uri);
    }

    pub(crate) async fn wait_for_uri_fresh(&self, uri: &Uri, version_before: u64, timeout: Duration) {
        let deadline = tokio::time::Instant::now() + timeout;

        loop {
            let current_version = self.current_uri_version(uri);
            if current_version != version_before {
                break;
            }

            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            if remaining.is_zero() {
                return;
            }

            tokio::select! {
                () = self.notify.notified() => {}
                () = tokio::time::sleep(remaining) => return,
            }
        }

        let mut last_version = self.current_uri_version(uri);
        loop {
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            if remaining.is_zero() {
                return;
            }

            let settle_wait = DIAGNOSTICS_SETTLE_DURATION.min(remaining);
            tokio::select! {
                () = self.notify.notified() => {
                    let new_version = self.current_uri_version(uri);
                    if new_version != last_version {
                        last_version = new_version;
                    }
                }
                () = tokio::time::sleep(settle_wait) => {
                    let final_version = self.current_uri_version(uri);
                    if final_version == last_version {
                        return;
                    }
                    last_version = final_version;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lsp_types::{Diagnostic, Position, Range};

    fn diagnostics(uri: &str, message: &str) -> PublishDiagnosticsParams {
        PublishDiagnosticsParams {
            uri: uri.parse().unwrap(),
            diagnostics: vec![Diagnostic {
                range: Range { start: Position::new(0, 0), end: Position::new(0, 1) },
                message: message.to_string(),
                ..Default::default()
            }],
            version: None,
        }
    }

    #[tokio::test(start_paused = true)]
    async fn wait_for_uri_fresh_waits_for_settle_window() {
        let store = DiagnosticsStore::new();
        let uri: Uri = "file:///test.rs".parse().unwrap();
        let version_before = store.current_uri_version(&uri);
        let publish_store = store.clone();
        let publish_uri = uri.clone();

        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(10)).await;
            publish_store.publish(diagnostics(publish_uri.as_str(), "first"));
            tokio::time::sleep(Duration::from_millis(50)).await;
            publish_store.publish(diagnostics(publish_uri.as_str(), "second"));
        });

        let start = tokio::time::Instant::now();
        store.wait_for_uri_fresh(&uri, version_before, Duration::from_secs(2)).await;

        assert!(start.elapsed() >= Duration::from_millis(600), "store should wait through the settle window");
        let diags = store.get(None);
        assert_eq!(diags[0].diagnostics[0].message, "second");
    }

    #[tokio::test(start_paused = true)]
    async fn wait_for_uri_fresh_ignores_unrelated_publishes() {
        let store = DiagnosticsStore::new();
        let target: Uri = "file:///target.rs".parse().unwrap();
        let other: Uri = "file:///other.rs".parse().unwrap();
        let version_before = store.current_uri_version(&target);

        let waiter = {
            let store = store.clone();
            let target = target.clone();
            tokio::spawn(async move {
                store.wait_for_uri_fresh(&target, version_before, Duration::from_secs(2)).await;
            })
        };

        tokio::time::sleep(Duration::from_millis(10)).await;
        store.publish(diagnostics(other.as_str(), "other"));
        tokio::time::sleep(Duration::from_millis(50)).await;

        assert!(!waiter.is_finished(), "unrelated publishes should not satisfy target URI freshness");

        store.publish(diagnostics(target.as_str(), "target"));
        waiter.await.unwrap();
    }

    #[tokio::test]
    async fn forget_removes_cached_diagnostics() {
        let store = DiagnosticsStore::new();
        let uri: Uri = "file:///test.rs".parse().unwrap();
        store.publish(diagnostics(uri.as_str(), "error"));
        store.forget_uri(&uri);
        assert!(store.get(Some(&uri)).is_empty());
    }
}
