use crate::error::{DaemonError, DaemonResult};
use crate::file_watcher::{FileWatcherBatch, FileWatcherHandle};
use crate::language_catalog::{DiagnosticsMode, LspConfig};
use crate::path_to_uri;
use crate::protocol::LspErrorResponse;
use futures::{SinkExt, StreamExt};
use lsp_types::notification::{DidChangeWatchedFiles, Initialized, Notification, PublishDiagnostics};
use lsp_types::request::{
    Initialize, RegisterCapability, Request, UnregisterCapability, WorkDoneProgressCreate, WorkspaceDiagnosticRefresh,
};
use lsp_types::{
    ClientCapabilities, DiagnosticClientCapabilities, DidChangeWatchedFilesClientCapabilities,
    GeneralClientCapabilities, GotoCapability, HoverClientCapabilities, InitializeParams, MarkupKind,
    PublishDiagnosticsClientCapabilities, PublishDiagnosticsParams, RegistrationParams, TextDocumentClientCapabilities,
    WorkspaceClientCapabilities, WorkspaceFolder,
};
use lsp_types::{DocumentSymbolClientCapabilities, DynamicRegistrationClientCapabilities};
#[cfg(unix)]
use nix::sys::signal::{Signal, kill};
#[cfg(unix)]
use nix::unistd::Pid;
use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::io::{self, ErrorKind};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;
use tokio::process::{Child, ChildStdin, ChildStdout, Command};
use tokio::sync::{mpsc, oneshot, watch};
use tokio_util::bytes::{Buf, BytesMut};
use tokio_util::codec::{Decoder, Encoder, FramedRead, FramedWrite};

const INITIALIZE_REQUEST_ID: i64 = 1;
const SERVER_STATUS_METHOD: &str = "experimental/serverStatus";

#[derive(Clone)]
pub(crate) struct ProcessTransport {
    command_tx: mpsc::Sender<TransportCommand>,
    process_id: Option<i32>,
    initialized_rx: watch::Receiver<bool>,
    ready: Arc<watch::Sender<bool>>,
}

/// A notification the daemon sends to its language server.
#[derive(Debug, Clone)]
pub(crate) struct LspNotification {
    pub(crate) method: String,
    pub(crate) params: Value,
}

pub(crate) enum TransportEvent {
    #[cfg(feature = "testing")]
    RequestReceived(String),
    PublishedDiagnostics(PublishDiagnosticsParams),
    DiagnosticsRefreshRequested,
    FileWatcherBatch(FileWatcherBatch),
    Closed,
}

/// Failure of a raw LSP request, typed so callers can react to a dead
/// transport (replace the server) without inspecting server-defined codes.
pub(crate) enum TransportError {
    /// The language server answered with an error response; forwarded verbatim.
    Lsp(LspErrorResponse),
    /// The language server process exited or its transport shut down before
    /// responding.
    Closed,
}

struct TransportRequest {
    method: String,
    params: Value,
    response_tx: oneshot::Sender<Result<Value, TransportError>>,
}

enum TransportCommand {
    Request(TransportRequest),
    Notification(LspNotification),
    Shutdown,
}

struct ProcessTransportActor {
    process: Child,
    reader: FramedRead<ChildStdout, LspCodec>,
    writer: FramedWrite<ChildStdin, LspCodec>,
    diagnostics_mode: DiagnosticsMode,
    initialized_tx: watch::Sender<bool>,
    ready: Arc<watch::Sender<bool>>,
    command_rx: mpsc::Receiver<TransportCommand>,
    event_tx: mpsc::Sender<TransportEvent>,
    watcher: FileWatcherHandle,
    watcher_rx: mpsc::Receiver<FileWatcherBatch>,
    next_id: i64,
    pending: HashMap<i64, oneshot::Sender<Result<Value, TransportError>>>,
}

impl ProcessTransport {
    pub(crate) fn spawn(root_path: &Path, config: &LspConfig) -> DaemonResult<(Self, mpsc::Receiver<TransportEvent>)> {
        let resolved_command = resolve_command(root_path, &config.command);
        let mut process = Command::new(&resolved_command)
            .current_dir(root_path)
            .args(&config.args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| DaemonError::LspSpawnFailed(format!("{}: {e}", resolved_command.display())))?;

        let stdin =
            process.stdin.take().ok_or_else(|| DaemonError::LspSpawnFailed("Failed to capture stdin".into()))?;
        let stdout =
            process.stdout.take().ok_or_else(|| DaemonError::LspSpawnFailed("Failed to capture stdout".into()))?;
        let process_id = process.id().and_then(|id| i32::try_from(id).ok());

        let (command_tx, command_rx) = mpsc::channel(100);
        let (event_tx, event_rx) = mpsc::channel(100);
        let (watcher_tx, watcher_rx) = mpsc::channel(64);
        let (initialized_tx, initialized_rx) = watch::channel(false);
        let ready = Arc::new(watch::Sender::new(!config.reports_server_status));
        let watcher = FileWatcherHandle::spawn(root_path.to_path_buf(), watcher_tx);

        let actor = ProcessTransportActor {
            process,
            reader: FramedRead::new(stdout, LspCodec),
            writer: FramedWrite::new(stdin, LspCodec),
            diagnostics_mode: config.diagnostics_mode,
            initialized_tx,
            ready: Arc::clone(&ready),
            command_rx,
            event_tx,
            watcher,
            watcher_rx,
            next_id: INITIALIZE_REQUEST_ID + 1,
            pending: HashMap::new(),
        };
        tokio::spawn(actor.run(root_path.to_path_buf()));

        Ok((Self { command_tx, process_id, initialized_rx, ready }, event_rx))
    }

