use lsp_types::notification::{DidCloseTextDocument, DidOpenTextDocument, Initialized, Notification};
use lsp_types::request::{GotoDefinition, Initialize, Request};
use lsp_types::*;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::{ChildStdin, ChildStdout, Command};
use tokio::sync::{Mutex, mpsc, oneshot};

/// Request ID reserved for the shutdown request during cleanup
const SHUTDOWN_REQUEST_ID: u64 = u64::MAX;

/// Convert a file path to an LSP Uri
fn path_to_uri(path: &std::path::Path) -> Result<Uri, String> {
    let url = url::Url::from_file_path(path)
        .map_err(|_| format!("Invalid path: {:?}", path))?;
    url.as_str()
        .parse()
        .map_err(|e| format!("Failed to parse URI: {e}"))
}

type PendingRequests = Arc<Mutex<HashMap<u64, oneshot::Sender<Result<Value, String>>>>>;

/// LSP client that spawns process in background task and provides typed API
pub struct LspClient {
    request_tx: mpsc::Sender<LspMessage>,
    pending_requests: PendingRequests,
    notification_rx: Arc<Mutex<mpsc::Receiver<Value>>>,
    shutdown_tx: Arc<Mutex<Option<oneshot::Sender<()>>>>,
    next_id: AtomicU64,
}

enum LspMessage {
    Request {
        id: u64,
        method: String,
        params: Value,
    },
    Notification {
        method: String,
        params: Value,
    },
}

impl LspClient {
    pub async fn new(_workspace_root: PathBuf) -> Result<Self, String> {
        // Spawn rust-analyzer process
        let mut child = Command::new("rust-analyzer")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .map_err(|e| format!("Failed to spawn rust-analyzer: {e}"))?;

        let stdin = child.stdin.take().ok_or("Failed to get stdin handle")?;
        let stdout = child.stdout.take().ok_or("Failed to get stdout handle")?;

        let (request_tx, request_rx) = mpsc::channel::<LspMessage>(100);
        let (notification_tx, notification_rx) = mpsc::channel::<Value>(100);
        let (shutdown_tx, shutdown_rx) = oneshot::channel();

        let pending_requests: PendingRequests = Arc::new(Mutex::new(HashMap::new()));
        let pending_for_loop = pending_requests.clone();

        // Spawn background task to handle stdin/stdout
        tokio::spawn(async move {
            if let Err(e) = Self::run_process_loop(
                child,
                stdin,
                stdout,
                request_rx,
                notification_tx,
                pending_for_loop,
                shutdown_rx,
            ).await {
                eprintln!("LSP process task failed: {e}");
            }
        });

        Ok(LspClient {
            request_tx,
            pending_requests,
            notification_rx: Arc::new(Mutex::new(notification_rx)),
            shutdown_tx: Arc::new(Mutex::new(Some(shutdown_tx))),
            next_id: AtomicU64::new(1),
        })
    }

    async fn run_process_loop(
        mut child: tokio::process::Child,
        stdin: ChildStdin,
        stdout: ChildStdout,
        mut request_rx: mpsc::Receiver<LspMessage>,
        notification_tx: mpsc::Sender<Value>,
        pending_requests: PendingRequests,
        mut shutdown_rx: oneshot::Receiver<()>,
    ) -> Result<(), String> {
        let stdin = Arc::new(Mutex::new(stdin));
        let mut reader = BufReader::new(stdout);

        loop {
            tokio::select! {
                // Handle outgoing requests/notifications
                request = request_rx.recv() => {
                    match request {
                        Some(LspMessage::Request { id, method, params }) => {
                            let message = json!({
                                "jsonrpc": "2.0",
                                "id": id,
                                "method": method,
                                "params": params
                            });
                            if let Err(e) = Self::send_message(&stdin, message).await {
                                let mut pending = pending_requests.lock().await;
                                if let Some(tx) = pending.remove(&id) {
                                    let _ = tx.send(Err(e));
                                }
                            }
                        }
                        Some(LspMessage::Notification { method, params }) => {
                            let message = json!({
                                "jsonrpc": "2.0",
                                "method": method,
                                "params": params
                            });
                            let _ = Self::send_message(&stdin, message).await;
                        }
                        None => break,
                    }
                }

                // Handle incoming messages from LSP server
                message_result = Self::read_lsp_message(&mut reader) => {
                    match message_result {
                        Ok(Some(message)) => {
                            // Response: has id but no method
                            if message.get("id").is_some() && message.get("method").is_none() {
                                if let Some(id) = message.get("id").and_then(|i| i.as_u64()) {
                                    let mut pending = pending_requests.lock().await;
                                    if let Some(tx) = pending.remove(&id) {
                                        let result = if let Some(error) = message.get("error") {
                                            Err(format!("LSP error: {error}"))
                                        } else if let Some(result) = message.get("result") {
                                            Ok(result.clone())
                                        } else {
                                            Ok(Value::Null)
                                        };
                                        let _ = tx.send(result);
                                    }
                                }
                            } else {
                                // Server notification
                                if let Err(e) = notification_tx.try_send(message) {
                                    if matches!(e, mpsc::error::TrySendError::Closed(_)) {
                                        break;
                                    }
                                }
                            }
                        }
                        Ok(None) => break,
                        Err(e) => {
                            eprintln!("Error reading LSP message: {e}");
                            break;
                        }
                    }
                }

                // Handle shutdown
                _ = &mut shutdown_rx => {
                    let shutdown_msg = json!({
                        "jsonrpc": "2.0",
                        "id": SHUTDOWN_REQUEST_ID,
                        "method": "shutdown",
                        "params": null
                    });
                    let _ = Self::send_message(&stdin, shutdown_msg).await;

                    let exit_msg = json!({
                        "jsonrpc": "2.0",
                        "method": "exit",
                        "params": null
                    });
                    let _ = Self::send_message(&stdin, exit_msg).await;

                    let _ = child.wait().await;
                    break;
                }
            }
        }

        Ok(())
    }

