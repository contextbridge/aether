use crate::diagnostics_store::DiagnosticsStore;
use crate::document_lifecycle::{AcquireAction, DocumentLifecycle, ReleaseAction};
use crate::language_catalog::LanguageId;
use crate::process_transport::{ProcessTransport, TransportError, TransportEvent};
use crate::protocol::LspNotification;
use crate::refresh_queue::RefreshQueue;
use ignore::WalkBuilder;
use lsp_types::notification::{
    DidChangeWatchedFiles, DidCloseTextDocument, DidOpenTextDocument, DidSaveTextDocument, Notification,
};
use lsp_types::request::DocumentDiagnosticRequest;
use lsp_types::request::Request as _;
use lsp_types::{
    Diagnostic, DidChangeWatchedFilesParams, DidCloseTextDocumentParams, DidOpenTextDocumentParams,
    DidSaveTextDocumentParams, DocumentDiagnosticReport, DocumentDiagnosticReportKind, DocumentDiagnosticReportResult,
    PublishDiagnosticsParams, TextDocumentIdentifier, TextDocumentItem, Uri,
};
use serde_json::Value;
use std::collections::HashMap;
use std::collections::HashSet;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use tokio::sync::mpsc;

const DIAGNOSTICS_TIMEOUT: Duration = Duration::from_secs(20);
const BACKGROUND_REFRESH_TIMEOUT: Duration = Duration::from_secs(20);

pub(crate) struct WorkspaceSession {
    transport: ProcessTransport,
    documents: DocumentLifecycle,
    diagnostics: DiagnosticsStore,
    refresh: RefreshQueue,
    alive: Arc<AtomicBool>,
    pull_support: PullSupport,
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
        let alive = Arc::new(AtomicBool::new(true));

        let pull_support = PullSupport::default();
        let session = Self {
            transport,
            documents,
            diagnostics,
            refresh,
            alive: Arc::clone(&alive),
            pull_support: pull_support.clone(),
        };
        let supported_extensions = Arc::new(supported_extensions);

        tokio::spawn(run_session_events(
            session.transport.clone(),
            session.documents.clone(),
            session.diagnostics.clone(),
            session.refresh.clone(),
            Arc::clone(&supported_extensions),
            event_rx,
            alive,
        ));

        tokio::spawn(run_background_refresh_worker(
            session.transport.clone(),
            session.documents.clone(),
            session.diagnostics.clone(),
            session.refresh.clone(),
            pull_support.clone(),
        ));

        tokio::spawn(bootstrap_workspace_refresh(
            workspace_root.to_path_buf(),
            supported_extensions,
            session.refresh.clone(),
        ));

        Ok(session)
    }

    pub(crate) async fn request_raw(&self, method: &str, params: Value) -> Result<Value, TransportError> {
        self.transport.request_raw(method, params).await
    }

    pub(crate) fn queue_diagnostic_refresh(&self, uri: Uri) {
        self.refresh.enqueue(vec![uri]);
    }

    pub(crate) async fn ensure_document_open(&self, uri: &Uri) -> Option<u64> {
        sync_document(&self.transport, &self.documents, &self.diagnostics, uri).await
    }

    pub(crate) async fn close_document(&self, uri: &Uri) {
        release_document(&self.transport, &self.documents, &self.refresh, uri).await;
    }

    pub(crate) async fn get_diagnostics(&self, uri: Option<&Uri>) -> Vec<PublishDiagnosticsParams> {
        self.sync_documents_for_diagnostics(uri).await;
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

    async fn sync_documents_for_diagnostics(&self, uri: Option<&Uri>) {
        let Some(uri) = uri else {
            self.refresh.wait_for_current_generation(BACKGROUND_REFRESH_TIMEOUT).await;
            return;
        };

        let version_before = self.ensure_document_open(uri).await;
        let pulled = version_before.is_some()
            && pull_diagnostics(&self.transport, &self.diagnostics, &self.pull_support, uri).await;
        if !pulled {
            if let Some(version_before) = version_before {
                self.diagnostics.wait_for_uri_fresh(uri, version_before, DIAGNOSTICS_TIMEOUT).await;
            } else {
                self.refresh.wait_for_current_generation(DIAGNOSTICS_TIMEOUT).await;
            }
        }
        self.close_document(uri).await;
    }
}

