use std::collections::HashMap;
use std::io;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};
use std::time::Duration;

use lsp_types::{
    CallHierarchyIncomingCall, CallHierarchyIncomingCallsParams, CallHierarchyItem, CallHierarchyOutgoingCall,
    CallHierarchyOutgoingCallsParams, CallHierarchyPrepareParams, DocumentSymbolParams, DocumentSymbolResponse,
    GotoDefinitionParams, GotoDefinitionResponse, Hover, HoverParams, Location, PartialResultParams, Position,
    PublishDiagnosticsParams, ReferenceContext, ReferenceParams, RenameParams, SymbolInformation,
    TextDocumentIdentifier, TextDocumentPositionParams, Uri, WorkDoneProgressParams, WorkspaceEdit,
    WorkspaceSymbolParams,
};
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;
use thiserror::Error;
use tokio::io::{ReadHalf, WriteHalf};
use tokio::net::UnixStream;
use tokio::process::Command;
use tokio::sync::{Mutex, oneshot};

use crate::language_catalog::LanguageId;
use crate::protocol::{DaemonRequest, DaemonResponse, InitializeRequest, read_frame, write_frame};
use crate::socket_path::{ensure_socket_dir, log_file_path};

#[doc = include_str!("docs/client_error.md")]
#[derive(Debug, Error)]
pub enum ClientError {
    #[error("Failed to connect to daemon: {0}")]
    ConnectionFailed(#[source] io::Error),

    #[error("IO error: {0}")]
    Io(#[from] io::Error),

    #[error("Daemon error: {0}")]
    DaemonError(String),

    #[error("LSP error (code={code}): {message}")]
    LspError { code: i32, message: String },

    #[error("Failed to spawn daemon: {0}")]
    SpawnFailed(#[source] io::Error),

    #[error("Timeout waiting for daemon to start")]
    SpawnTimeout,

    #[error("Daemon binary not found: {0}")]
    DaemonBinaryNotFound(String),

    #[error("Protocol error: {0}")]
    ProtocolError(String),

    #[error("Initialization failed: {0}")]
    InitializationFailed(String),
}

pub type ClientResult<T> = std::result::Result<T, ClientError>;

#[doc = include_str!("docs/client.md")]
pub struct LspClient {
    writer: Mutex<WriteHalf<UnixStream>>,
    pending: Arc<Mutex<HashMap<i64, oneshot::Sender<PendingResult>>>>,
    next_id: AtomicI64,
    reader_task: tokio::task::JoinHandle<()>,
}

impl LspClient {
    pub async fn connect(workspace_root: &Path, language: LanguageId) -> ClientResult<Self> {
        let socket_path = ensure_socket_dir(workspace_root, language).map_err(ClientError::Io)?;

        match UnixStream::connect(&socket_path).await {
            Ok(stream) => {
                return Self::from_stream(stream, workspace_root, language).await;
            }
            Err(err) if err.kind() == ErrorKind::ConnectionRefused || err.kind() == ErrorKind::NotFound => {}
            Err(err) => return Err(ClientError::ConnectionFailed(err)),
        }

        spawn_daemon(&socket_path).await?;
        let stream = UnixStream::connect(&socket_path).await.map_err(ClientError::ConnectionFailed)?;
        Self::from_stream(stream, workspace_root, language).await
    }

    pub async fn goto_definition(&self, uri: Uri, line: u32, character: u32) -> ClientResult<GotoDefinitionResponse> {
        let params = GotoDefinitionParams {
            text_document_position_params: TextDocumentPositionParams {
                text_document: TextDocumentIdentifier { uri },
                position: Position { line, character },
            },
            work_done_progress_params: WorkDoneProgressParams::default(),
            partial_result_params: PartialResultParams::default(),
        };
        self.call("textDocument/definition", &params, || GotoDefinitionResponse::Array(vec![])).await
    }

    pub async fn goto_implementation(
        &self,
        uri: Uri,
        line: u32,
        character: u32,
    ) -> ClientResult<GotoDefinitionResponse> {
        let params = GotoDefinitionParams {
            text_document_position_params: TextDocumentPositionParams {
                text_document: TextDocumentIdentifier { uri },
                position: Position { line, character },
            },
            work_done_progress_params: WorkDoneProgressParams::default(),
            partial_result_params: PartialResultParams::default(),
        };
        self.call("textDocument/implementation", &params, || GotoDefinitionResponse::Array(vec![])).await
    }

    pub async fn find_references(
        &self,
        uri: Uri,
        line: u32,
        character: u32,
        include_declaration: bool,
    ) -> ClientResult<Vec<Location>> {
        let params = ReferenceParams {
            text_document_position: TextDocumentPositionParams {
                text_document: TextDocumentIdentifier { uri },
                position: Position { line, character },
            },
            work_done_progress_params: WorkDoneProgressParams::default(),
            partial_result_params: PartialResultParams::default(),
            context: ReferenceContext { include_declaration },
        };
        self.call("textDocument/references", &params, Vec::new).await
    }

    pub async fn hover(&self, uri: Uri, line: u32, character: u32) -> ClientResult<Option<Hover>> {
        let params = HoverParams {
            text_document_position_params: TextDocumentPositionParams {
                text_document: TextDocumentIdentifier { uri },
                position: Position { line, character },
            },
            work_done_progress_params: WorkDoneProgressParams::default(),
        };
        self.call("textDocument/hover", &params, || None).await
    }

    pub async fn workspace_symbol(&self, query: String) -> ClientResult<Vec<SymbolInformation>> {
        let params = WorkspaceSymbolParams {
            query,
            partial_result_params: PartialResultParams::default(),
            work_done_progress_params: WorkDoneProgressParams::default(),
        };
        self.call("workspace/symbol", &params, Vec::new).await
    }

    pub async fn document_symbol(&self, uri: Uri) -> ClientResult<DocumentSymbolResponse> {
        let params = DocumentSymbolParams {
            text_document: TextDocumentIdentifier { uri },
            work_done_progress_params: WorkDoneProgressParams::default(),
            partial_result_params: PartialResultParams::default(),
        };
        self.call("textDocument/documentSymbol", &params, || DocumentSymbolResponse::Flat(vec![])).await
    }

    pub async fn prepare_call_hierarchy(
        &self,
        uri: Uri,
        line: u32,
        character: u32,
    ) -> ClientResult<Vec<CallHierarchyItem>> {
        let params = CallHierarchyPrepareParams {
            text_document_position_params: TextDocumentPositionParams {
                text_document: TextDocumentIdentifier { uri },
                position: Position { line, character },
            },
            work_done_progress_params: WorkDoneProgressParams::default(),
        };
        self.call("textDocument/prepareCallHierarchy", &params, Vec::new).await
    }

    pub async fn incoming_calls(&self, item: CallHierarchyItem) -> ClientResult<Vec<CallHierarchyIncomingCall>> {
        let params = CallHierarchyIncomingCallsParams {
            item,
            work_done_progress_params: WorkDoneProgressParams::default(),
            partial_result_params: PartialResultParams::default(),
        };
        self.call("callHierarchy/incomingCalls", &params, Vec::new).await
    }

    pub async fn outgoing_calls(&self, item: CallHierarchyItem) -> ClientResult<Vec<CallHierarchyOutgoingCall>> {
        let params = CallHierarchyOutgoingCallsParams {
            item,
            work_done_progress_params: WorkDoneProgressParams::default(),
            partial_result_params: PartialResultParams::default(),
        };
        self.call("callHierarchy/outgoingCalls", &params, Vec::new).await
    }

    pub async fn rename(
        &self,
        uri: Uri,
        line: u32,
        character: u32,
        new_name: String,
    ) -> ClientResult<Option<WorkspaceEdit>> {
        let params = RenameParams {
            text_document_position: TextDocumentPositionParams {
                text_document: TextDocumentIdentifier { uri },
                position: Position { line, character },
            },
            new_name,
            work_done_progress_params: WorkDoneProgressParams::default(),
        };
        self.call("textDocument/rename", &params, || None).await
    }

    pub async fn get_diagnostics(&self, uri: Option<Uri>) -> ClientResult<Vec<PublishDiagnosticsParams>> {
        let client_id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let request = DaemonRequest::GetDiagnostics { client_id, uri };

        self.send_and_await(request, client_id)
            .await
            .and_then(|value| serde_json::from_value(value).map_err(|err| ClientError::ProtocolError(err.to_string())))
    }

    pub async fn queue_diagnostic_refresh(&self, uri: Uri) -> ClientResult<()> {
        let client_id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let request = DaemonRequest::QueueDiagnosticRefresh { client_id, uri };
        self.send_and_await(request, client_id).await.map(|_| ())
    }

    pub async fn disconnect(self) -> ClientResult<()> {
        let request = DaemonRequest::Disconnect;
        let mut writer = self.writer.lock().await;
        write_frame(&mut *writer, &request).await.map_err(ClientError::Io)
    }

    pub async fn call<P: Serialize, R: DeserializeOwned>(
        &self,
        method: &str,
        params: &P,
        default: impl FnOnce() -> R,
    ) -> ClientResult<R> {
        let params_value = serde_json::to_value(params).map_err(|err| ClientError::ProtocolError(err.to_string()))?;

        let client_id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let request = DaemonRequest::LspCall { client_id, method: method.to_string(), params: params_value };

        let value = self.send_and_await(request, client_id).await?;

        if value.is_null() {
            Ok(default())
        } else {
            serde_json::from_value(value).map_err(|err| ClientError::ProtocolError(format!("Parse error: {err}")))
        }
    }
}

impl LspClient {
    async fn from_stream(stream: UnixStream, workspace_root: &Path, language: LanguageId) -> ClientResult<Self> {
        let (mut reader, mut writer) = tokio::io::split(stream);

        let initialize =
            DaemonRequest::Initialize(InitializeRequest { workspace_root: workspace_root.to_path_buf(), language });

        write_frame(&mut writer, &initialize).await.map_err(ClientError::Io)?;

        let response: Option<DaemonResponse> = read_frame(&mut reader).await.map_err(ClientError::Io)?;

        match response {
            Some(DaemonResponse::Initialized) => {}
            Some(DaemonResponse::Error(err)) => {
                return Err(ClientError::InitializationFailed(err.message));
            }
            Some(_) => {
                return Err(ClientError::ProtocolError("Unexpected response to Initialize".into()));
            }
            None => {
                return Err(ClientError::ProtocolError("Connection closed during initialization".into()));
            }
        }

        let pending: Arc<Mutex<HashMap<i64, oneshot::Sender<PendingResult>>>> = Arc::new(Mutex::new(HashMap::new()));

        let pending_clone = Arc::clone(&pending);
        let reader_task = tokio::spawn(async move {
            run_reader(reader, pending_clone).await;
        });

        Ok(Self { writer: Mutex::new(writer), pending, next_id: AtomicI64::new(1), reader_task })
    }

    async fn send_and_await(&self, request: DaemonRequest, client_id: i64) -> ClientResult<Value> {
        let (response_tx, response_rx) = oneshot::channel();

        {
            let mut pending = self.pending.lock().await;
            pending.insert(client_id, response_tx);
        }

        let write_result = {
            let mut writer = self.writer.lock().await;
            write_frame(&mut *writer, &request).await
        };

        if let Err(err) = write_result {
            self.pending.lock().await.remove(&client_id);
            return Err(ClientError::Io(err));
        }

        response_rx.await.map_err(|_| ClientError::ProtocolError("Response channel closed".into()))?
    }
}

impl Drop for LspClient {
    fn drop(&mut self) {
        self.reader_task.abort();
    }
}

type PendingResult = Result<Value, ClientError>;

async fn run_reader(
    mut reader: ReadHalf<UnixStream>,
    pending: Arc<Mutex<HashMap<i64, oneshot::Sender<PendingResult>>>>,
) {
    loop {
        let response: Option<DaemonResponse> = match read_frame(&mut reader).await {
            Ok(Some(response)) => Some(response),
            Ok(None) => break,
            Err(err) => {
                tracing::debug!(%err, "Error reading daemon response");
                break;
            }
        };

        match response {
            Some(DaemonResponse::LspResult { client_id, result }) => {
                let mut pending = pending.lock().await;
                if let Some(tx) = pending.remove(&client_id) {
                    let value_result =
                        result.map_err(|err| ClientError::LspError { code: err.code, message: err.message });
                    let _ = tx.send(value_result);
                }
            }
            Some(DaemonResponse::Error(err)) => {
                if let Some(client_id) = err.client_id {
                    let mut pending = pending.lock().await;
                    if let Some(tx) = pending.remove(&client_id) {
                        let _ = tx.send(Err(ClientError::DaemonError(err.message)));
                    }
                }
            }
            _ => {}
        }
    }

    let mut pending = pending.lock().await;
    for (_, tx) in pending.drain() {
        let _ = tx.send(Err(ClientError::ProtocolError("Daemon disconnected".into())));
    }
}

async fn spawn_daemon(socket_path: &Path) -> ClientResult<()> {
    let (binary, subcommand) = find_daemon_binary()?;
    let log_file = log_file_path(socket_path);

    let mut cmd = Command::new(&binary);
    if let Some(sub) = subcommand {
        cmd.arg(sub);
    }
    cmd.arg("--socket")
        .arg(socket_path)
        .arg("--log-file")
        .arg(&log_file)
        .arg("--log-level")
        .arg("debug")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());

    #[cfg(unix)]
    unsafe {
        use std::os::unix::process::CommandExt;
        cmd.as_std_mut()
            .pre_exec(|| nix::unistd::setsid().map(|_| ()).map_err(|e| std::io::Error::from_raw_os_error(e as i32)));
    }

    let mut child = cmd.spawn().map_err(ClientError::SpawnFailed)?;

    for _ in 0..50 {
        match child.try_wait() {
            Ok(Some(status)) if !status.success() => {
                return Err(ClientError::SpawnFailed(io::Error::other(format!("Daemon exited with status: {status}"))));
            }
            Ok(_) => {}
            Err(err) => return Err(ClientError::SpawnFailed(err)),
        }

        tokio::time::sleep(Duration::from_millis(100)).await;
        if UnixStream::connect(socket_path).await.is_ok() {
            tokio::spawn(async move {
                match child.wait().await {
                    Ok(status) => tracing::debug!(%status, "aether-lspd launcher reaped"),
                    Err(err) => tracing::warn!(%err, "Failed to reap aether-lspd launcher"),
                }
            });
            return Ok(());
        }
    }

    let _ = child.kill().await;
    let _ = child.wait().await;
    Err(ClientError::SpawnTimeout)
}

fn find_daemon_binary() -> ClientResult<(PathBuf, Option<&'static str>)> {
    let exe = std::env::current_exe().ok();
    let exe_dir = exe.as_deref().and_then(|p| p.parent());

    let standalone_candidates = [
        exe_dir.map(|dir| dir.join("aether-lspd")),
        exe_dir.and_then(|dir| dir.parent()).map(|dir| dir.join("aether-lspd")),
        which_aether_lspd(),
        Some(PathBuf::from("target/debug/aether-lspd")),
        Some(PathBuf::from("target/release/aether-lspd")),
        Some(PathBuf::from("../../target/debug/aether-lspd")),
        Some(PathBuf::from("../../target/release/aether-lspd")),
    ];

    for candidate in standalone_candidates.into_iter().flatten() {
        if candidate.exists() {
            return Ok((candidate, None));
        }
    }

    if let Some(exe) = exe {
        return Ok((exe, Some("lspd")));
    }

    Err(ClientError::DaemonBinaryNotFound("aether-lspd not found".into()))
}

fn which_aether_lspd() -> Option<PathBuf> {
    std::env::var_os("PATH")
        .and_then(|paths| std::env::split_paths(&paths).map(|path| path.join("aether-lspd")).find(|path| path.exists()))
}