    async fn read_lsp_message(reader: &mut BufReader<ChildStdout>) -> Result<Option<Value>, String> {
        let mut content_length = 0;
        let mut line = String::new();

        loop {
            line.clear();
            match reader.read_line(&mut line).await {
                Ok(0) => return Ok(None),
                Ok(_) => {
                    let trimmed = line.trim_end();
                    if trimmed.is_empty() {
                        break;
                    }
                    if let Some(len_str) = trimmed.strip_prefix("Content-Length: ") {
                        content_length = len_str
                            .trim()
                            .parse::<usize>()
                            .map_err(|e| format!("Invalid Content-Length: {e}"))?;
                    }
                }
                Err(e) => return Err(format!("Error reading header: {e}")),
            }
        }

        if content_length == 0 {
            return Err("Missing Content-Length header".to_string());
        }

        let mut content_bytes = vec![0u8; content_length];
        reader
            .read_exact(&mut content_bytes)
            .await
            .map_err(|e| format!("Error reading content: {e}"))?;

        let content = String::from_utf8(content_bytes)
            .map_err(|e| format!("Invalid UTF-8: {e}"))?;

        serde_json::from_str(&content)
            .map(Some)
            .map_err(|e| format!("Failed to parse JSON: {e}"))
    }

    async fn send_message(stdin: &Arc<Mutex<ChildStdin>>, message: Value) -> Result<(), String> {
        let content = serde_json::to_string(&message)
            .map_err(|e| format!("Failed to serialize: {e}"))?;

        let full_message = format!("Content-Length: {}\r\n\r\n{}", content.len(), content);

        let mut stdin_guard = stdin.lock().await;
        stdin_guard
            .write_all(full_message.as_bytes())
            .await
            .map_err(|e| format!("Failed to write: {e}"))?;
        stdin_guard
            .flush()
            .await
            .map_err(|e| format!("Failed to flush: {e}"))
    }

    /// Send a typed LSP request and wait for response
    pub async fn send_request<R>(&self, params: R::Params) -> Result<R::Result, String>
    where
        R: Request,
        R::Params: serde::Serialize,
        R::Result: serde::de::DeserializeOwned,
    {
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);

        let params_value = serde_json::to_value(params)
            .map_err(|e| format!("Failed to serialize params: {e}"))?;

        // Create response channel and register BEFORE sending
        let (tx, rx) = oneshot::channel();
        {
            let mut pending = self.pending_requests.lock().await;
            pending.insert(id, tx);
        }

        // Send the request
        self.request_tx
            .send(LspMessage::Request {
                id,
                method: R::METHOD.to_string(),
                params: params_value,
            })
            .await
            .map_err(|_| "Channel closed".to_string())?;

        // Wait for response
        let response = rx.await.map_err(|_| "Response channel closed".to_string())??;

