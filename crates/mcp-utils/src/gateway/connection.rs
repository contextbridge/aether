use super::{ConnectedServer, GatewayError, ServerSpec};
use crate::client::{ClientError, ClientOptions, McpClient};
use aether_auth::OAuthError;
use async_stream::stream;
use futures::{Stream, StreamExt};
use std::pin::pin;
use tokio::sync::{mpsc, oneshot, watch};
use tokio_util::sync::CancellationToken;
use tokio_util::task::TaskTracker;
use utils::mcp_status::{McpServerAuthCapability, McpServerStatus, McpServerStatusEntry};

pub(super) struct Connection {
    name: String,
    supports_oauth: bool,
    state: watch::Receiver<ConnectionState>,
    sign_ins: mpsc::UnboundedSender<Reply>,
}

#[derive(Debug, Clone)]
pub(super) enum ConnectionState {
    Connecting,
    Authenticating,
    NeedsOAuth { challenge: Option<String> },
    Failed { error: String, after_sign_in: bool },
    Connected(ConnectedServer),
}

impl Connection {
    pub(super) fn spawn(
        spec: ServerSpec,
        options: &ClientOptions,
        stop: CancellationToken,
        tasks: &TaskTracker,
    ) -> Self {
        let (state, receiver) = watch::channel(ConnectionState::Connecting);
        let (sign_ins, requests) = mpsc::unbounded_channel();
        let supports_oauth = spec.transport.supports_oauth(options);
        let connection = Self { name: spec.name.clone(), supports_oauth, state: receiver, sign_ins };
        let task = ConnectionTask { spec, options: options.clone(), supports_oauth, state };
        tasks.spawn(task.run(requests, stop));
        connection
    }

    pub(super) fn name(&self) -> &str {
        &self.name
    }

    pub(super) fn connected(&self) -> Option<ConnectedServer> {
        self.state.borrow().connected().cloned()
    }

    pub(super) fn subscribe(&self) -> watch::Receiver<ConnectionState> {
        let mut receiver = self.state.clone();
        receiver.mark_unchanged();
        receiver
    }

    pub(super) async fn settled(&self) {
        let _ = self.subscribe().wait_for(|state| !matches!(state, ConnectionState::Connecting)).await;
    }

    pub(super) async fn sign_in(&self) -> Result<(), GatewayError> {
        let (reply, outcome) = oneshot::channel();
        self.sign_ins.send(reply).map_err(|_| GatewayError::Closed)?;
        outcome.await.unwrap_or(Err(GatewayError::Closed))
    }

    pub(super) fn status(&self) -> McpServerStatusEntry {
        let state = self.state.borrow();
        let auth = if state.allows_sign_in(self.supports_oauth) {
            McpServerAuthCapability::OAuth
        } else {
            McpServerAuthCapability::Unavailable
        };
        McpServerStatusEntry::new(self.name.clone(), state.status()).with_auth_capability(auth)
    }
}

impl ConnectionState {
    fn connected(&self) -> Option<&ConnectedServer> {
        match self {
            Self::Connected(server) => Some(server),
            _ => None,
        }
    }

    fn allows_sign_in(&self, supports_oauth: bool) -> bool {
        supports_oauth
            && match self {
                Self::Connecting => false,
                Self::Failed { after_sign_in, .. } => *after_sign_in,
                Self::Authenticating | Self::NeedsOAuth { .. } | Self::Connected(_) => true,
            }
    }

    fn status(&self) -> McpServerStatus {
        match self {
            Self::Connecting => McpServerStatus::Connecting,
            Self::Authenticating => McpServerStatus::Authenticating,
            Self::NeedsOAuth { .. } => McpServerStatus::NeedsOAuth,
            Self::Failed { error, .. } => McpServerStatus::Failed { error: error.clone() },
            Self::Connected(server) => McpServerStatus::Connected { tool_count: server.tools().len() },
        }
    }
}

type Reply = oneshot::Sender<Result<(), GatewayError>>;

type Update = Result<ConnectedServer, GatewayError>;

enum Credentials {
    Existing,
    SignIn { challenge: Option<String> },
}

