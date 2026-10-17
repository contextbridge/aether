use crate::protocol::{DaemonCall, DaemonRequest, DaemonResponse, LspErrorResponse, frame_reader, frame_writer};
use crate::workspace_registry::{WorkspaceBinding, WorkspaceRegistry};
use futures::{SinkExt, StreamExt};
use tokio::io::{ReadHalf, WriteHalf, split};
use tokio::net::UnixStream;
use tokio::spawn;
use tokio::sync::mpsc;

#[tracing::instrument(skip(stream, registry), fields(%client_id))]
pub(crate) async fn handle_client(stream: UnixStream, registry: WorkspaceRegistry, client_id: uuid::Uuid) {
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

        match request {
            DaemonRequest::Disconnect => break,
            DaemonRequest::Initialize(init) => {
                let response = match registry.bind(&init.workspace_root, init.language).await {
                    Ok(bound) => {
                        binding = Some(bound);
                        DaemonResponse::Initialized
                    }
                    Err(err) => DaemonResponse::InitializeFailed(err.to_string()),
                };
                let _ = response_tx.send(response).await;
            }
            DaemonRequest::Call { id, call } => {
                let ordered = matches!(call, DaemonCall::QueueRefresh { .. });
                let reply = reply(registry.clone(), binding.clone(), id, call, response_tx.clone());
                if ordered {
                    reply.await;
                } else {
                    spawn(reply);
                }
            }
        }
    }
}

async fn reply(
    registry: WorkspaceRegistry,
    binding: Option<WorkspaceBinding>,
    id: i64,
    call: DaemonCall,
    response_tx: mpsc::Sender<DaemonResponse>,
) {
    let result = match binding {
        Some(binding) => registry.call(&binding, call).await,
        None => Err(LspErrorResponse::daemon("Not initialized")),
    };
    let _ = response_tx.send(DaemonResponse::Reply { id, result }).await;
}