    pub(crate) async fn wait_until_initialized(&self) -> DaemonResult<()> {
        let mut initialized_rx = self.initialized_rx.clone();
        initialized_rx
            .wait_for(|initialized| *initialized)
            .await
            .map(|_| ())
            .map_err(|_| DaemonError::LspInitializeFailed)
    }

    pub(crate) async fn wait_until_ready(&self, bound: Duration) {
        let mut ready = self.ready.subscribe();
        if tokio::time::timeout(bound, ready.wait_for(|ready| *ready)).await.is_err() {
            tracing::warn!("Language server still loading after {}s; sending requests anyway", bound.as_secs());
            self.ready.send_replace(true);
        }
    }

    pub(crate) async fn request_raw(&self, method: &str, params: Value) -> Result<Value, TransportError> {
        let (response_tx, response_rx) = oneshot::channel();
        let request = TransportRequest { method: method.to_string(), params, response_tx };
        self.command_tx.send(TransportCommand::Request(request)).await.map_err(|_| TransportError::Closed)?;

        response_rx.await.map_err(|_| TransportError::Closed)?
    }

    pub(crate) async fn send_notification(&self, notification: LspNotification) {
        let _ = self.command_tx.send(TransportCommand::Notification(notification)).await;
    }

    pub(crate) async fn shutdown(&self) {
        let _ = self.command_tx.send(TransportCommand::Shutdown).await;
    }

    /// Forcibly terminate the language server process, bypassing the actor.
    ///
    /// Used when the server stops responding: the actor may be blocked on the
    /// server's pipes, so a cooperative `Shutdown` command would never be
    /// processed. Killing the process closes its pipes, which unblocks the
    /// actor and fails all pending requests.
    pub(crate) fn kill_process(&self) {
        #[cfg(unix)]
        if let Some(pid) = self.process_id {
            let _ = kill(Pid::from_raw(pid), Signal::SIGKILL);
        }
    }
}

impl ProcessTransportActor {
    async fn run(mut self, root_path: PathBuf) {
        if let Err(err) = self.initialize(&root_path).await {
            tracing::error!(%err, "Failed to initialize LSP transport");
            let _ = self.process.kill().await;
            self.cleanup_pending();
            let _ = self.event_tx.send(TransportEvent::Closed).await;
            return;
        }
        self.initialized_tx.send_replace(true);

        loop {
            tokio::select! {
                message = self.reader.next() => match message {
                    Some(Ok(message)) => self.handle_lsp_message(message).await,
                    Some(Err(err)) => tracing::warn!(%err, "Error reading LSP message"),
                    None => break,
                },
                Some(command) = self.command_rx.recv() => {
                    if !self.handle_command(command).await {
                        break;
                    }
                }
                Some(batch) = self.watcher_rx.recv() => {
                    if self.event_tx.send(TransportEvent::FileWatcherBatch(batch)).await.is_err() {
                        break;
                    }
                }
                _ = self.process.wait() => break,
            }
        }

        self.cleanup_pending();
        let _ = self.event_tx.send(TransportEvent::Closed).await;
    }

    async fn handle_command(&mut self, command: TransportCommand) -> bool {
        match command {
            TransportCommand::Request(request) => {
                let id = self.next_id;
                self.next_id += 1;
                self.pending.insert(id, request.response_tx);
                if let Err(err) = self.send_request(id, &request.method, request.params).await
                    && let Some(tx) = self.pending.remove(&id)
                {
                    tracing::warn!(%err, "Failed to write LSP request");
                    let _ = tx.send(Err(TransportError::Closed));
                }
                true
            }
            TransportCommand::Notification(notification) => {
                if let Err(err) = self.send_notification(&notification.method, notification.params).await {
                    tracing::warn!(%err, "Failed to forward LSP notification");
                }
                true
            }
            TransportCommand::Shutdown => {
                let _ = self.process.kill().await;
                false
            }
        }
    }

