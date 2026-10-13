use crate::protocol::{DaemonRequest, DaemonResponse, ProtocolError, frame_reader, frame_writer};
use crate::workspace_registry::{WorkspaceBinding, WorkspaceRegistry};
use futures::{SinkExt, StreamExt};
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

async fn run_writer(writer: WriteHalf<UnixStream>, mut response_rx: mpsc::Receiver<DaemonResponse>) {
    let mut writer = frame_writer::<_, DaemonResponse>(writer);
    while let Some(response) = response_rx.recv().await {
        if let Err(err) = writer.send(response).await {
            tracing::debug!(%err, "Error writing daemon response");
            break;
        }
    }
}

async fn run_reader(
    reader: ReadHalf<UnixStream>,
    registry: WorkspaceRegistry,
    client_id: uuid::Uuid,
    response_tx: mpsc::Sender<DaemonResponse>,
) {
    tracing::debug!("Client connected: {}", client_id);
    let mut binding: Option<WorkspaceBinding> = None;
    let mut reader = frame_reader::<_, DaemonRequest>(reader);

    while let Some(msg) = reader.next().await {
        let request = match msg {
            Ok(request) => request,
            Err(err) => {
                tracing::debug!(%err, "Error reading client request");
                break;
            }
        };

        let response = match request {
            DaemonRequest::Ping => DaemonResponse::Pong,
            DaemonRequest::Disconnect => break,
            DaemonRequest::Initialize(init) => match registry.bind(&init.workspace_root, init.language).await {
                Ok(bound) => {
                    binding = Some(bound);
                    DaemonResponse::Initialized
                }
                Err(err) => DaemonResponse::Error(ProtocolError::new(err.to_string())),
            },
            DaemonRequest::LspCall { client_id, method, params } => match &binding {
                Some(binding) => {
                    DaemonResponse::LspResult { client_id, result: registry.lsp_call(binding, &method, params).await }
                }
                None => not_initialized(client_id),
            },
            DaemonRequest::GetDiagnostics { client_id, uri } => match &binding {
                Some(binding) => DaemonResponse::LspResult {
                    client_id,
                    result: registry.get_diagnostics(binding, uri.as_ref()).await,
                },
                None => not_initialized(client_id),
            },
            DaemonRequest::QueueDiagnosticRefresh { client_id, uri } => match &binding {
                Some(binding) => DaemonResponse::LspResult {
                    client_id,
                    result: registry.queue_diagnostic_refresh(binding, uri).map(|()| Value::Null),
                },
                None => not_initialized(client_id),
            },
        };
        let _ = response_tx.send(response).await;
    }
}

fn not_initialized(client_id: i64) -> DaemonResponse {
    DaemonResponse::Error(ProtocolError::with_client_id("Not initialized", client_id))
}
