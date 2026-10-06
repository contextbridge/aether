use crate::diagnostics_store::DiagnosticsStore;
use crate::document_lifecycle::{AcquireAction, DocumentLifecycle, ReleaseAction};
use crate::error::DaemonResult;
use crate::file_watcher::FileWatcherBatch;
use crate::language_catalog::{DiagnosticsMode, LanguageId, LspConfig, metadata_for};
use crate::process_transport::{ProcessTransport, TransportError, TransportEvent};
use crate::protocol::{LspNotification, extract_document_uri};
use crate::refresh_queue::RefreshQueue;
use ignore::WalkBuilder;
use lsp_types::notification::{
    DidChangeWatchedFiles, DidCloseTextDocument, DidOpenTextDocument, DidSaveTextDocument, Notification,
};
use lsp_types::request::{DocumentDiagnosticRequest, Request};
use lsp_types::{
    DidChangeWatchedFilesParams, DidCloseTextDocumentParams, DidOpenTextDocumentParams, DidSaveTextDocumentParams,
    DocumentDiagnosticReport, DocumentDiagnosticReportResult, PublishDiagnosticsParams, TextDocumentIdentifier,
    TextDocumentItem, Uri,
};
use serde_json::Value;
use std::collections::HashSet;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use tokio::sync::mpsc;

const DIAGNOSTICS_TIMEOUT: Duration = Duration::from_secs(20);
const BACKGROUND_REFRESH_TIMEOUT: Duration = Duration::from_secs(20);
const LSP_CONTENT_MODIFIED: i32 = -32801;
const LSP_SERVER_CANCELLED: i32 = -32802;
const TRANSIENT_RETRY_LIMIT: u32 = 3;
const TRANSIENT_RETRY_DELAY: Duration = Duration::from_millis(500);

#[derive(Clone)]
pub(crate) struct WorkspaceSession {
    transport: ProcessTransport,
    documents: DocumentLifecycle,
    diagnostics: DiagnosticsStore,
    refresh: RefreshQueue,
    diagnostics_mode: DiagnosticsMode,
    supported_extensions: Arc<HashSet<String>>,
    alive: Arc<AtomicBool>,
}

impl WorkspaceSession {
    pub(crate) fn spawn(workspace_root: &Path, config: &LspConfig) -> DaemonResult<Self> {
        let (transport, event_rx) = ProcessTransport::spawn(workspace_root, config)?;
        let session = Self {
            transport,
            documents: DocumentLifecycle::new(),
            diagnostics: DiagnosticsStore::new(),
            refresh: RefreshQueue::new(),
            diagnostics_mode: config.diagnostics_mode,
            supported_extensions: Arc::new(supported_extensions(config)),
            alive: Arc::new(AtomicBool::new(true)),
        };

        tokio::spawn(session.clone().run_events(event_rx));
        tokio::spawn(session.clone().run_refresh_worker());
        tokio::spawn(session.clone().bootstrap_refresh(workspace_root.to_path_buf()));

        Ok(session)
    }

    pub(crate) async fn wait_until_initialized(&self) -> DaemonResult<()> {
        self.transport.wait_until_initialized().await
    }

    pub(crate) async fn request(&self, method: &str, params: &Value) -> Result<Value, TransportError> {
        let Some(uri) = extract_document_uri(method, params) else {
            return self.request_with_retry(method, params).await;
        };

        self.open_document(&uri).await;
        let result = self.request_with_retry(method, params).await;
        self.close_document(&uri).await;
        result
    }

    pub(crate) fn queue_diagnostic_refresh(&self, uri: Uri) {
        self.refresh.enqueue(vec![uri]);
    }

    pub(crate) async fn get_diagnostics(&self, uri: Option<&Uri>) -> Vec<PublishDiagnosticsParams> {
        match uri {
            Some(uri) => {
                if self.refresh_uri(uri).await == Freshness::Unconfirmed {
                    self.refresh.wait_for_current_generation(DIAGNOSTICS_TIMEOUT).await;
                }
            }
            None => self.refresh.wait_for_current_generation(BACKGROUND_REFRESH_TIMEOUT).await,
        }
        self.diagnostics.get(uri)
    }