    async fn initialize(&mut self, root_path: &Path) -> io::Result<()> {
        let root_uri = crate::path_to_uri(root_path).map_err(|err| io::Error::new(ErrorKind::InvalidInput, err))?;
        let pull_diagnostics = self.diagnostics_mode == DiagnosticsMode::Pull;

        let capabilities = ClientCapabilities {
            general: Some(GeneralClientCapabilities::default()),
            text_document: Some(TextDocumentClientCapabilities {
                publish_diagnostics: Some(PublishDiagnosticsClientCapabilities {
                    related_information: Some(true),
                    ..Default::default()
                }),
                definition: Some(GotoCapability { dynamic_registration: Some(false), link_support: Some(true) }),
                references: Some(DynamicRegistrationClientCapabilities { dynamic_registration: Some(false) }),
                hover: Some(HoverClientCapabilities {
                    dynamic_registration: Some(false),
                    content_format: Some(vec![MarkupKind::Markdown, MarkupKind::PlainText]),
                }),
                document_symbol: Some(DocumentSymbolClientCapabilities {
                    hierarchical_document_symbol_support: Some(true),
                    ..Default::default()
                }),
                diagnostic: pull_diagnostics.then_some(DiagnosticClientCapabilities {
                    dynamic_registration: Some(false),
                    related_document_support: Some(false),
                }),
                ..Default::default()
            }),
            workspace: Some(WorkspaceClientCapabilities {
                did_change_watched_files: Some(DidChangeWatchedFilesClientCapabilities {
                    dynamic_registration: Some(true),
                    relative_pattern_support: Some(false),
                }),
                ..Default::default()
            }),
            experimental: Some(json!({ "serverStatusNotification": true })),
            ..Default::default()
        };

        let params = InitializeParams {
            process_id: Some(std::process::id()),
            #[allow(deprecated)]
            root_uri: Some(root_uri),
            workspace_folders: Some(vec![WorkspaceFolder {
                uri: path_to_uri(root_path).map_err(|err| io::Error::new(ErrorKind::InvalidInput, err))?,
                name: root_path.file_name().and_then(|name| name.to_str()).unwrap_or("workspace").to_string(),
            }]),
            capabilities,
            ..Default::default()
        };
        let mut params = serde_json::to_value(&params).unwrap();
        if pull_diagnostics {
            // lsp-types serializes this as `workspace.diagnostic`, but the spec (and servers) read `workspace.diagnostics`.
            params["capabilities"]["workspace"]["diagnostics"] = json!({ "refreshSupport": true });
        }

        self.send_request(INITIALIZE_REQUEST_ID, Initialize::METHOD, params).await?;

        while let Some(message) = self.reader.next().await {
            let message = match message {
                Ok(message) => message,
                Err(err) => {
                    tracing::warn!(%err, "Error reading LSP message");
                    continue;
                }
            };
            if message.id.as_ref().and_then(Value::as_i64) == Some(INITIALIZE_REQUEST_ID) {
                if let Some(error) = message.error {
                    return Err(io::Error::other(format!(
                        "server rejected `initialize`: {} (code {})",
                        error.message, error.code
                    )));
                }
                return self.send_notification(Initialized::METHOD, json!({})).await;
            }

            self.handle_lsp_message(message).await;
        }

        Err(io::Error::new(ErrorKind::UnexpectedEof, "LSP closed during initialization"))
    }

    async fn handle_lsp_message(&mut self, message: Incoming) {
        match (message.id, message.method) {
            (Some(id), Some(method)) => match method.as_str() {
                RegisterCapability::METHOD => self.handle_register_capability(&id, message.params).await,
                UnregisterCapability::METHOD => self.handle_unregister_capability(&id, message.params).await,
                WorkDoneProgressCreate::METHOD => {
                    let _ = self.send_ok_response(&id).await;
                }
                WorkspaceDiagnosticRefresh::METHOD => {
                    let _ = self.send_ok_response(&id).await;
                    let _ = self.event_tx.send(TransportEvent::DiagnosticsRefreshRequested).await;
                }
                _ => {
                    let error = json!({ "code": -32601, "message": format!("Method not found: {method}") });
                    let _ = self.writer.send(json!({ "jsonrpc": "2.0", "id": id, "error": error })).await;
                }
            },
            (Some(id), None) => {
                if let Some(tx) = id.as_i64().and_then(|id| self.pending.remove(&id)) {
                    let _ = tx.send(message.error.map_or(Ok(message.result), |error| Err(TransportError::Lsp(error))));
                }
            }
            (None, Some(method)) if method == PublishDiagnostics::METHOD => {
                if let Ok(diagnostics) = serde_json::from_value(message.params) {
                    let _ = self.event_tx.send(TransportEvent::PublishedDiagnostics(diagnostics)).await;
                }
            }
            #[cfg(feature = "testing")]
            (None, Some(method)) if method == "aether/testing/requestReceived" => {
                if let Some(method) = message.params.get("method").and_then(Value::as_str) {
                    let _ = self.event_tx.send(TransportEvent::RequestReceived(method.to_owned())).await;
                }
            }
            (None, Some(method)) if method == SERVER_STATUS_METHOD => {
                if let Some(quiescent) = message.params.get("quiescent").and_then(Value::as_bool) {
                    self.ready.send_replace(quiescent);
                }
            }
            _ => {}
        }
    }

