use lsp_types::{PublishDiagnosticsParams, Uri};
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;
use tokio::sync::{Notify, RwLock};

const DIAGNOSTICS_SETTLE_DURATION: Duration = Duration::from_millis(600);

#[derive(Clone, Default)]
pub(crate) struct DiagnosticsStore {
    state: Arc<RwLock<HashMap<Uri, PublishDiagnosticsParams>>>,
    notify: Arc<Notify>,
    version: Arc<AtomicU64>,
    uri_versions: Arc<RwLock<HashMap<Uri, u64>>>,
}

impl DiagnosticsStore {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    pub(crate) async fn publish(&self, diagnostics: PublishDiagnosticsParams) {
        let uri = diagnostics.uri.clone();
        let version = self.version.fetch_add(1, Ordering::Relaxed) + 1;
        self.state.write().await.insert(uri.clone(), diagnostics);
        self.uri_versions.write().await.insert(uri, version);
        self.notify.notify_waiters();
    }

    pub(crate) async fn current_uri_version(&self, uri: &Uri) -> u64 {
        self.uri_versions.read().await.get(uri).copied().unwrap_or_default()
    }

    pub(crate) async fn get(&self, uri: Option<&Uri>) -> Vec<PublishDiagnosticsParams> {
        let state = self.state.read().await;
        if let Some(uri) = uri {
            state.get(uri).cloned().into_iter().collect()
        } else {
            state.values().cloned().collect()
        }
    }

    pub(crate) async fn forget_uri(&self, uri: &Uri) {
        self.state.write().await.remove(uri);
        self.uri_versions.write().await.remove(uri);
    }

    pub(crate) async fn wait_for_uri_fresh(&self, uri: &Uri, version_before: u64, timeout: Duration) {
        let deadline = tokio::time::Instant::now() + timeout;

        loop {
            let current_version = self.current_uri_version(uri).await;
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

        let mut last_version = self.current_uri_version(uri).await;
        loop {
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            if remaining.is_zero() {
                return;
            }

            let settle_wait = DIAGNOSTICS_SETTLE_DURATION.min(remaining);
            tokio::select! {
                () = self.notify.notified() => {
                    let new_version = self.current_uri_version(uri).await;
                    if new_version != last_version {
                        last_version = new_version;
                    }
                }
                () = tokio::time::sleep(settle_wait) => {
                    let final_version = self.current_uri_version(uri).await;
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

    #[tokio::test]
    async fn wait_for_uri_fresh_waits_for_settle_window() {
        let store = DiagnosticsStore::new();
        let uri: Uri = "file:///test.rs".parse().unwrap();
        let version_before = store.current_uri_version(&uri).await;
        let publish_store = store.clone();
        let publish_uri = uri.clone();

        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(10)).await;
            publish_store.publish(diagnostics(publish_uri.as_str(), "first")).await;
            tokio::time::sleep(Duration::from_millis(50)).await;
            publish_store.publish(diagnostics(publish_uri.as_str(), "second")).await;
        });

        let start = tokio::time::Instant::now();
        store.wait_for_uri_fresh(&uri, version_before, Duration::from_secs(2)).await;

        assert!(start.elapsed() >= Duration::from_millis(600), "store should wait through the settle window");
        let diags = store.get(None).await;
        assert_eq!(diags[0].diagnostics[0].message, "second");
    }

    #[tokio::test]
    async fn wait_for_uri_fresh_ignores_unrelated_publishes() {
        let store = DiagnosticsStore::new();
        let target: Uri = "file:///target.rs".parse().unwrap();
        let other: Uri = "file:///other.rs".parse().unwrap();
        let version_before = store.current_uri_version(&target).await;

        let waiter = {
            let store = store.clone();
            let target = target.clone();
            tokio::spawn(async move {
                store.wait_for_uri_fresh(&target, version_before, Duration::from_secs(2)).await;
            })
        };

        tokio::time::sleep(Duration::from_millis(10)).await;
        store.publish(diagnostics(other.as_str(), "other")).await;
        tokio::time::sleep(Duration::from_millis(50)).await;

        assert!(!waiter.is_finished(), "unrelated publishes should not satisfy target URI freshness");

        store.publish(diagnostics(target.as_str(), "target")).await;
        waiter.await.unwrap();
    }

    #[tokio::test]
    async fn forget_removes_cached_diagnostics() {
        let store = DiagnosticsStore::new();
        let uri: Uri = "file:///test.rs".parse().unwrap();
        store.publish(diagnostics(uri.as_str(), "error")).await;
        store.forget_uri(&uri).await;
        assert!(store.get(Some(&uri)).await.is_empty());
    }
}
