use std::future::Future;
use std::sync::Arc;

use acp_utils::client::AcpClientHandle;
use acp_utils::notifications::{GitDiffClosePayload, GitDiffCommandPayload};
use clankerdiff_client::protocol::client::ClientCommand;
use clankerdiff_client::protocol::server::ServerMessage;
use clankerdiff_client::{
    ClientError, ClientMessageTransport, ClientOptions, ClientState, ClientSubscription, DiffClient, MaybeSend,
};
use tokio::sync::mpsc;

use super::tasks::TaskSupervisor;
use crate::command::{CommandResult, GitReviewCommand};
use crate::git_review::DiffReviewEvent;

pub(super) struct GitReviewRuntime {
    handle: AcpClientHandle,
    connection: Option<ReviewConnection>,
}

struct ReviewConnection {
    session_id: String,
    client: DiffClient,
    subscription: ClientSubscription,
    messages: mpsc::UnboundedSender<ServerMessage>,
}

impl GitReviewRuntime {
    pub(super) fn new(handle: AcpClientHandle) -> Self {
        Self { handle, connection: None }
    }

    pub(super) fn dispatch(&mut self, command: GitReviewCommand, tasks: &mut TaskSupervisor) -> Option<CommandResult> {
        match command {
            GitReviewCommand::Open { session_id } => {
                self.open(&session_id);
                None
            }
            GitReviewCommand::Event(event) => self.on_event(event, tasks),
            GitReviewCommand::Forward(message) => {
                self.forward(message);
                None
            }
            GitReviewCommand::Close => {
                self.close();
                None
            }
        }
    }

    pub(super) fn is_active(&self) -> bool {
        self.connection.is_some()
    }

    pub(super) async fn changed(&mut self) -> Option<Arc<ClientState>> {
        match &mut self.connection {
            Some(connection) => connection.subscription.changed().await.ok(),
            None => std::future::pending().await,
        }
    }

    pub(super) fn close(&mut self) {
        if let Some(connection) = self.connection.take() {
            let _ = self.handle.notify(GitDiffClosePayload { session_id: connection.session_id });
        }
    }

    fn open(&mut self, session_id: &str) {
        self.close();
        let (messages, receiver) = mpsc::unbounded_channel();
        let transport =
            AcpClientTransport { handle: self.handle.clone(), session_id: session_id.to_string(), messages: receiver };
        let client = DiffClient::spawn(transport, ClientOptions::default());
        let subscription = client.subscribe();
        self.connection = Some(ReviewConnection { session_id: session_id.to_string(), client, subscription, messages });
    }

    fn on_event(&mut self, event: DiffReviewEvent, tasks: &mut TaskSupervisor) -> Option<CommandResult> {
        let Some(connection) = &self.connection else {
            return Some(CommandResult::GitReviewAction(Err("git review is not connected".to_string())));
        };
        match connection.client.dispatch(event) {
            Ok(Some(reply)) => {
                tasks.submit_network(async move {
                    let result = match reply.await {
                        Ok(result) => result.map_err(|error| error.to_string()),
                        Err(_) => Err("git review connection closed".to_string()),
                    };
                    CommandResult::GitReviewAction(result)
                });
                None
            }
            Ok(None) => None,
            Err(error) => Some(CommandResult::GitReviewAction(Err(error.to_string()))),
        }
    }

    fn forward(&mut self, message: ServerMessage) {
        if let Some(connection) = &self.connection {
            let _ = connection.messages.send(message);
        }
    }
}

struct AcpClientTransport {
    handle: AcpClientHandle,
    session_id: String,
    messages: mpsc::UnboundedReceiver<ServerMessage>,
}

impl ClientMessageTransport for AcpClientTransport {
    fn send(&mut self, command: ClientCommand) -> impl Future<Output = Result<(), ClientError>> + MaybeSend {
        let result = self
            .handle
            .notify(GitDiffCommandPayload { session_id: self.session_id.clone(), command })
            .map_err(|_| ClientError::Disconnected);
        std::future::ready(result)
    }

    async fn recv(&mut self) -> Result<ServerMessage, ClientError> {
        self.messages.recv().await.ok_or(ClientError::Disconnected)
    }

    async fn close(&mut self) {}
}
