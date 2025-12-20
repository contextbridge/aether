use lsp_types::notification::{Initialized, Notification};
use lsp_types::request::{GotoDefinition, Initialize, Request};
use lsp_types::*;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::{ChildStdin, ChildStdout, Command};
use tokio::sync::{Mutex, mpsc, oneshot};

/// Convert a file path to an LSP Uri
fn path_to_uri(path: &std::path::Path) -> Result<Uri, String> {
    let url = url::Url::from_file_path(path)
        .map_err(|_| format!("Invalid path: {:?}", path))?;
    url.as_str()
        .parse()
        .map_err(|e| format!("Failed to parse URI: {}", e))
}

/// LSP client that spawns process in background task and provides typed API
pub struct LspClient {
    request_tx: mpsc::Sender<LspRequest>,
    notification_rx: Arc<Mutex<mpsc::Receiver<Value>>>,
    shutdown_tx: Arc<Mutex<Option<oneshot::Sender<()>>>>,
}

enum LspRequest {
    TypedRequest {
        method: String,
        params: Value,
        response_tx: oneshot::Sender<Result<Value, String>>,
    },
    TypedNotification {
        method: String,
        params: Value,
    },
}

impl LspClient {
    pub async fn new(_workspace_root: PathBuf) -> Result<Self, String> {
        let (request_tx, notification_rx, shutdown_tx) = Self::process_task().await?;

        Ok(LspClient {
            request_tx,
            notification_rx: Arc::new(Mutex::new(notification_rx)),
            shutdown_tx: Arc::new(Mutex::new(Some(shutdown_tx))),
        })
    }

    async fn process_task() -> Result<
        (
            mpsc::Sender<LspRequest>,
            mpsc::Receiver<Value>,
            oneshot::Sender<()>,
        ),
        String,
    > {
        let (request_tx, request_rx) = mpsc::channel(100);
        let (notification_tx, notification_rx) = mpsc::channel(100);
        let (shutdown_tx, shutdown_rx) = oneshot::channel();

        // Spawn background task to manage the rust-analyzer process
        tokio::spawn(async move {
            if let Err(e) = Self::run_process_loop(request_rx, notification_tx, shutdown_rx).await {
                eprintln!("LSP process task failed: {}", e);
            }
        });

        Ok((request_tx, notification_rx, shutdown_tx))
    }

    async fn run_process_loop(
        mut request_rx: mpsc::Receiver<LspRequest>,
        notification_tx: mpsc::Sender<Value>,
        mut shutdown_rx: oneshot::Receiver<()>,
    ) -> Result<(), String> {
        // Spawn rust-analyzer process within the task
        let mut child = Command::new("rust-analyzer")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .map_err(|e| format!("Failed to spawn rust-analyzer: {}", e))?;

        let stdin = child.stdin.take().ok_or("Failed to get stdin handle")?;

        let stdout = child.stdout.take().ok_or("Failed to get stdout handle")?;

        // Set up low-level protocol handling
        let pending_requests = Arc::new(Mutex::new(HashMap::<u64, oneshot::Sender<Value>>::new()));
        let request_id = AtomicU64::new(1);
        let stdin = Arc::new(Mutex::new(stdin));

        let mut reader = BufReader::new(stdout);

        loop {
            tokio::select! {
                request = request_rx.recv() => {
                    match request {
                        Some(LspRequest::TypedRequest { method, params, response_tx }) => {
                            let result = Self::send_request_raw(&stdin, &request_id, &pending_requests, &method, params).await;
                            let _ = response_tx.send(result);
                        }
                        Some(LspRequest::TypedNotification { method, params }) => {
                            let _ = Self::send_notification_raw(&stdin, &method, params).await;
                        }
                        None => break, // Channel closed
                    }
                }
                message_result = Self::read_lsp_message(&mut reader) => {
                    match message_result {
                        Ok(Some(message)) => {
                            if let Some(id) = message.get("id") {
                                if let Some(id) = id.as_u64() {
                                    let mut pending = pending_requests.lock().await;
                                    if let Some(sender) = pending.remove(&id) {
                                        let _ = sender.send(message);
                                    }
                                }
                            } else {
                                if let Err(e) = notification_tx.try_send(message) {
                                    match e {
                                        mpsc::error::TrySendError::Full(_) => {
                                            eprintln!("Warning: LSP notification channel full, dropping message");
                                        }
                                        mpsc::error::TrySendError::Closed(_) => {
                                            eprintln!("LSP notification channel closed");
                                            break;
                                        }
                                    }
                                }
                            }
                        }
                        Ok(None) => break, // EOF
                        Err(e) => {
                            eprintln!("Error reading LSP message: {}", e);
                            break;
                        }
                    }
                }
                _ = &mut shutdown_rx => {
                    // Shutdown requested
                    let _ = Self::send_request_raw(&stdin, &request_id, &pending_requests, "shutdown", json!({})).await;
                    let _ = Self::send_notification_raw(&stdin, "exit", json!({})).await;
                    let _ = child.wait().await;
                    break;
                }
            }
        }

        Ok(())
    }

