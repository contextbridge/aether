use crate::diagnostics_store::DiagnosticsStore;
use crate::document_lifecycle::{AcquireAction, DocumentLifecycle, ReleaseAction};
use crate::language_catalog::LanguageId;
use crate::process_transport::{ProcessTransport, TransportEvent};
use crate::protocol::LspNotification;
use crate::refresh_queue::RefreshQueue;
use ignore::WalkBuilder;
use lsp_types::notification::{
    DidChangeWatchedFiles, DidCloseTextDocument, DidOpenTextDocument, DidSaveTextDocument, Notification,
};
use lsp_types::{
    DidChangeWatchedFilesParams, DidCloseTextDocumentParams, DidOpenTextDocumentParams, DidSaveTextDocumentParams,
    PublishDiagnosticsParams, TextDocumentIdentifier, TextDocumentItem, Uri,
};
use serde_json::Value;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;

const DIAGNOSTICS_TIMEOUT: Duration = Duration::from_secs(20);
const BACKGROUND_REFRESH_TIMEOUT: Duration = Duration::from_secs(20);

pub(crate) struct WorkspaceSession {
    transport: ProcessTransport,
    documents: DocumentLifecycle,
    diagnostics: DiagnosticsStore,
    refresh: RefreshQueue,
}

impl WorkspaceSession {
    pub(crate) fn spawn(
        workspace_root: &Path,
        command: &str,
        args: &[String],
        supported_extensions: HashSet<String>,
    ) -> crate::DaemonResult<Self> {
        let (transport, event_rx) = ProcessTransport::spawn(workspace_root, command, args)?;
        let documents = DocumentLifecycle::new();
        let diagnostics = DiagnosticsStore::new();
        let refresh = RefreshQueue::new();

        let session = Self { transport, documents, diagnostics, refresh };
        let supported_extensions = Arc::new(supported_extensions);

        tokio::spawn(run_session_events(
            session.transport.clone(),
            session.documents.clone(),
            session.diagnostics.clone(),
            session.refresh.clone(),
            Arc::clone(&supported_extensions),
            event_rx,
        ));

        tokio::spawn(run_background_refresh_worker(
            session.transport.clone(),
            session.documents.clone(),
            session.diagnostics.clone(),
            session.refresh.clone(),
        ));

        tokio::spawn(bootstrap_workspace_refresh(
            workspace_root.to_path_buf(),
            supported_extensions,
            session.refresh.clone(),
        ));

        Ok(session)
    }

    pub(crate) async fn request_raw(&self, method: &str, params: Value) -> Result<Value, crate::LspErrorResponse> {
        self.transport.request_raw(method, params).await
    }

    pub(crate) async fn queue_diagnostic_refresh(&self, uri: Uri) {
        self.refresh.enqueue(vec![uri]).await;
    }

    pub(crate) async fn ensure_document_open(&self, uri: &Uri) -> Option<u64> {
        sync_document(&self.transport, &self.documents, &self.diagnostics, uri).await
    }

    pub(crate) async fn close_document(&self, uri: &Uri) {
        release_document(&self.transport, &self.documents, &self.refresh, uri).await;
    }

    pub(crate) async fn get_diagnostics(&self, uri: Option<&Uri>) -> Vec<PublishDiagnosticsParams> {
        self.sync_documents_for_diagnostics(uri).await;
        self.diagnostics.get(uri).await
    }

    pub(crate) async fn shutdown(&self) {
        self.refresh.shutdown().await;
        self.transport.shutdown().await;
    }

    async fn sync_documents_for_diagnostics(&self, uri: Option<&Uri>) {
        if let Some(uri) = uri {
            let version_before = self.ensure_document_open(uri).await;
            if let Some(version_before) = version_before {
                self.diagnostics.wait_for_uri_fresh(uri, version_before, DIAGNOSTICS_TIMEOUT).await;
            } else {
                self.refresh.wait_for_current_generation(DIAGNOSTICS_TIMEOUT).await;
            }
            self.close_document(uri).await;
            return;
        }

        self.refresh.wait_for_current_generation(BACKGROUND_REFRESH_TIMEOUT).await;
    }
}

