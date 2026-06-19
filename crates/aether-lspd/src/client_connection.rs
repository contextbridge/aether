use crate::protocol::{
    DaemonRequest, DaemonResponse, LspErrorResponse, ProtocolError, extract_document_uri, read_frame, write_frame,
};
use crate::workspace_registry::WorkspaceRegistry;
use crate::workspace_session::WorkspaceSession;
use serde_json::Value;
use std::sync::Arc;
use tokio::io::{ReadHalf, WriteHalf, split};
use tokio::net::UnixStream;
use tokio::spawn;
use tokio::sync::mpsc;

const LSP_CONTENT_MODIFIED: i32 = -32801;
const TRANSIENT_RETRY_LIMIT: u32 = 3;
const TRANSIENT_RETRY_DELAY: std::time::Duration = std::time::Duration::from_millis(500);

enum ConnectionState {
    Uninitialized,
    Bound { session: Arc<WorkspaceSession> },
}

#[tracing::instrument(skip(stream, registry), fields(%client_id))]
pub async fn handle_client(stream: UnixStream, registry: WorkspaceRegistry, client_id: uuid::Uuid) {
    let (reader, writer) = split(stream);
    let (response_tx, response_rx) = mpsc::channel::<DaemonResponse>(100);
    let writer_task = spawn(run_writer(writer, response_rx));
    run_reader(reader, registry, client_id, response_tx).await;
    let _ = writer_task.await;
}

async fn run_writer(mut writer: WriteHalf<UnixStream>, mut response_rx: mpsc::Receiver<DaemonResponse>) {
    while let Some(response) = response_rx.recv().await {
        if let Err(err) = write_frame(&mut writer, &response).await {
            tracing::debug!(%err, "Error writing daemon response");
            break;
        }
    }
}

async fn run_reader(
    mut reader: ReadHalf<UnixStream>,
    registry: WorkspaceRegistry,
    client_id: uuid::Uuid,
    response_tx: mpsc::Sender<DaemonResponse>,
) {
    tracing::debug!("Client connected: {}", client_id);
    let mut state = ConnectionState::Uninitialized;

    loop {
        let request: Option<DaemonRequest> = match read_frame(&mut reader).await {
            Ok(Some(request)) => Some(request),
            Ok(None) => break,
            Err(err) => {
                tracing::debug!(%err, "Error reading client request");
                break;
            }
        };

        match request {
            Some(DaemonRequest::Ping) => {
                let _ = response_tx.send(DaemonResponse::Pong).await;
            }
            Some(DaemonRequest::Disconnect) => break,
            Some(DaemonRequest::Initialize(init)) => {
                match registry.get_or_spawn(&init.workspace_root, init.language).await {
                    Ok(session) => {
                        state = ConnectionState::Bound { session };
                        let _ = response_tx.send(DaemonResponse::Initialized).await;
                    }
                    Err(err) => {
                        let _ = response_tx.send(DaemonResponse::Error(ProtocolError::new(err.to_string()))).await;
                    }
                }
            }
            Some(DaemonRequest::LspCall { client_id, method, params }) => {
                let ConnectionState::Bound { session } = &state else {
                    let _ = send_not_initialized(client_id, &response_tx).await;
                    continue;
                };

                let opened_uri = if let Some(uri) = extract_document_uri(&method, &params) {
                    let _ = session.ensure_document_open(&uri).await;
                    Some(uri)
                } else {
                    None
                };

                let result = request_with_retry(session, &method, params, TRANSIENT_RETRY_LIMIT).await;

                if let Some(uri) = opened_uri {
                    session.close_document(&uri).await;
                }

                let _ = response_tx.send(DaemonResponse::LspResult { client_id, result }).await;
            }
            Some(DaemonRequest::GetDiagnostics { client_id, uri }) => {
                let ConnectionState::Bound { session } = &state else {
                    let _ = send_not_initialized(client_id, &response_tx).await;
                    continue;
                };

                let diagnostics = session.get_diagnostics(uri.as_ref()).await;
                let result = serde_json::to_value(&diagnostics)
                    .map_err(|err| LspErrorResponse { code: -1, message: err.to_string() });
                let _ = response_tx.send(DaemonResponse::LspResult { client_id, result }).await;
            }
            Some(DaemonRequest::QueueDiagnosticRefresh { client_id, uri }) => {
                let ConnectionState::Bound { session } = &state else {
                    let _ = send_not_initialized(client_id, &response_tx).await;
                    continue;
                };

                session.queue_diagnostic_refresh(uri).await;
                let _ = response_tx.send(DaemonResponse::LspResult { client_id, result: Ok(Value::Null) }).await;
            }
            None => {}
        }
    }
}

async fn send_not_initialized(
    client_id: i64,
    tx: &mpsc::Sender<DaemonResponse>,
) -> Result<(), mpsc::error::SendError<DaemonResponse>> {
    tx.send(DaemonResponse::Error(ProtocolError::with_client_id("Not initialized", client_id))).await
}

async fn request_with_retry(
    session: &WorkspaceSession,
    method: &str,
    params: serde_json::Value,
    max_retries: u32,
) -> Result<serde_json::Value, LspErrorResponse> {
    let mut last_err = None;
    for attempt in 0..=max_retries {
        match session.request_raw(method, params.clone()).await {
            Ok(value) => return Ok(value),
            Err(err) if err.code == LSP_CONTENT_MODIFIED && attempt < max_retries => {
                last_err = Some(err);
                tokio::time::sleep(TRANSIENT_RETRY_DELAY).await;
            }
            Err(err) => return Err(err),
        }
    }
    Err(last_err.unwrap())
}