    pub(crate) async fn shutdown(&self) {
        self.refresh.shutdown();
        self.transport.shutdown().await;
    }

    /// Whether the language server behind this session is still usable.
    pub(crate) fn is_alive(&self) -> bool {
        self.alive.load(Ordering::SeqCst)
    }

    /// Mark this session dead so the registry replaces it on the next request.
    pub(crate) fn mark_dead(&self) {
        self.alive.store(false, Ordering::SeqCst);
    }

    /// Declare the language server wedged: mark the session dead and kill the
    /// server process so blocked pipes unwind and pending requests fail.
    pub(crate) fn declare_wedged(&self) {
        if self.alive.swap(false, Ordering::SeqCst) {
            self.transport.kill_process();
        }
    }

    async fn refresh_uri(&self, uri: &Uri) -> Freshness {
        let sync = self.open_document(uri).await;
        let freshness = self.settle_diagnostics(uri, sync).await;
        self.close_document(uri).await;
        freshness
    }

    async fn open_document(&self, uri: &Uri) -> DocumentSync {
        let notifications = match self.documents.acquire(uri).await {
            AcquireAction::Open { file_path, content } => open_and_save_notifications(uri, &file_path, content),
            AcquireAction::Reopen { file_path, content } => reopen_notifications(uri, &file_path, content),
            AcquireAction::Unchanged => return DocumentSync::AlreadyOpen,
            AcquireAction::MissingOnDisk => {
                self.documents.forget_uri(uri);
                self.diagnostics.forget_uri(uri);
                return DocumentSync::Missing;
            }
        };

        let version_before = self.diagnostics.current_uri_version(uri);
        for notification in notifications {
            self.transport.send_notification(notification).await;
        }
        DocumentSync::Sent { version_before }
    }

    async fn close_document(&self, uri: &Uri) {
        match self.documents.release(uri) {
            ReleaseAction::Close => self.transport.send_notification(close_notification(uri)).await,
            ReleaseAction::CloseAndRefresh => {
                self.transport.send_notification(close_notification(uri)).await;
                self.refresh.enqueue(vec![uri.clone()]);
            }
            ReleaseAction::Unchanged => {}
        }
    }

    async fn settle_diagnostics(&self, uri: &Uri, sync: DocumentSync) -> Freshness {
        match (self.diagnostics_mode, sync) {
            (_, DocumentSync::Missing) | (DiagnosticsMode::Push, DocumentSync::AlreadyOpen) => Freshness::Unconfirmed,
            (DiagnosticsMode::Push, DocumentSync::Sent { version_before }) => {
                self.diagnostics.wait_for_uri_fresh(uri, version_before, DIAGNOSTICS_TIMEOUT).await;
                Freshness::Fresh
            }
            (DiagnosticsMode::Pull, DocumentSync::Sent { .. } | DocumentSync::AlreadyOpen) => {
                self.pull_diagnostics(uri).await
            }
        }
    }

    async fn pull_diagnostics(&self, uri: &Uri) -> Freshness {
        // Built by hand: lsp-types serializes `identifier` and `previousResultId` as explicit nulls,
        // which TypeScript 7 rejects.
        let params = serde_json::json!({ "textDocument": TextDocumentIdentifier { uri: uri.clone() } });
        let request = self.request_with_retry(DocumentDiagnosticRequest::METHOD, &params);
        let report = tokio::time::timeout(DIAGNOSTICS_TIMEOUT, request)
            .await
            .ok()
            .and_then(Result::ok)
            .and_then(|value| serde_json::from_value(value).ok());

        let Some(DocumentDiagnosticReportResult::Report(DocumentDiagnosticReport::Full(report))) = report else {
            tracing::warn!(uri = uri.as_str(), "Language server returned no full diagnostic report");
            return Freshness::Unconfirmed;
        };
        self.diagnostics.publish(PublishDiagnosticsParams {
            uri: uri.clone(),
            diagnostics: report.full_document_diagnostic_report.items,
            version: None,
        });
        Freshness::Fresh
    }