async fn pull_diagnostics(
    transport: &ProcessTransport,
    diagnostics: &DiagnosticsStore,
    pull_support: &PullSupport,
    uri: &Uri,
) -> bool {
    if pull_support.is_unsupported() {
        return false;
    }
    let params = serde_json::json!({"textDocument": {"uri": uri}});
    let value = match transport.request_raw(DocumentDiagnosticRequest::METHOD, params).await {
        Ok(value) => value,
        Err(TransportError::Lsp(err)) if err.code == METHOD_NOT_FOUND => {
            pull_support.mark_unsupported();
            return false;
        }
        Err(TransportError::Lsp(err)) => {
            tracing::debug!(code = err.code, "Pull diagnostics failed, using push cache");
            return false;
        }
        Err(TransportError::Closed) => return false,
    };
    let report = match serde_json::from_value::<DocumentDiagnosticReportResult>(value) {
        Ok(report) => report,
        Err(err) => {
            tracing::debug!(%err, "Ignoring malformed pull diagnostics report");
            return false;
        }
    };
    let Some(pulled) = convert_pull_report(report) else {
        return false;
    };
    diagnostics.publish(PublishDiagnosticsParams { uri: uri.clone(), diagnostics: pulled.primary, version: None });
    for (related_uri, related_diagnostics) in pulled.related {
        diagnostics.publish(PublishDiagnosticsParams {
            uri: related_uri,
            diagnostics: related_diagnostics,
            version: None,
        });
    }
    true
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
            documents.forget_uri(uri);
            diagnostics.forget_uri(uri);
            return None;
        }
    };

    let version_before = diagnostics.current_uri_version(uri);
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
    match documents.release(uri) {
        ReleaseAction::Close => {
            transport.send_notification(close_notification(uri)).await;
        }
        ReleaseAction::CloseAndRefresh => {
            transport.send_notification(close_notification(uri)).await;
            refresh.enqueue(vec![uri.clone()]);
        }
        ReleaseAction::Unchanged => {}
    }
}

async fn run_background_refresh_worker(
    transport: ProcessTransport,
    documents: DocumentLifecycle,
    diagnostics: DiagnosticsStore,
    refresh: RefreshQueue,
    pull_support: PullSupport,
) {
    while let Some(uri) = refresh.recv().await {
        refresh_uri(&transport, &documents, &diagnostics, &refresh, &pull_support, &uri).await;
    }
}