struct ConnectionTask {
    spec: ServerSpec,
    options: ClientOptions,
    supports_oauth: bool,
    state: watch::Sender<ConnectionState>,
}

impl ConnectionTask {
    async fn run(self, mut sign_ins: mpsc::UnboundedReceiver<Reply>, stop: CancellationToken) {
        let mut pending = None;
        let mut connection = pin!(self.connection(Credentials::Existing));
        loop {
            tokio::select! {
                () = stop.cancelled() => break,
                Some(update) = connection.next() => self.settle(update, &mut pending),
                Some(reply) = sign_ins.recv() => {
                    if let Some(credentials) = self.sign_in(reply, &mut pending) {
                        connection.set(self.connection(credentials));
                    }
                }
            }
        }
        let server = self.state.borrow().connected().cloned();
        if let Some(server) = server {
            server.client().close().await;
        }
    }

    fn connection(&self, credentials: Credentials) -> impl Stream<Item = Update> + Send + '_ {
        stream! {
            let (name, transport) = (&self.spec.name, self.spec.transport.clone());
            let connected = match credentials {
                Credentials::Existing => McpClient::connect(name, transport, &self.options).await,
                Credentials::SignIn { challenge } => McpClient::authorize(name, transport, &self.options, challenge).await,
            };
            let client = match connected {
                Ok(client) => client,
                Err(error) => {
                    yield Err(error.into());
                    return;
                }
            };
            let _close = client.close_on_drop();
            let mut tool_lists = pin!(client.tool_lists());
            let mut current: Option<ConnectedServer> = None;
            while let Some(tools) = tool_lists.next().await {
                match (tools, &current) {
                    (Ok(tools), Some(server)) if server.tools() == tools.as_slice() => {}
                    (Ok(tools), _) => {
                        let server = ConnectedServer::new(client.clone(), tools);
                        current = Some(server.clone());
                        yield Ok(server);
                    }
                    (Err(error), None) => {
                        yield Err(error.into());
                        return;
                    }
                    (Err(error), Some(_)) => {
                        tracing::warn!(server = name, %error, "Failed to refresh MCP tools; keeping the previous list");
                    }
                }
            }
            yield Err(ClientError::Disconnected { server: name.clone() }.into());
        }
    }

    fn sign_in(&self, reply: Reply, pending: &mut Option<Reply>) -> Option<Credentials> {
        let server = self.spec.name.clone();
        if !self.can_sign_in() {
            let _ = reply.send(Err(ClientError::OAuthUnavailable { server }.into()));
            return None;
        }
        let challenge = match self.state.send_replace(ConnectionState::Authenticating) {
            ConnectionState::NeedsOAuth { challenge } => challenge,
            _ => None,
        };
        if let Some(replaced) = pending.replace(reply) {
            let _ = replaced.send(Err(ClientError::Auth { server, source: OAuthError::UserCancelled }.into()));
        }
        Some(Credentials::SignIn { challenge })
    }

    fn settle(&self, update: Update, pending: &mut Option<Reply>) {
        let (state, result) = match update {
            Ok(server) => (ConnectionState::Connected(server), Ok(())),
            Err(error) => (self.failed(&error), Err(error)),
        };
        self.state.send_replace(state);
        if let Some(reply) = pending.take() {
            let _ = reply.send(result);
        }
    }

    fn failed(&self, error: &GatewayError) -> ConnectionState {
        let server = &self.spec.name;
        match error {
            GatewayError::Client(ClientError::AuthRequired { challenge, .. }) => {
                tracing::warn!(server, "MCP server needs OAuth");
                ConnectionState::NeedsOAuth { challenge: challenge.clone() }
            }
            error => {
                tracing::warn!(server, %error, "MCP server failed");
                let after_sign_in = matches!(*self.state.borrow(), ConnectionState::Authenticating);
                ConnectionState::Failed { error: error.to_string(), after_sign_in }
            }
        }
    }

    fn can_sign_in(&self) -> bool {
        self.state.borrow().allows_sign_in(self.supports_oauth)
    }
}