    async fn request_with_retry(&self, method: &str, params: &Value) -> Result<Value, TransportError> {
        let mut retries = 0;
        loop {
            match self.transport.request_raw(method, params.clone()).await {
                Err(TransportError::Lsp(err))
                    if matches!(err.code, LSP_CONTENT_MODIFIED | LSP_SERVER_CANCELLED)
                        && retries < TRANSIENT_RETRY_LIMIT =>
                {
                    retries += 1;
                    tokio::time::sleep(TRANSIENT_RETRY_DELAY).await;
                }
                result => return result,
            }
        }
    }

    async fn run_events(self, mut event_rx: mpsc::Receiver<TransportEvent>) {
        while let Some(event) = event_rx.recv().await {
            match event {
                TransportEvent::PublishedDiagnostics(params) => self.diagnostics.publish(params),
                TransportEvent::DiagnosticsRefreshRequested => {
                    self.refresh.enqueue(self.filter_supported(self.diagnostics.uris()));
                }
                TransportEvent::FileWatcherBatch(batch) => self.forward_watcher_batch(batch).await,
                TransportEvent::Closed => break,
            }
        }

        self.mark_dead();
        self.refresh.shutdown();
    }

    async fn forward_watcher_batch(&self, batch: FileWatcherBatch) {
        let changes = self.documents.filter_watcher_changes(batch.forwarded_changes);
        let mut refresh_uris = self.filter_supported(changes.iter().map(|change| change.uri.clone()).collect());
        refresh_uris.extend(self.filter_supported(batch.discovered_uris));
        self.refresh.enqueue(refresh_uris);

        if changes.is_empty() {
            return;
        }

        let params = DidChangeWatchedFilesParams { changes };
        if let Ok(value) = serde_json::to_value(&params) {
            self.transport
                .send_notification(LspNotification { method: DidChangeWatchedFiles::METHOD.to_string(), params: value })
                .await;
        }
    }

    async fn run_refresh_worker(self) {
        while let Some(uri) = self.refresh.recv().await {
            self.refresh_uri(&uri).await;
        }
    }

    async fn bootstrap_refresh(self, workspace_root: PathBuf) {
        let supported_extensions = Arc::clone(&self.supported_extensions);
        let uris = tokio::task::spawn_blocking(move || discover_supported_uris(&workspace_root, &supported_extensions))
            .await
            .unwrap_or_default();

        self.refresh.enqueue(uris);
        self.refresh.complete_bootstrap();
    }

    fn filter_supported(&self, uris: Vec<Uri>) -> Vec<Uri> {
        uris.into_iter().filter(|uri| uri_is_supported(uri, &self.supported_extensions)).collect()
    }
}

enum DocumentSync {
    Sent { version_before: u64 },
    AlreadyOpen,
    Missing,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Freshness {
    Fresh,
    Unconfirmed,
}

fn supported_extensions(config: &LspConfig) -> HashSet<String> {
    config
        .languages
        .iter()
        .filter_map(|language| metadata_for(*language))
        .flat_map(|metadata| metadata.extensions.iter().copied())
        .map(ToOwned::to_owned)
        .collect()
}

fn discover_supported_uris(workspace_root: &Path, supported_extensions: &HashSet<String>) -> Vec<Uri> {
    if supported_extensions.is_empty() {
        return Vec::new();
    }

    let mut builder = WalkBuilder::new(workspace_root);
    builder
        .standard_filters(true)
        .filter_entry(|entry| entry.depth() == 0 || !is_ignored_directory_name(entry.file_name()));

    builder
        .build()
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_some_and(|file_type| file_type.is_file()))
        .filter(|entry| path_is_supported(entry.path(), supported_extensions))
        .filter_map(|entry| crate::path_to_uri(entry.path()).ok())
        .collect()
}

fn uri_is_supported(uri: &Uri, supported_extensions: &HashSet<String>) -> bool {
    let path = crate::uri_to_path(uri);
    path_is_supported(Path::new(&path), supported_extensions)
}

fn is_ignored_directory_name(name: &OsStr) -> bool {
    matches!(name.to_string_lossy().as_ref(), ".git" | "node_modules" | ".next" | "dist" | "build" | "target")
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