    async fn handle_register_capability(&mut self, id: &Value, params: Value) {
        if let Ok(registration_params) = serde_json::from_value::<RegistrationParams>(params) {
            for registration in &registration_params.registrations {
                if registration.method == DidChangeWatchedFiles::METHOD
                    && let Some(options) = &registration.register_options
                    && let Ok(watchers) = parse_file_system_watchers(options)
                {
                    self.watcher.register_watchers(registration.id.clone(), watchers);
                }
            }
        }

        let _ = self.send_ok_response(id).await;
    }

    async fn handle_unregister_capability(&mut self, id: &Value, params: Value) {
        if let Ok(unregistration_params) = serde_json::from_value::<lsp_types::UnregistrationParams>(params) {
            for registration in &unregistration_params.unregisterations {
                if registration.method == DidChangeWatchedFiles::METHOD {
                    self.watcher.unregister(registration.id.clone());
                }
            }
        }

        let _ = self.send_ok_response(id).await;
    }

    async fn send_request(&mut self, id: i64, method: &str, params: Value) -> io::Result<()> {
        self.writer.send(json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params })).await
    }

    async fn send_notification(&mut self, method: &str, params: Value) -> io::Result<()> {
        self.writer.send(json!({ "jsonrpc": "2.0", "method": method, "params": params })).await
    }

    async fn send_ok_response(&mut self, id: &Value) -> io::Result<()> {
        self.writer.send(json!({ "jsonrpc": "2.0", "id": id, "result": null })).await
    }

    fn cleanup_pending(&mut self) {
        for (_, tx) in self.pending.drain() {
            let _ = tx.send(Err(TransportError::Closed));
        }
    }
}

fn parse_file_system_watchers(opts: &Value) -> Result<Vec<lsp_types::FileSystemWatcher>, serde_json::Error> {
    #[derive(serde::Deserialize)]
    struct WatcherOptions {
        watchers: Vec<lsp_types::FileSystemWatcher>,
    }

    let parsed: WatcherOptions = serde_json::from_value(opts.clone())?;
    Ok(parsed.watchers)
}

fn resolve_command(root_path: &Path, command: &str) -> PathBuf {
    let command_path = Path::new(command);
    if command_path.is_absolute() || command_path.components().count() > 1 {
        return command_path.to_path_buf();
    }

    let local_command = root_path.join("node_modules").join(".bin").join(command);
    if local_command.exists() { local_command } else { command_path.to_path_buf() }
}

#[derive(Deserialize)]
struct Incoming {
    id: Option<Value>,
    method: Option<String>,
    #[serde(default)]
    params: Value,
    #[serde(default)]
    result: Value,
    error: Option<LspErrorResponse>,
}

struct LspCodec;

impl Decoder for LspCodec {
    type Item = Incoming;
    type Error = io::Error;

    fn decode(&mut self, src: &mut BytesMut) -> io::Result<Option<Incoming>> {
        loop {
            let Some(header_len) = src.windows(4).position(|window| window == b"\r\n\r\n") else {
                return Ok(None);
            };
            let body_start = header_len + 4;
            let content_length = String::from_utf8_lossy(&src[..header_len])
                .lines()
                .find_map(|line| line.strip_prefix("Content-Length: ")?.trim().parse::<usize>().ok());
            let Some(content_length) = content_length else {
                tracing::warn!("Skipping LSP message without a Content-Length header");
                src.advance(body_start);
                continue;
            };
            if src.len() < body_start + content_length {
                src.reserve(body_start + content_length - src.len());
                return Ok(None);
            }
            let frame = src.split_to(body_start + content_length);
            match serde_json::from_slice(&frame[body_start..]) {
                Ok(message) => return Ok(Some(message)),
                Err(err) => tracing::warn!(%err, "Skipping invalid LSP message"),
            }
        }
    }
}

impl Encoder<Value> for LspCodec {
    type Error = io::Error;

    fn encode(&mut self, message: Value, dst: &mut BytesMut) -> io::Result<()> {
        let body = serde_json::to_vec(&message)?;
        dst.extend_from_slice(format!("Content-Length: {}\r\n\r\n", body.len()).as_bytes());
        dst.extend_from_slice(&body);
        Ok(())
    }
}