async fn refresh_uri(
    transport: &ProcessTransport,
    documents: &DocumentLifecycle,
    diagnostics: &DiagnosticsStore,
    refresh: &RefreshQueue,
    pull_support: &PullSupport,
    uri: &Uri,
) {
    let sync_result = sync_document(transport, documents, diagnostics, uri).await;
    if let Some(version_before) = sync_result {
        pull_diagnostics(transport, diagnostics, pull_support, uri).await;
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
            builder
                .standard_filters(true)
                .filter_entry(|entry| entry.depth() == 0 || !is_ignored_directory_name(entry.file_name()));

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

    refresh.enqueue(uris);
    refresh.complete_bootstrap();
}

async fn run_session_events(
    transport: ProcessTransport,
    documents: DocumentLifecycle,
    diagnostics: DiagnosticsStore,
    refresh: RefreshQueue,
    supported_extensions: Arc<HashSet<String>>,
    mut event_rx: mpsc::Receiver<TransportEvent>,
    alive: Arc<AtomicBool>,
) {
    while let Some(event) = event_rx.recv().await {
        match event {
            TransportEvent::PublishedDiagnostics(params) => {
                diagnostics.publish(params);
            }
            TransportEvent::FileWatcherBatch(batch) => {
                let filtered = documents.filter_watcher_changes(batch.forwarded_changes);
                let discovered = filter_supported_uris(batch.discovered_uris, supported_extensions.as_ref());

                let mut refresh_uris = filter_supported_uris(
                    filtered.iter().map(|change| change.uri.clone()).collect(),
                    supported_extensions.as_ref(),
                );
                refresh_uris.extend(discovered);
                refresh.enqueue(refresh_uris);

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
            TransportEvent::DiagnosticRefreshRequested => {
                refresh.enqueue(documents.open_uris());
            }
            TransportEvent::Closed => break,
        }
    }

    alive.store(false, Ordering::SeqCst);
    refresh.shutdown();
}

fn filter_supported_uris(uris: Vec<Uri>, supported_extensions: &HashSet<String>) -> Vec<Uri> {
    uris.into_iter().filter(|uri| uri_is_supported(uri, supported_extensions)).collect()
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

const METHOD_NOT_FOUND: i32 = -32601;

/// Whether the session's server has rejected `textDocument/diagnostic`, making
/// further pull probes pointless.
#[derive(Clone, Default)]
struct PullSupport(Arc<AtomicBool>);

impl PullSupport {
    fn is_unsupported(&self) -> bool {
        self.0.load(Ordering::SeqCst)
    }

    fn mark_unsupported(&self) {
        self.0.store(true, Ordering::SeqCst);
    }
}

struct PulledDiagnostics {
    primary: Vec<Diagnostic>,
    related: Vec<(Uri, Vec<Diagnostic>)>,
}

fn convert_pull_report(report: DocumentDiagnosticReportResult) -> Option<PulledDiagnostics> {
    match report {
        DocumentDiagnosticReportResult::Report(DocumentDiagnosticReport::Full(report)) => Some(PulledDiagnostics {
            primary: report.full_document_diagnostic_report.items,
            related: flatten_related(report.related_documents),
        }),
        DocumentDiagnosticReportResult::Report(DocumentDiagnosticReport::Unchanged(_)) => None,
        DocumentDiagnosticReportResult::Partial(partial) => {
            Some(PulledDiagnostics { primary: Vec::new(), related: flatten_related(partial.related_documents) })
        }
    }
}

fn flatten_related(related: Option<HashMap<Uri, DocumentDiagnosticReportKind>>) -> Vec<(Uri, Vec<Diagnostic>)> {
    related
        .unwrap_or_default()
        .into_iter()
        .filter_map(|(uri, kind)| match kind {
            DocumentDiagnosticReportKind::Full(report) => Some((uri, report.items)),
            DocumentDiagnosticReportKind::Unchanged(_) => None,
        })
        .collect()
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

    #[test]
    fn convert_pull_report_full_with_items() {
        let related_uri: Uri = "file:///workspace/other.ts".parse().unwrap();
        let value = serde_json::json!({
            "kind": "full",
            "items": [{"range": {"start": {"line": 0, "character": 0}, "end": {"line": 0, "character": 5}}, "severity": 1, "message": "boom"}],
            "relatedDocuments": {
                related_uri.as_str(): {"kind": "full", "items": [{"range": {"start": {"line": 1, "character": 0}, "end": {"line": 1, "character": 2}}, "message": "related"}]},
                "file:///workspace/stale.ts": {"kind": "unchanged", "resultId": "abc"}
            }
        });
        let report: DocumentDiagnosticReportResult = serde_json::from_value(value).unwrap();
        let pulled = convert_pull_report(report).unwrap();

        assert_eq!(pulled.primary.len(), 1);
        assert_eq!(pulled.primary[0].message, "boom");
        assert_eq!(pulled.related.len(), 1);
        assert_eq!(pulled.related[0].0, related_uri);
    }

    #[test]
    fn convert_pull_report_full_empty_clears_stale_errors() {
        let value = serde_json::json!({"kind": "full", "items": []});
        let report: DocumentDiagnosticReportResult = serde_json::from_value(value).unwrap();
        let pulled = convert_pull_report(report).unwrap();

        assert!(pulled.primary.is_empty());
        assert!(pulled.related.is_empty());
    }

    #[test]
    fn convert_pull_report_unchanged_keeps_cache() {
        let value = serde_json::json!({"kind": "unchanged", "resultId": "abc"});
        let report: DocumentDiagnosticReportResult = serde_json::from_value(value).unwrap();

        assert!(convert_pull_report(report).is_none());
    }
}