async fn sync_document(
    transport: &ProcessTransport,
    documents: &DocumentLifecycle,
    diagnostics: &DiagnosticsStore,
    uri: &Uri,
) -> Option<u64> {
    let notifications = match documents.acquire(uri).await {
        AcquireAction::Open { file_path, content } => open_and_save_notifications(uri, &file_path, content),
        AcquireAction::Reopen { file_path, content } => reopen_notifications(uri, &file_path, content),
        AcquireAction::Unchanged => return None,
        AcquireAction::MissingOnDisk => {
            documents.forget_uri(uri).await;
            diagnostics.forget_uri(uri).await;
            return None;
        }
    };

    let version_before = diagnostics.current_uri_version(uri).await;
    for notification in notifications {
        transport.send_notification(notification).await;
    }
    Some(version_before)
}

async fn release_document(
    transport: &ProcessTransport,
    documents: &DocumentLifecycle,
    refresh: &RefreshQueue,
    uri: &Uri,
) {
    match documents.release(uri).await {
        ReleaseAction::Close => {
            transport.send_notification(close_notification(uri)).await;
        }
        ReleaseAction::CloseAndRefresh => {
            transport.send_notification(close_notification(uri)).await;
            refresh.enqueue(vec![uri.clone()]).await;
        }
        ReleaseAction::Unchanged => {}
    }
}

async fn run_background_refresh_worker(
    transport: ProcessTransport,
    documents: DocumentLifecycle,
    diagnostics: DiagnosticsStore,
    refresh: RefreshQueue,
) {
    while let Some(uri) = refresh.recv().await {
        refresh_uri(&transport, &documents, &diagnostics, &refresh, &uri).await;
    }
}

async fn refresh_uri(
    transport: &ProcessTransport,
    documents: &DocumentLifecycle,
    diagnostics: &DiagnosticsStore,
    refresh: &RefreshQueue,
    uri: &Uri,
) {
    let sync_result = sync_document(transport, documents, diagnostics, uri).await;
    if let Some(version_before) = sync_result {
        diagnostics.wait_for_uri_fresh(uri, version_before, DIAGNOSTICS_TIMEOUT).await;
    }

    release_document(transport, documents, refresh, uri).await;
}

async fn bootstrap_workspace_refresh(
    workspace_root: PathBuf,
    supported_extensions: Arc<HashSet<String>>,
    refresh: RefreshQueue,
) {
    let uris = if supported_extensions.is_empty() {
        Vec::new()
    } else {
        tokio::task::spawn_blocking(move || {
            let mut builder = WalkBuilder::new(&workspace_root);
            builder.standard_filters(true);

            let mut uris = Vec::new();
            for entry in builder.build() {
                let Ok(entry) = entry else {
                    continue;
                };
                if !entry.file_type().is_some_and(|file_type| file_type.is_file()) {
                    continue;
                }
                if !path_is_supported(entry.path(), supported_extensions.as_ref()) {
                    continue;
                }
                if let Ok(uri) = crate::path_to_uri(entry.path()) {
                    uris.push(uri);
                }
            }
            uris
        })
        .await
        .unwrap_or_default()
    };

    refresh.enqueue(uris).await;
    refresh.complete_bootstrap().await;
}

