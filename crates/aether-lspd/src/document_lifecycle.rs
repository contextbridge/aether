use crate::uri::uri_to_path;
use lsp_types::{FileEvent, Uri};
use std::collections::HashMap;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::sync::Arc;
use tokio::fs::read_to_string;
use tokio::sync::RwLock;

#[derive(Clone, Default)]
pub(crate) struct DocumentLifecycle {
    state: Arc<RwLock<DocumentState>>,
}

#[derive(Default)]
struct DocumentState {
    documents: HashMap<Uri, DocumentEntry>,
}

#[derive(Clone, Copy, Debug, Default)]
struct DocumentEntry {
    open_holders: usize,
    content_hash: Option<u64>,
    needs_refresh: bool,
}

pub(crate) enum AcquireAction {
    Open { file_path: String, content: String },
    Reopen { file_path: String, content: String },
    Unchanged,
    MissingOnDisk,
}

pub(crate) enum ReleaseAction {
    Close,
    CloseAndRefresh,
    Unchanged,
}

impl DocumentLifecycle {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    pub(crate) async fn forget_uri(&self, uri: &Uri) {
        self.state.write().await.documents.remove(uri);
    }

    pub(crate) async fn acquire(&self, uri: &Uri) -> AcquireAction {
        let file_path = uri_to_path(uri);
        let Ok(content) = read_to_string(&file_path).await else {
            return AcquireAction::MissingOnDisk;
        };

        let content_hash = hash_content(&content);
        let mut state = self.state.write().await;
        let entry = state.documents.entry(uri.clone()).or_default();
        entry.open_holders += 1;

        if entry.open_holders == 1 {
            entry.content_hash = Some(content_hash);
            return AcquireAction::Open { file_path, content };
        }

        if entry.content_hash == Some(content_hash) {
            AcquireAction::Unchanged
        } else {
            entry.content_hash = Some(content_hash);
            AcquireAction::Reopen { file_path, content }
        }
    }

    pub(crate) async fn release(&self, uri: &Uri) -> ReleaseAction {
        let mut state = self.state.write().await;
        let Some(entry) = state.documents.get_mut(uri) else {
            return ReleaseAction::Unchanged;
        };

        if entry.open_holders == 0 {
            return ReleaseAction::Unchanged;
        }

        entry.open_holders -= 1;
        if entry.open_holders > 0 {
            return ReleaseAction::Unchanged;
        }

        if entry.needs_refresh {
            entry.needs_refresh = false;
            ReleaseAction::CloseAndRefresh
        } else {
            ReleaseAction::Close
        }
    }

    pub(crate) async fn filter_watcher_changes(&self, changes: Vec<FileEvent>) -> Vec<FileEvent> {
        let mut state = self.state.write().await;
        changes
            .into_iter()
            .filter_map(|change| {
                if let Some(entry) = state.documents.get_mut(&change.uri)
                    && entry.open_holders > 0
                {
                    entry.needs_refresh = true;
                    None
                } else {
                    Some(change)
                }
            })
            .collect()
    }
}

