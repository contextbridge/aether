use crate::error::{DaemonError, DaemonResult};
use crate::file_watcher::{FileWatcherBatch, FileWatcherHandle};
use crate::protocol::{LspErrorResponse, LspNotification};
use lsp_types::notification::{DidChangeWatchedFiles, Initialized, Notification, PublishDiagnostics};
use lsp_types::request::{Initialize, RegisterCapability, Request, UnregisterCapability, WorkDoneProgressCreate};
use lsp_types::{
    CallHierarchyClientCapabilities, ClientCapabilities, DidChangeWatchedFilesClientCapabilities,
    GeneralClientCapabilities, GotoCapability, HoverClientCapabilities, InitializeParams, MarkupKind,
    PublishDiagnosticsClientCapabilities, RegistrationParams, TextDocumentClientCapabilities,
    WorkspaceClientCapabilities,
};
use lsp_types::{DocumentSymbolClientCapabilities, DynamicRegistrationClientCapabilities};
use serde_json::Value;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};
use tokio::sync::{mpsc, oneshot};

const INITIALIZE_REQUEST_ID: i64 = 1;

#[derive(Clone)]
pub(crate) struct ProcessTransport {
    command_tx: mpsc::Sender<TransportCommand>,
}

pub(crate) enum TransportEvent {
    PublishedDiagnostics(lsp_types::PublishDiagnosticsParams),
    FileWatcherBatch(FileWatcherBatch),
    Closed,
}

struct TransportRequest {
    method: String,
    params: Value,
    response_tx: oneshot::Sender<Result<Value, LspErrorResponse>>,
}

enum TransportCommand {
    Request(TransportRequest),
    Notification(LspNotification),
    Shutdown,
}

struct ProcessTransportActor {
    process: Child,
    stdin: ChildStdin,
    reader: BufReader<ChildStdout>,
    command_rx: mpsc::Receiver<TransportCommand>,
    event_tx: mpsc::Sender<TransportEvent>,
    watcher: FileWatcherHandle,
    watcher_rx: mpsc::Receiver<FileWatcherBatch>,
    next_id: i64,
    pending: HashMap<i64, oneshot::Sender<Result<Value, LspErrorResponse>>>,
}

impl ProcessTransport {
    pub(crate) fn spawn(
        root_path: &Path,
        command: &str,
        args: &[String],
    ) -> DaemonResult<(Self, mpsc::Receiver<TransportEvent>)> {
        let resolved_command = resolve_command(root_path, command);
        let mut process = Command::new(&resolved_command)
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| DaemonError::LspSpawnFailed(format!("{}: {e}", resolved_command.display())))?;

        let stdin =
            process.stdin.take().ok_or_else(|| DaemonError::LspSpawnFailed("Failed to capture stdin".into()))?;
        let stdout =
            process.stdout.take().ok_or_else(|| DaemonError::LspSpawnFailed("Failed to capture stdout".into()))?;

        let (command_tx, command_rx) = mpsc::channel(100);
        let (event_tx, event_rx) = mpsc::channel(100);
        let (watcher_tx, watcher_rx) = mpsc::channel(64);
        let watcher = FileWatcherHandle::spawn(root_path.to_path_buf(), watcher_tx);

        let actor = ProcessTransportActor {
            process,
            stdin,
            reader: BufReader::new(stdout),
            command_rx,
            event_tx,
            watcher,
            watcher_rx,
            next_id: INITIALIZE_REQUEST_ID + 1,
            pending: HashMap::new(),
        };
        tokio::spawn(actor.run(root_path.to_path_buf()));

        Ok((Self { command_tx }, event_rx))
    }

    pub(crate) async fn request_raw(&self, method: &str, params: Value) -> Result<Value, LspErrorResponse> {
        let (response_tx, response_rx) = oneshot::channel();
        let request = TransportRequest { method: method.to_string(), params, response_tx };
        self.command_tx
            .send(TransportCommand::Request(request))
            .await
            .map_err(|_| LspErrorResponse { code: -1, message: "LSP transport closed".into() })?;

        response_rx.await.map_err(|_| LspErrorResponse { code: -1, message: "Response channel closed".into() })?
    }

    pub(crate) async fn send_notification(&self, notification: LspNotification) {
        let _ = self.command_tx.send(TransportCommand::Notification(notification)).await;
    }

    pub(crate) async fn shutdown(&self) {
        let _ = self.command_tx.send(TransportCommand::Shutdown).await;
    }
}