    async fn read_lsp_message(
        reader: &mut BufReader<ChildStdout>,
    ) -> Result<Option<Value>, String> {
        // Read headers
        let mut content_length = 0;
        let mut line = String::new();

        loop {
            line.clear();
            match reader.read_line(&mut line).await {
                Ok(0) => return Ok(None), // EOF
                Ok(_) => {
                    let line = line.trim_end();
                    if line.is_empty() {
                        break; // End of headers
                    }
                    if line.starts_with("Content-Length: ") {
                        content_length = line[16..]
                            .trim()
                            .parse::<usize>()
                            .map_err(|e| format!("Invalid Content-Length: {}", e))?;
                    }
                }
                Err(e) => return Err(format!("Error reading header: {}", e)),
            }
        }

        if content_length == 0 {
            return Err("Missing Content-Length header".to_string());
        }

        // Read exact number of bytes for content
        let mut content_bytes = vec![0u8; content_length];
        reader
            .read_exact(&mut content_bytes)
            .await
            .map_err(|e| format!("Error reading content bytes: {}", e))?;

        let content = String::from_utf8(content_bytes)
            .map_err(|e| format!("Invalid UTF-8 in content: {}", e))?;

        // Parse JSON
        serde_json::from_str(&content)
            .map(Some)
            .map_err(|e| format!("Failed to parse JSON: {}", e))
    }

    async fn send_request_raw(
        stdin: &Arc<Mutex<ChildStdin>>,
        request_id: &AtomicU64,
        pending_requests: &Arc<Mutex<HashMap<u64, oneshot::Sender<Value>>>>,
        method: &str,
        params: Value,
    ) -> Result<Value, String> {
        let id = request_id.fetch_add(1, Ordering::SeqCst);
        let request = json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params
        });

        let (tx, rx) = oneshot::channel();
        {
            let mut pending = pending_requests.lock().await;
            pending.insert(id, tx);
        }

        Self::send_message(stdin, request).await?;

        let response = rx.await.map_err(|_| "Request was cancelled".to_string())?;

        // Check for error in response
        if let Some(error) = response.get("error") {
            return Err(format!("LSP request failed: {}", error));
        }

        // Extract result
        response
            .get("result")
            .cloned()
            .ok_or("No result in LSP response".to_string())
    }

    async fn send_notification_raw(
        stdin: &Arc<Mutex<ChildStdin>>,
        method: &str,
        params: Value,
    ) -> Result<(), String> {
        let notification = json!({
            "jsonrpc": "2.0",
            "method": method,
            "params": params
        });

        Self::send_message(stdin, notification).await
    }

    async fn send_message(stdin: &Arc<Mutex<ChildStdin>>, message: Value) -> Result<(), String> {
        let content = serde_json::to_string(&message)
            .map_err(|e| format!("Failed to serialize message: {}", e))?;

        let header = format!("Content-Length: {}\r\n\r\n", content.len());
        let full_message = format!("{}{}", header, content);

        let mut stdin_guard = stdin.lock().await;
        stdin_guard
            .write_all(full_message.as_bytes())
            .await
            .map_err(|e| format!("Failed to write to stdin: {}", e))?;

        stdin_guard
            .flush()
            .await
            .map_err(|e| format!("Failed to flush stdin: {}", e))
    }

    // Public API methods for the LspClient
    pub async fn send_request<R>(&self, params: R::Params) -> Result<R::Result, String>
    where
        R: Request,
        R::Params: serde::Serialize,
        R::Result: serde::de::DeserializeOwned,
    {
        let params_value = serde_json::to_value(params)
            .map_err(|e| format!("Failed to serialize params: {}", e))?;

        let (response_tx, response_rx) = oneshot::channel();

        let request = LspRequest::TypedRequest {
            method: R::METHOD.to_string(),
            params: params_value,
            response_tx,
        };

        self.request_tx
            .try_send(request)
            .map_err(|e| match e {
                mpsc::error::TrySendError::Full(_) => "LSP client channel full".to_string(),
                mpsc::error::TrySendError::Closed(_) => "LSP client channel closed".to_string(),
            })?;

        let response = response_rx
            .await
            .map_err(|_| "Response channel closed".to_string())??;

        serde_json::from_value(response)
            .map_err(|e| format!("Failed to deserialize response: {}", e))
    }

    pub async fn send_notification<N>(&self, params: N::Params) -> Result<(), String>
    where
        N: Notification,
        N::Params: serde::Serialize,
    {
        let params_value = serde_json::to_value(params)
            .map_err(|e| format!("Failed to serialize params: {}", e))?;

        let request = LspRequest::TypedNotification {
            method: N::METHOD.to_string(),
            params: params_value,
        };

        self.request_tx
            .try_send(request)
            .map_err(|e| match e {
                mpsc::error::TrySendError::Full(_) => "LSP client channel full".to_string(),
                mpsc::error::TrySendError::Closed(_) => "LSP client channel closed".to_string(),
            })
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

        // Initialize the LSP connection
        session.initialize(workspace_root).await?;

        Ok(session)
    }

    async fn initialize(&self, workspace_root: PathBuf) -> Result<(), String> {
        let workspace_uri = path_to_uri(&workspace_root)?;

        #[allow(deprecated)]
        let init_params = InitializeParams {
            process_id: Some(std::process::id()),
            root_path: None,  // deprecated, but required by struct
            root_uri: None,   // deprecated, using workspace_folders instead
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

        // Send initialized notification
        self.client
            .send_notification::<Initialized>(InitializedParams {})
            .await?;

        Ok(())
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
