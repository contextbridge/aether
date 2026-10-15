use crate::uri::uri_to_path;
use lsp_types::{FileEvent, Uri};
use std::collections::HashMap;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::sync::{Arc, Mutex, PoisonError};
use tokio::fs::read_to_string;

#[derive(Clone, Default)]
pub(crate) struct DocumentLifecycle {
    documents: Arc<Mutex<HashMap<Uri, DocumentEntry>>>,
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

    pub(crate) fn forget_uri(&self, uri: &Uri) {
        self.documents.lock().unwrap_or_else(PoisonError::into_inner).remove(uri);
    }

    pub(crate) async fn acquire(&self, uri: &Uri) -> AcquireAction {
        let file_path = uri_to_path(uri);
        let Ok(content) = read_to_string(&file_path).await else {
            return AcquireAction::MissingOnDisk;
        };

        let content_hash = hash_content(&content);
        let mut documents = self.documents.lock().unwrap_or_else(PoisonError::into_inner);
        let entry = documents.entry(uri.clone()).or_default();
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

    pub(crate) fn release(&self, uri: &Uri) -> ReleaseAction {
        let mut documents = self.documents.lock().unwrap_or_else(PoisonError::into_inner);
        let Some(entry) = documents.get_mut(uri) else {
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

    pub(crate) fn filter_watcher_changes(&self, changes: Vec<FileEvent>) -> Vec<FileEvent> {
        let mut documents = self.documents.lock().unwrap_or_else(PoisonError::into_inner);
        changes
            .into_iter()
            .filter_map(|change| {
                if let Some(entry) = documents.get_mut(&change.uri)
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
    use lsp_types::FileChangeType;
    use tempfile::TempDir;

    #[tokio::test]
    async fn acquire_returns_open_action() {
        let temp = TempDir::new().unwrap();
        let path = temp.path().join("main.rs");
        std::fs::write(&path, "fn main() {}\n").unwrap();

        let action = DocumentLifecycle::new().acquire(&crate::path_to_uri(&path).unwrap()).await;

        let AcquireAction::Open { file_path, content } = action else {
            panic!("expected Open");
        };
        assert_eq!(file_path, path.canonicalize().unwrap().to_string_lossy());
        assert_eq!(content, "fn main() {}\n");
    }

    #[tokio::test]
    async fn acquire_changed_content_returns_reopen() {
        let (temp, uri, lifecycle) = open_fixture().await;
        let path = temp.path().join("main.rs");
        std::fs::write(&path, "fn main() { println!(\"hi\"); }\n").unwrap();

        let AcquireAction::Reopen { file_path, content } = lifecycle.acquire(&uri).await else {
            panic!("expected Reopen after content change");
        };
        assert_eq!(file_path, path.canonicalize().unwrap().to_string_lossy());
        assert_eq!(content, "fn main() { println!(\"hi\"); }\n");
    }

    #[tokio::test]
    async fn acquire_missing_file() {
        let uri: Uri = "file:///nonexistent/path.rs".parse().unwrap();
        assert!(matches!(DocumentLifecycle::new().acquire(&uri).await, AcquireAction::MissingOnDisk));
    }

    #[test]
    fn release_when_not_open_returns_unchanged() {
        let uri: Uri = "file:///test.rs".parse().unwrap();
        assert!(matches!(DocumentLifecycle::new().release(&uri), ReleaseAction::Unchanged));
    }

    #[tokio::test]
    async fn release_after_acquire_closes() {
        let (_temp, uri, lifecycle) = open_fixture().await;

        assert!(matches!(lifecycle.release(&uri), ReleaseAction::Close));
        assert!(matches!(lifecycle.release(&uri), ReleaseAction::Unchanged));
    }

    #[tokio::test]
    async fn repeated_acquire_stays_open_until_last_release() {
        let (_temp, uri, lifecycle) = open_fixture().await;

        assert!(matches!(lifecycle.acquire(&uri).await, AcquireAction::Unchanged));
        assert!(matches!(lifecycle.release(&uri), ReleaseAction::Unchanged));
        assert!(matches!(lifecycle.release(&uri), ReleaseAction::Close));
    }

    #[tokio::test]
    async fn filter_watcher_changes_suppresses_open_docs() {
        let (_temp, uri, lifecycle) = open_fixture().await;
        let other_uri: Uri = "file:///other.rs".parse().unwrap();

        let filtered = lifecycle.filter_watcher_changes(vec![changed(&uri), changed(&other_uri)]);

        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].uri, other_uri);
    }

    #[tokio::test]
    async fn suppressed_watcher_change_requests_refresh_after_close() {
        let (_temp, uri, lifecycle) = open_fixture().await;

        assert_eq!(lifecycle.filter_watcher_changes(vec![changed(&uri)]), Vec::<FileEvent>::new());
        assert!(matches!(lifecycle.release(&uri), ReleaseAction::CloseAndRefresh));
    }

    #[tokio::test]
    async fn suppressed_watcher_change_waits_for_last_release() {
        let (_temp, uri, lifecycle) = open_fixture().await;
        assert!(matches!(lifecycle.acquire(&uri).await, AcquireAction::Unchanged));

        assert_eq!(lifecycle.filter_watcher_changes(vec![changed(&uri)]), Vec::<FileEvent>::new());
        assert!(matches!(lifecycle.release(&uri), ReleaseAction::Unchanged));
        assert!(matches!(lifecycle.release(&uri), ReleaseAction::CloseAndRefresh));
    }

    async fn open_fixture() -> (TempDir, Uri, DocumentLifecycle) {
        let temp = TempDir::new().unwrap();
        let path = temp.path().join("main.rs");
        std::fs::write(&path, "fn main() {}\n").unwrap();
        let uri = crate::path_to_uri(&path).unwrap();
        let lifecycle = DocumentLifecycle::new();
        assert!(matches!(lifecycle.acquire(&uri).await, AcquireAction::Open { .. }));
        (temp, uri, lifecycle)
    }

    fn changed(uri: &Uri) -> FileEvent {
        FileEvent { uri: uri.clone(), typ: FileChangeType::CHANGED }
    }
}