fn hash_content(content: &str) -> u64 {
    let mut hasher = DefaultHasher::new();
    content.hash(&mut hasher);
    hasher.finish()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;
    use tempfile::TempDir;

    fn uri_for(path: &Path) -> Uri {
        crate::path_to_uri(path).unwrap()
    }

    #[tokio::test]
    async fn acquire_returns_open_action() {
        let temp = TempDir::new().unwrap();
        let path = temp.path().join("main.rs");
        std::fs::write(&path, "fn main() {}\n").unwrap();
        let uri = uri_for(&path);
        let lifecycle = DocumentLifecycle::new();

        let action = lifecycle.acquire(&uri).await;
        let AcquireAction::Open { file_path, content } = action else {
            panic!("expected Open");
        };
        let expected_path = path.canonicalize().unwrap();
        assert_eq!(file_path, expected_path.to_string_lossy());
        assert_eq!(content, "fn main() {}\n");
    }

    #[tokio::test]
    async fn acquire_unchanged_content_returns_unchanged() {
        let temp = TempDir::new().unwrap();
        let path = temp.path().join("main.rs");
        std::fs::write(&path, "fn main() {}\n").unwrap();
        let uri = uri_for(&path);
        let lifecycle = DocumentLifecycle::new();

        assert!(matches!(lifecycle.acquire(&uri).await, AcquireAction::Open { .. }));
        assert!(matches!(lifecycle.acquire(&uri).await, AcquireAction::Unchanged));
    }

    #[tokio::test]
    async fn acquire_changed_content_returns_reopen() {
        let temp = TempDir::new().unwrap();
        let path = temp.path().join("main.rs");
        std::fs::write(&path, "fn main() {}\n").unwrap();
        let uri = uri_for(&path);
        let lifecycle = DocumentLifecycle::new();

        assert!(matches!(lifecycle.acquire(&uri).await, AcquireAction::Open { .. }));

        std::fs::write(&path, "fn main() { println!(\"hi\"); }\n").unwrap();
        let action = lifecycle.acquire(&uri).await;
        let AcquireAction::Reopen { file_path, content } = action else {
            panic!("expected Reopen after content change");
        };
        let expected_path = path.canonicalize().unwrap();
        assert_eq!(file_path, expected_path.to_string_lossy());
        assert_eq!(content, "fn main() { println!(\"hi\"); }\n");
    }

    #[tokio::test]
    async fn acquire_missing_file() {
        let uri: Uri = "file:///nonexistent/path.rs".parse().unwrap();
        let lifecycle = DocumentLifecycle::new();
        assert!(matches!(lifecycle.acquire(&uri).await, AcquireAction::MissingOnDisk));
    }

    #[tokio::test]
    async fn release_when_not_open_returns_unchanged() {
        let uri: Uri = "file:///test.rs".parse().unwrap();
        let lifecycle = DocumentLifecycle::new();
        assert!(matches!(lifecycle.release(&uri).await, ReleaseAction::Unchanged));
    }

    #[tokio::test]
    async fn release_after_acquire_closes() {
        let temp = TempDir::new().unwrap();
        let path = temp.path().join("main.rs");
        std::fs::write(&path, "fn main() {}\n").unwrap();
        let uri = uri_for(&path);
        let lifecycle = DocumentLifecycle::new();

        assert!(matches!(lifecycle.acquire(&uri).await, AcquireAction::Open { .. }));
        assert!(matches!(lifecycle.release(&uri).await, ReleaseAction::Close));
        assert!(matches!(lifecycle.release(&uri).await, ReleaseAction::Unchanged));
    }

    #[tokio::test]
    async fn filter_watcher_changes_suppresses_open_docs() {
        let temp = TempDir::new().unwrap();
        let path = temp.path().join("main.rs");
        std::fs::write(&path, "fn main() {}\n").unwrap();
        let uri = uri_for(&path);
        let other_uri: Uri = "file:///other.rs".parse().unwrap();
        let lifecycle = DocumentLifecycle::new();

        assert!(matches!(lifecycle.acquire(&uri).await, AcquireAction::Open { .. }));

        let changes = vec![
            FileEvent { uri: uri.clone(), typ: lsp_types::FileChangeType::CHANGED },
            FileEvent { uri: other_uri.clone(), typ: lsp_types::FileChangeType::CHANGED },
        ];
        let filtered = lifecycle.filter_watcher_changes(changes).await;
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].uri, other_uri);
    }

    #[tokio::test]
    async fn suppressed_watcher_change_requests_refresh_after_close() {
        let temp = TempDir::new().unwrap();
        let path = temp.path().join("main.rs");
        std::fs::write(&path, "fn main() {}\n").unwrap();
        let uri = uri_for(&path);
        let lifecycle = DocumentLifecycle::new();

        assert!(matches!(lifecycle.acquire(&uri).await, AcquireAction::Open { .. }));

        let filtered = lifecycle
            .filter_watcher_changes(vec![FileEvent { uri: uri.clone(), typ: lsp_types::FileChangeType::CHANGED }])
            .await;
        assert!(filtered.is_empty());
        assert!(matches!(lifecycle.release(&uri).await, ReleaseAction::CloseAndRefresh));
    }

    #[tokio::test]
    async fn suppressed_watcher_change_waits_for_last_release() {
        let temp = TempDir::new().unwrap();
        let path = temp.path().join("main.rs");
        std::fs::write(&path, "fn main() {}\n").unwrap();
        let uri = uri_for(&path);
        let lifecycle = DocumentLifecycle::new();

        assert!(matches!(lifecycle.acquire(&uri).await, AcquireAction::Open { .. }));
        assert!(matches!(lifecycle.acquire(&uri).await, AcquireAction::Unchanged));

        let filtered = lifecycle
            .filter_watcher_changes(vec![FileEvent { uri: uri.clone(), typ: lsp_types::FileChangeType::CHANGED }])
            .await;
        assert!(filtered.is_empty());
        assert!(matches!(lifecycle.release(&uri).await, ReleaseAction::Unchanged));
        assert!(matches!(lifecycle.release(&uri).await, ReleaseAction::CloseAndRefresh));
    }

    #[tokio::test]
    async fn concurrent_acquire_single_release_keeps_open() {
        let temp = TempDir::new().unwrap();
        let path = temp.path().join("main.rs");
        std::fs::write(&path, "fn main() {}\n").unwrap();
        let uri = uri_for(&path);
        let lifecycle = DocumentLifecycle::new();

        assert!(matches!(lifecycle.acquire(&uri).await, AcquireAction::Open { .. }));
        assert!(matches!(lifecycle.acquire(&uri).await, AcquireAction::Unchanged));
        assert!(matches!(lifecycle.release(&uri).await, ReleaseAction::Unchanged));
    }

    #[tokio::test]
    async fn concurrent_acquire_both_release_closes() {
        let temp = TempDir::new().unwrap();
        let path = temp.path().join("main.rs");
        std::fs::write(&path, "fn main() {}\n").unwrap();
        let uri = uri_for(&path);
        let lifecycle = DocumentLifecycle::new();

        assert!(matches!(lifecycle.acquire(&uri).await, AcquireAction::Open { .. }));
        assert!(matches!(lifecycle.acquire(&uri).await, AcquireAction::Unchanged));
        assert!(matches!(lifecycle.release(&uri).await, ReleaseAction::Unchanged));
        assert!(matches!(lifecycle.release(&uri).await, ReleaseAction::Close));
    }
}
