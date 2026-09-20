use super::actor::SessionIo;
use clankerdiff_protocol::client::ClientCommand;
use clankerdiff_protocol::shared::{DocumentUpdate, Event, RemoteError, RemoteErrorCode};
use clankerdiff_server::{DiffServer, ServerMessageTransport, ServerOptions};
use std::future::Future;
use std::path::PathBuf;
use tokio::sync::mpsc;
use tracing::warn;

pub(crate) struct GitDiffService {
    cwd: PathBuf,
    io: SessionIo,
    connection: Option<Connection>,
}

struct Connection {
    server: DiffServer,
    commands: mpsc::UnboundedSender<ClientCommand>,
}

impl GitDiffService {
    pub(crate) fn new(cwd: PathBuf, io: SessionIo) -> Self {
        Self { cwd, io, connection: None }
    }

    pub(crate) async fn command(&mut self, command: ClientCommand) {
        if matches!(command, ClientCommand::Initialize { .. }) {
            self.connect().await;
        }
        if let Some(connection) = &self.connection {
            let _ = connection.commands.send(command);
        }
    }

    pub(crate) async fn close(&mut self) {
        if let Some(connection) = self.connection.take()
            && let Err(error) = connection.server.shutdown().await
        {
            warn!(%error, "git diff server shutdown failed");
        }
    }

    async fn connect(&mut self) {
        self.close().await;
        let server = match DiffServer::open(&self.cwd, ServerOptions::default()).await {
            Ok(server) => server,
            Err(error) => {
                self.report(error.to_string());
                return;
            }
        };
        let (commands, receiver) = mpsc::unbounded_channel();
        let transport = AcpServerTransport { commands: receiver, io: self.io.clone() };
        if let Err(error) = server.accept(transport) {
            self.report(error.to_string());
            return;
        }
        self.connection = Some(Connection { server, commands });
    }

    fn report(&self, message: String) {
        warn!(%message, "git diff server unavailable");
        self.io.send_git_diff_event(Event::Error(RemoteError::new(RemoteErrorCode::Git, message)));
    }
}

/// A clankerdiff [`ServerMessageTransport`] backed by ACP notifications. Dropping
/// the matching sender ends the server's connection loop.
struct AcpServerTransport {
    commands: mpsc::UnboundedReceiver<ClientCommand>,
    io: SessionIo,
}

impl ServerMessageTransport for AcpServerTransport {
    async fn recv(&mut self) -> Result<ClientCommand, RemoteError> {
        self.commands
            .recv()
            .await
            .ok_or_else(|| RemoteError::new(RemoteErrorCode::Cancelled, "git diff transport closed"))
    }

    fn send(&mut self, message: Event<DocumentUpdate>) -> impl Future<Output = Result<(), RemoteError>> + Send {
        self.io.send_git_diff_event(message);
        std::future::ready(Ok(()))
    }

    async fn close(&mut self) {}
}