        serde_json::from_value(response)
            .map_err(|e| format!("Failed to deserialize response: {e}"))
    }

    pub async fn send_notification<N>(&self, params: N::Params) -> Result<(), String>
    where
        N: Notification,
        N::Params: serde::Serialize,
    {
        let params_value = serde_json::to_value(params)
            .map_err(|e| format!("Failed to serialize params: {e}"))?;

        self.request_tx
            .send(LspMessage::Notification {
                method: N::METHOD.to_string(),
                params: params_value,
            })
            .await
            .map_err(|_| "Channel closed".to_string())
    }

    pub async fn get_next_notification(&self) -> Option<Value> {
        let mut rx = self.notification_rx.lock().await;
        rx.recv().await
    }

    pub async fn shutdown(&self) -> Result<(), String> {
        let mut shutdown_tx_guard = self.shutdown_tx.lock().await;
        if let Some(shutdown_tx) = shutdown_tx_guard.take() {
            let _ = shutdown_tx.send(());
        }
        Ok(())
    }
}

/// High-level LSP session that handles initialization and common operations
pub struct LspSession {
    client: LspClient,
}

impl LspSession {
    pub async fn new(workspace_root: PathBuf) -> Result<Self, String> {
        let client = LspClient::new(workspace_root.clone()).await?;
        let session = LspSession { client };
        session.initialize(workspace_root).await?;
        Ok(session)
    }

    async fn initialize(&self, workspace_root: PathBuf) -> Result<(), String> {
        let workspace_uri = path_to_uri(&workspace_root)?;

        #[allow(deprecated)]
        let init_params = InitializeParams {
            process_id: Some(std::process::id()),
            root_path: None,
            root_uri: None,
            initialization_options: None,
            capabilities: ClientCapabilities {
                text_document: Some(TextDocumentClientCapabilities {
                    publish_diagnostics: Some(PublishDiagnosticsClientCapabilities {
                        related_information: Some(true),
                        tag_support: Some(TagSupport {
                            value_set: vec![DiagnosticTag::UNNECESSARY, DiagnosticTag::DEPRECATED],
                        }),
                        version_support: Some(true),
                        code_description_support: Some(true),
                        data_support: Some(true),
                    }),
                    ..Default::default()
                }),
                workspace: Some(WorkspaceClientCapabilities {
                    workspace_folders: Some(true),
                    ..Default::default()
                }),
                ..Default::default()
            },
            trace: Some(TraceValue::Off),
            workspace_folders: Some(vec![WorkspaceFolder {
                uri: workspace_uri,
                name: workspace_root
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("workspace")
                    .to_string(),
            }]),
            client_info: Some(ClientInfo {
                name: "mcp-lexicon".to_string(),
                version: Some("0.1.0".to_string()),
            }),
            locale: None,
            work_done_progress_params: WorkDoneProgressParams {
                work_done_token: None,
            },
        };

        let _init_result: InitializeResult =
            self.client.send_request::<Initialize>(init_params).await?;

        self.client
            .send_notification::<Initialized>(InitializedParams {})
            .await?;

        Ok(())
    }

    /// Open a file for analysis. This triggers diagnostics for the file.
    pub async fn open_file(&self, file_path: PathBuf) -> Result<(), String> {
        let uri = path_to_uri(&file_path)?;
        let content = fs::read_to_string(&file_path)
            .map_err(|e| format!("Failed to read file {:?}: {e}", file_path))?;

        let params = DidOpenTextDocumentParams {
            text_document: TextDocumentItem {
                uri,
                language_id: "rust".to_string(),
                version: 1,
                text: content,
            },
        };

        self.client
            .send_notification::<DidOpenTextDocument>(params)
            .await
    }

    /// Close a file (stop tracking it)
    pub async fn close_file(&self, file_path: PathBuf) -> Result<(), String> {
        let uri = path_to_uri(&file_path)?;

        let params = DidCloseTextDocumentParams {
            text_document: TextDocumentIdentifier { uri },
        };

        self.client
            .send_notification::<DidCloseTextDocument>(params)
            .await
    }

    pub async fn goto_definition(
        &self,
        file_path: PathBuf,
        line: u32,
        character: u32,
    ) -> Result<Option<GotoDefinitionResponse>, String> {
        let uri = path_to_uri(&file_path)?;

        let params = GotoDefinitionParams {
            text_document_position_params: TextDocumentPositionParams {
                text_document: TextDocumentIdentifier { uri },
                position: Position { line, character },
            },
            work_done_progress_params: WorkDoneProgressParams {
                work_done_token: None,
            },
            partial_result_params: PartialResultParams {
                partial_result_token: None,
            },
        };

        self.client.send_request::<GotoDefinition>(params).await
    }

    pub async fn get_next_notification(&self) -> Option<Value> {
        self.client.get_next_notification().await
    }

    pub async fn shutdown(&self) -> Result<(), String> {
        self.client.shutdown().await
    }
}
