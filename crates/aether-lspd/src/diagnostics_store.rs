use lsp_types::{PublishDiagnosticsParams, Uri};
use std::collections::HashMap;
use std::time::Duration;
use tokio::sync::watch;
use tokio::time::{Instant, timeout_at};

#[derive(Clone)]
pub(crate) struct DiagnosticsStore {
    state: watch::Sender<DiagnosticsState>,
    settle: Duration,
}

/// Whether a diagnostics wait saw the server publish for the file.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Freshness {
    Fresh,
    Unconfirmed,
}

impl DiagnosticsStore {
    pub(crate) fn new(settle: Duration) -> Self {
        Self { state: watch::Sender::new(DiagnosticsState::default()), settle }
    }

    pub(crate) fn publish(&self, diagnostics: PublishDiagnosticsParams) {
        self.state.send_modify(|state| {
            state.generation += 1;
            state.diagnostics.insert(diagnostics.uri.clone(), (diagnostics, state.generation));
        });
    }

    pub(crate) fn current_uri_version(&self, uri: &Uri) -> u64 {
        self.state.borrow().uri_version(uri)
    }

    pub(crate) fn get(&self, uri: Option<&Uri>) -> Vec<PublishDiagnosticsParams> {
        let state = self.state.borrow();
        if let Some(uri) = uri {
            state.diagnostics.get(uri).map(|(diagnostics, _)| diagnostics.clone()).into_iter().collect()
        } else {
            state.diagnostics.values().map(|(diagnostics, _)| diagnostics.clone()).collect()
        }
    }

    pub(crate) fn uris(&self) -> Vec<Uri> {
        self.state.borrow().diagnostics.keys().cloned().collect()
    }

    pub(crate) fn forget_uri(&self, uri: &Uri) {
        self.state.send_if_modified(|state| {
            state.diagnostics.remove(uri);
            // Eviction is not a server publication and should not wake freshness waiters.
            false
        });
    }

    pub(crate) async fn wait_for_uri_fresh(&self, uri: &Uri, version_before: u64, timeout: Duration) -> Freshness {
        let deadline = Instant::now() + timeout;
        let mut published = self.state.subscribe();
        let first = published.wait_for(|state| state.uri_version(uri) != version_before);
        if !matches!(timeout_at(deadline, first).await, Ok(Ok(_))) {
            return Freshness::Unconfirmed;
        }

        loop {
            let settled = self.current_uri_version(uri);
            let quiet_until = (Instant::now() + self.settle).min(deadline);
            let republished = published.wait_for(|state| state.uri_version(uri) != settled);
            if !matches!(timeout_at(quiet_until, republished).await, Ok(Ok(_))) {
                return Freshness::Fresh;
            }
        }
    }
}

#[derive(Default)]
struct DiagnosticsState {
    diagnostics: HashMap<Uri, (PublishDiagnosticsParams, u64)>,
    generation: u64,
}

impl DiagnosticsState {
    fn uri_version(&self, uri: &Uri) -> u64 {
        self.diagnostics.get(uri).map(|(_, version)| *version).unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lsp_types::{Diagnostic, Position, Range};

    const SETTLE: Duration = Duration::from_millis(600);

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
        let store = DiagnosticsStore::new(SETTLE);
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
        let freshness = store.wait_for_uri_fresh(&uri, version_before, Duration::from_secs(2)).await;

        assert_eq!(freshness, Freshness::Fresh);
        assert!(start.elapsed() >= Duration::from_millis(600), "store should wait through the settle window");
        let diags = store.get(None);
        assert_eq!(diags[0].diagnostics[0].message, "second");
    }

    #[tokio::test(start_paused = true)]
    async fn wait_for_uri_fresh_ignores_unrelated_publishes() {
        let store = DiagnosticsStore::new(SETTLE);
        let target: Uri = "file:///target.rs".parse().unwrap();
        let other: Uri = "file:///other.rs".parse().unwrap();
        let version_before = store.current_uri_version(&target);

        let waiter = {
            let store = store.clone();
            let target = target.clone();
            tokio::spawn(async move { store.wait_for_uri_fresh(&target, version_before, Duration::from_secs(2)).await })
        };

        tokio::time::sleep(Duration::from_millis(10)).await;
        store.publish(diagnostics(other.as_str(), "other"));
        tokio::time::sleep(Duration::from_millis(50)).await;

        assert!(!waiter.is_finished(), "unrelated publishes should not satisfy target URI freshness");

        store.publish(diagnostics(target.as_str(), "target"));
        assert_eq!(waiter.await.unwrap(), Freshness::Fresh);
    }

    #[tokio::test(start_paused = true)]
    async fn wait_for_uri_fresh_is_unconfirmed_when_the_uri_is_never_published() {
        let store = DiagnosticsStore::new(SETTLE);
        let uri: Uri = "file:///test.rs".parse().unwrap();

        let freshness = store.wait_for_uri_fresh(&uri, 0, Duration::from_secs(2)).await;

        assert_eq!(freshness, Freshness::Unconfirmed);
    }

    #[tokio::test(start_paused = true)]
    async fn wait_for_uri_fresh_settles_while_other_uris_keep_publishing() {
        let store = DiagnosticsStore::new(SETTLE);
        let target: Uri = "file:///target.rs".parse().unwrap();
        let publisher = {
            let store = store.clone();
            let target = target.clone();
            tokio::spawn(async move {
                store.publish(diagnostics(target.as_str(), "target"));
                loop {
                    tokio::time::sleep(Duration::from_millis(100)).await;
                    store.publish(diagnostics("file:///other.rs", "other"));
                }
            })
        };

        let start = tokio::time::Instant::now();
        let freshness = store.wait_for_uri_fresh(&target, 0, Duration::from_secs(20)).await;
        publisher.abort();

        assert_eq!(freshness, Freshness::Fresh);
        assert!(start.elapsed() < Duration::from_secs(1), "settled after {:?}", start.elapsed());
    }

    #[tokio::test]
    async fn forget_removes_cached_diagnostics() {
        let store = DiagnosticsStore::new(SETTLE);
        let uri: Uri = "file:///test.rs".parse().unwrap();
        store.publish(diagnostics(uri.as_str(), "error"));
        store.forget_uri(&uri);
        assert_eq!(store.get(Some(&uri)).len(), 0);
    }
}
