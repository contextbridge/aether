use crate::protocol::{DaemonRequest, DaemonResponse, ProtocolError, read_frame, write_frame};
use crate::workspace_registry::{WorkspaceBinding, WorkspaceRegistry};
use serde_json::Value;
use tokio::io::{ReadHalf, WriteHalf, split};
use tokio::net::UnixStream;
use tokio::spawn;
use tokio::sync::mpsc;

#[tracing::instrument(skip(stream, registry), fields(%client_id))]
pub async fn handle_client(stream: UnixStream, registry: WorkspaceRegistry, client_id: uuid::Uuid) {
    let (reader, writer) = split(stream);
    let (response_tx, response_rx) = mpsc::channel::<DaemonResponse>(100);
    let writer_task = spawn(run_writer(writer, response_rx));
    run_reader(reader, registry, client_id, response_tx).await;
    let _ = writer_task.await;
}

enum ConnectionState {
    Uninitialized,
    Bound { binding: WorkspaceBinding },
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
            Some(DaemonRequest::Initialize(init)) => match registry.bind(&init.workspace_root, init.language).await {
                Ok(binding) => {
                    state = ConnectionState::Bound { binding };
                    let _ = response_tx.send(DaemonResponse::Initialized).await;
                }
                Err(err) => {
                    let _ = response_tx.send(DaemonResponse::Error(ProtocolError::new(err.to_string()))).await;
                }
            },
            Some(DaemonRequest::LspCall { client_id, method, params }) => {
                let ConnectionState::Bound { binding } = &state else {
                    let _ = send_not_initialized(client_id, &response_tx).await;
                    continue;
                };

                let result = registry.lsp_call(binding, &method, params).await;
                let _ = response_tx.send(DaemonResponse::LspResult { client_id, result }).await;
            }
            Some(DaemonRequest::GetDiagnostics { client_id, uri }) => {
                let ConnectionState::Bound { binding } = &state else {
                    let _ = send_not_initialized(client_id, &response_tx).await;
                    continue;
                };

                let result = registry.get_diagnostics(binding, uri.as_ref()).await;
                let _ = response_tx.send(DaemonResponse::LspResult { client_id, result }).await;
            }
            Some(DaemonRequest::QueueDiagnosticRefresh { client_id, uri }) => {
                let ConnectionState::Bound { binding } = &state else {
                    let _ = send_not_initialized(client_id, &response_tx).await;
                    continue;
                };

                let result = registry.queue_diagnostic_refresh(binding, uri).await.map(|()| Value::Null);
                let _ = response_tx.send(DaemonResponse::LspResult { client_id, result }).await;
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