impl ProcessTransportActor {
    async fn run(mut self, root_path: PathBuf) {
        if let Err(err) = self.initialize(&root_path).await {
            tracing::error!(%err, "Failed to initialize LSP transport");
            self.cleanup_pending();
            let _ = self.event_tx.send(TransportEvent::Closed).await;
            return;
        }

        loop {
            tokio::select! {
                msg = read_lsp_message(&mut self.reader) => {
                    match msg {
                        Ok(Some(message)) => self.handle_lsp_message(message).await,
                        Ok(None) => break,
                        Err(err) => {
                            tracing::warn!(%err, "Error reading LSP message");
                        }
                    }
                }
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
                    let _ = tx.send(Err(LspErrorResponse { code: -1, message: err.to_string() }));
                }
                true
            }
            TransportCommand::Notification(notification) => {
                if let Err(err) = send_notification(&mut self.stdin, &notification.method, notification.params).await {
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

    async fn initialize(&mut self, root_path: &Path) -> std::io::Result<()> {
        let root_uri =
            crate::path_to_uri(root_path).map_err(|err| std::io::Error::new(std::io::ErrorKind::InvalidInput, err))?;

        let capabilities = ClientCapabilities {
            general: Some(GeneralClientCapabilities::default()),
            text_document: Some(TextDocumentClientCapabilities {
                publish_diagnostics: Some(PublishDiagnosticsClientCapabilities {
                    related_information: Some(true),
                    ..Default::default()
                }),
                definition: Some(GotoCapability { dynamic_registration: Some(false), link_support: Some(true) }),
                implementation: Some(GotoCapability { dynamic_registration: Some(false), link_support: Some(true) }),
                references: Some(DynamicRegistrationClientCapabilities { dynamic_registration: Some(false) }),
                hover: Some(HoverClientCapabilities {
                    dynamic_registration: Some(false),
                    content_format: Some(vec![MarkupKind::Markdown, MarkupKind::PlainText]),
                }),
                document_symbol: Some(DocumentSymbolClientCapabilities {
                    hierarchical_document_symbol_support: Some(true),
                    ..Default::default()
                }),
                call_hierarchy: Some(CallHierarchyClientCapabilities { dynamic_registration: Some(false) }),
                ..Default::default()
            }),
            workspace: Some(WorkspaceClientCapabilities {
                did_change_watched_files: Some(DidChangeWatchedFilesClientCapabilities {
                    dynamic_registration: Some(true),
                    relative_pattern_support: Some(false),
                }),
                ..Default::default()
            }),
            ..Default::default()
        };

        let params = InitializeParams {
            process_id: Some(std::process::id()),
            #[allow(deprecated)]
            root_uri: Some(root_uri),
            capabilities,
            ..Default::default()
        };

        self.send_request(INITIALIZE_REQUEST_ID, Initialize::METHOD, serde_json::to_value(&params).unwrap()).await?;

        while let Some(message) = read_lsp_message(&mut self.reader).await? {
            if message.get("id").and_then(Value::as_i64) == Some(INITIALIZE_REQUEST_ID) {
                send_notification(&mut self.stdin, Initialized::METHOD, serde_json::json!({})).await?;
                return Ok(());
            }

            self.handle_lsp_message(message).await;
        }

        Err(std::io::Error::new(std::io::ErrorKind::UnexpectedEof, "LSP closed during initialization"))
    }

    async fn handle_lsp_message(&mut self, message: Value) {
        let has_id = message.get("id").is_some();
        let method = message.get("method").and_then(Value::as_str);

        match (has_id, method) {
            (true, Some(method)) => {
                let id = message.get("id").cloned().unwrap_or(Value::Null);
                let params = message.get("params").cloned().unwrap_or(Value::Null);
                match method {
                    RegisterCapability::METHOD => {
                        self.handle_register_capability(&id, &params).await;
                    }
                    UnregisterCapability::METHOD => {
                        self.handle_unregister_capability(&id, &params).await;
                    }
                    WorkDoneProgressCreate::METHOD => {
                        let _ = self.send_ok_response(&id).await;
                    }
                    _ => {
                        let response = serde_json::json!({
                            "jsonrpc": "2.0",
                            "id": id,
                            "error": {
                                "code": -32601,
                                "message": format!("Method not found: {method}")
                            }
                        });
                        let _ = write_lsp_message(&mut self.stdin, &response).await;
                    }
                }
            }
            (true, None) => {
                if let Some(id) = message.get("id").and_then(Value::as_i64)
                    && let Some(tx) = self.pending.remove(&id)
                {
                    let result = if let Some(error) = message.get("error") {
                        let code = error
                            .get("code")
                            .and_then(Value::as_i64)
                            .and_then(|code| i32::try_from(code).ok())
                            .unwrap_or(-1);
                        let message =
                            error.get("message").and_then(Value::as_str).unwrap_or("Unknown error").to_string();
                        Err(LspErrorResponse { code, message })
                    } else {
                        Ok(message.get("result").cloned().unwrap_or(Value::Null))
                    };
                    let _ = tx.send(result);
                }
            }
            (false, Some(method)) if method == PublishDiagnostics::METHOD => {
                let params = message.get("params").cloned().unwrap_or(Value::Null);
                if let Ok(diagnostics) = serde_json::from_value(params) {
                    let _ = self.event_tx.send(TransportEvent::PublishedDiagnostics(diagnostics)).await;
                }
            }
            _ => {}
        }
    }

    async fn handle_register_capability(&mut self, id: &Value, params: &Value) {
        if let Ok(registration_params) = serde_json::from_value::<RegistrationParams>(params.clone()) {
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

    async fn handle_unregister_capability(&mut self, id: &Value, params: &Value) {
        if let Ok(unregistration_params) = serde_json::from_value::<lsp_types::UnregistrationParams>(params.clone()) {
            for registration in &unregistration_params.unregisterations {
                if registration.method == DidChangeWatchedFiles::METHOD {
                    self.watcher.unregister(registration.id.clone());
                }
            }
        }

        let _ = self.send_ok_response(id).await;
    }

    async fn send_request(&mut self, id: i64, method: &str, params: Value) -> std::io::Result<()> {
        let msg = serde_json::json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params
        });
        write_lsp_message(&mut self.stdin, &msg).await
    }

    async fn send_ok_response(&mut self, id: &Value) -> std::io::Result<()> {
        let response = serde_json::json!({
            "jsonrpc": "2.0",
            "id": id,
            "result": null
        });
        write_lsp_message(&mut self.stdin, &response).await
    }

    fn cleanup_pending(&mut self) {
        for (_, tx) in self.pending.drain() {
            let _ = tx.send(Err(LspErrorResponse { code: -1, message: "LSP transport closed".into() }));
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

async fn send_notification(stdin: &mut ChildStdin, method: &str, params: Value) -> std::io::Result<()> {
    let msg = serde_json::json!({
        "jsonrpc": "2.0",
        "method": method,
        "params": params
    });
    write_lsp_message(stdin, &msg).await
}

async fn read_lsp_message(reader: &mut BufReader<ChildStdout>) -> std::io::Result<Option<Value>> {
    let mut content_length: Option<usize> = None;

    loop {
        let mut header = String::new();
        let bytes = reader.read_line(&mut header).await?;
        if bytes == 0 {
            return Ok(None);
        }

        let header = header.trim();
        if header.is_empty() {
            break;
        }

        if let Some(value) = header.strip_prefix("Content-Length: ") {
            content_length = value.parse().ok();
        }
    }

    let content_length =
        content_length.ok_or_else(|| std::io::Error::new(std::io::ErrorKind::InvalidData, "Missing Content-Length"))?;

    let mut buf = vec![0u8; content_length];
    reader.read_exact(&mut buf).await?;

    serde_json::from_slice(&buf).map(Some).map_err(|err| std::io::Error::new(std::io::ErrorKind::InvalidData, err))
}

async fn write_lsp_message(stdin: &mut ChildStdin, msg: &Value) -> std::io::Result<()> {
    let content = serde_json::to_string(msg)?;
    let header = format!("Content-Length: {}\r\n\r\n", content.len());
    stdin.write_all(header.as_bytes()).await?;
    stdin.write_all(content.as_bytes()).await?;
    stdin.flush().await
}