async fn run_session_events(
    transport: ProcessTransport,
    documents: DocumentLifecycle,
    diagnostics: DiagnosticsStore,
    refresh: RefreshQueue,
    supported_extensions: Arc<HashSet<String>>,
    mut event_rx: mpsc::Receiver<TransportEvent>,
) {
    while let Some(event) = event_rx.recv().await {
        match event {
            TransportEvent::PublishedDiagnostics(params) => {
                diagnostics.publish(params).await;
            }
            TransportEvent::FileWatcherBatch(batch) => {
                let filtered = documents.filter_watcher_changes(batch.forwarded_changes).await;
                let discovered = filter_supported_uris(batch.discovered_uris, supported_extensions.as_ref());

                let mut refresh_uris = filter_supported_uris(
                    filtered.iter().map(|change| change.uri.clone()).collect(),
                    supported_extensions.as_ref(),
                );
                refresh_uris.extend(discovered);
                refresh.enqueue(refresh_uris).await;

                if filtered.is_empty() {
                    continue;
                }

                let params = DidChangeWatchedFilesParams { changes: filtered };
                if let Ok(value) = serde_json::to_value(&params) {
                    transport
                        .send_notification(LspNotification {
                            method: DidChangeWatchedFiles::METHOD.to_string(),
                            params: value,
                        })
                        .await;
                }
            }
            TransportEvent::Closed => break,
        }
    }
}

fn filter_supported_uris(uris: Vec<Uri>, supported_extensions: &HashSet<String>) -> Vec<Uri> {
    uris.into_iter().filter(|uri| uri_is_supported(uri, supported_extensions)).collect()
}

fn uri_is_supported(uri: &Uri, supported_extensions: &HashSet<String>) -> bool {
    let path = crate::uri_to_path(uri);
    path_is_supported(Path::new(&path), supported_extensions)
}

fn path_is_supported(path: &Path, supported_extensions: &HashSet<String>) -> bool {
    path.extension().and_then(|ext| ext.to_str()).is_some_and(|ext| supported_extensions.contains(ext))
}

fn open_and_save_notifications(uri: &Uri, file_path: &str, content: String) -> Vec<LspNotification> {
    vec![open_notification(uri, file_path, 1, content), save_notification(uri)]
}

fn reopen_notifications(uri: &Uri, file_path: &str, content: String) -> Vec<LspNotification> {
    vec![close_notification(uri), open_notification(uri, file_path, 1, content), save_notification(uri)]
}

fn open_notification(uri: &Uri, file_path: &str, version: i32, content: String) -> LspNotification {
    let language_id = LanguageId::from_path(Path::new(file_path));
    let params = DidOpenTextDocumentParams {
        text_document: TextDocumentItem {
            uri: uri.clone(),
            language_id: language_id.as_str().to_string(),
            version,
            text: content,
        },
    };
    LspNotification { method: DidOpenTextDocument::METHOD.to_string(), params: serde_json::to_value(&params).unwrap() }
}

fn save_notification(uri: &Uri) -> LspNotification {
    let params = DidSaveTextDocumentParams { text_document: TextDocumentIdentifier { uri: uri.clone() }, text: None };
    LspNotification { method: DidSaveTextDocument::METHOD.to_string(), params: serde_json::to_value(&params).unwrap() }
}

fn close_notification(uri: &Uri) -> LspNotification {
    let params = DidCloseTextDocumentParams { text_document: TextDocumentIdentifier { uri: uri.clone() } };
    LspNotification { method: DidCloseTextDocument::METHOD.to_string(), params: serde_json::to_value(&params).unwrap() }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn open_and_save_notifications_emit_open_then_save() {
        let uri: Uri = "file:///workspace/main.rs".parse().unwrap();
        let notifications = open_and_save_notifications(&uri, "/workspace/main.rs", "fn main() {}\n".to_string());

        assert_eq!(notifications.len(), 2);
        assert_eq!(notifications[0].method, DidOpenTextDocument::METHOD);
        assert_eq!(notifications[1].method, DidSaveTextDocument::METHOD);
    }

    #[test]
    fn reopen_notifications_emit_close_open_save() {
        let uri: Uri = "file:///workspace/main.rs".parse().unwrap();
        let notifications = reopen_notifications(&uri, "/workspace/main.rs", "fn main() {}\n".to_string());

        assert_eq!(notifications.len(), 3);
        assert_eq!(notifications[0].method, DidCloseTextDocument::METHOD);
        assert_eq!(notifications[1].method, DidOpenTextDocument::METHOD);
        assert_eq!(notifications[2].method, DidSaveTextDocument::METHOD);
    }
}
