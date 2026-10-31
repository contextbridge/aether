use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::process::Stdio;
use std::sync::Arc;

use acp_utils::client::AcpClientHandle;
use acp_utils::notifications::{GitDiffClosePayload, GitDiffCommandPayload};
use agent_client_protocol::schema::v2::SessionId;
use clankerdiff_client::protocol::client::ClientCommand;
use clankerdiff_client::protocol::server::ServerMessage;
use clankerdiff_client::transport::StreamTransport;
use clankerdiff_client::{
    ClientError, ClientMessageTransport, ClientOptions, ClientState, ClientSubscription, ConnectionHeader,
    ConnectionState, DiffClient, DiffReviewEvent, LookupReply, MaybeSend, RemoteError, RemoteErrorCode,
};
use clankerdiff_remote::Invocation;
use clankerdiff_server::{DiffServer, ServerError, ServerOptions};
use tokio::process::{Child, Command};
use tokio::sync::mpsc;
use tokio::task::JoinSet;

use crate::command::{CommandResult, GitReviewCommand, Reply, ReviewSource};

pub(super) struct GitReviewRuntime {
    handle: AcpClientHandle,
    review: Review,
}

enum Review {
    Closed,
    Opening(Pin<Box<dyn Future<Output = Result<ReviewConnection, ClientError>> + Send>>),
    Open(ReviewConnection),
}

struct ReviewConnection {
    client: DiffClient,
    subscription: ClientSubscription,
    server: ReviewServer,
}

enum ReviewServer {
    Agent { session_id: SessionId, messages: mpsc::UnboundedSender<ServerMessage> },
    Local { _server: DiffServer },
    Process { _child: Child },
    Remote,
}

impl GitReviewRuntime {
    pub(super) fn new(handle: AcpClientHandle) -> Self {
        Self { handle, review: Review::Closed }
    }

    pub(super) fn dispatch(
        &mut self,
        command: GitReviewCommand,
        tasks: &mut JoinSet<CommandResult>,
    ) -> Option<CommandResult> {
        match command {
            GitReviewCommand::Open(source) => self.open(source),
            GitReviewCommand::Event(event) => self.on_event(event, tasks),
            GitReviewCommand::Close => {
                self.close();
                None
            }
        }
    }

    pub(super) async fn changed(&mut self) -> Option<Arc<ClientState>> {
        loop {
            match &mut self.review {
                Review::Closed => return std::future::pending().await,
                Review::Opening(opening) => match opening.as_mut().await {
                    Ok(connection) => self.review = Review::Open(connection),
                    Err(error) => {
                        self.review = Review::Closed;
                        return Some(failed(error));
                    }
                },
                Review::Open(connection) => return connection.subscription.changed().await.ok(),
            }
        }
    }

    pub(super) fn close(&mut self) {
        if let Review::Open(ReviewConnection { server: ReviewServer::Agent { session_id, .. }, .. }) =
            std::mem::replace(&mut self.review, Review::Closed)
        {
            let _ = self.handle.notify(GitDiffClosePayload { session_id: session_id.to_string() });
        }
    }

    pub(super) fn forward(&mut self, session_id: &SessionId, message: ServerMessage) {
        if let Review::Open(ReviewConnection { server: ReviewServer::Agent { session_id: reviewed, messages }, .. }) =
            &self.review
            && reviewed == session_id
        {
            let _ = messages.send(message);
        }
    }

    fn open(&mut self, source: ReviewSource) -> Option<CommandResult> {
        self.close();
        self.review = match source {
            ReviewSource::Agent { session_id } => {
                let (messages, receiver) = mpsc::unbounded_channel();
                let transport = AcpClientTransport {
                    handle: self.handle.clone(),
                    session_id: session_id.clone(),
                    messages: receiver,
                };
                let client = DiffClient::spawn(transport, ClientOptions::default());
                Review::Open(ReviewConnection::new(client, ReviewServer::Agent { session_id, messages }))
            }
            ReviewSource::Local(path) => Review::Opening(Box::pin(open_local(path))),
            ReviewSource::Stdio(invocation) => Review::Opening(Box::pin(open_process(invocation))),
            ReviewSource::WebSocket { url, headers } => Review::Opening(Box::pin(connect(url, headers))),
            ReviewSource::Unavailable(reason) => {
                return Some(CommandResult::GitReview(failed(ClientError::Transport(reason))));
            }
        };
        None
    }

    fn on_event(&mut self, event: DiffReviewEvent, tasks: &mut JoinSet<CommandResult>) -> Option<CommandResult> {
        match (&self.review, event) {
            (Review::Open(connection), DiffReviewEvent::Lookup(lookup)) => {
                let client = connection.client.clone();
                tasks.spawn(async move { CommandResult::Reply(Reply::GitReviewLookup(client.lookup(lookup).await)) });
                None
            }
            (Review::Open(connection), event) => match connection.client.dispatch(event) {
                Ok(Some(reply)) => {
                    tasks.spawn(async move {
                        let result = reply.await.unwrap_or(Err(ClientError::Disconnected));
                        CommandResult::Reply(Reply::GitReviewAction(result))
                    });
                    None
                }
                Ok(None) => None,
                Err(error) => Some(CommandResult::Reply(Reply::GitReviewAction(Err(error)))),
            },
            (_, DiffReviewEvent::Lookup(lookup)) => {
                let reply = LookupReply::new(lookup, Err(ClientError::Disconnected.to_string()));
                Some(CommandResult::Reply(Reply::GitReviewLookup(reply)))
            }
            (_, _) => Some(CommandResult::Reply(Reply::GitReviewAction(Err(ClientError::Disconnected)))),
        }
    }
}

impl ReviewConnection {
    fn new(client: DiffClient, server: ReviewServer) -> Self {
        Self { subscription: client.subscribe(), client, server }
    }
}

struct AcpClientTransport {
    handle: AcpClientHandle,
    session_id: SessionId,
    messages: mpsc::UnboundedReceiver<ServerMessage>,
}

impl ClientMessageTransport for AcpClientTransport {
    fn send(&mut self, command: ClientCommand) -> impl Future<Output = Result<(), ClientError>> + MaybeSend {
        let result = self
            .handle
            .notify(GitDiffCommandPayload { session_id: self.session_id.to_string(), command })
            .map_err(|_| ClientError::Disconnected);
        std::future::ready(result)
    }

    async fn recv(&mut self) -> Result<ServerMessage, ClientError> {
        self.messages.recv().await.ok_or(ClientError::Disconnected)
    }

    async fn close(&mut self) {}
}

async fn open_local(path: PathBuf) -> Result<ReviewConnection, ClientError> {
    let unreadable =
        |error: ServerError| ClientError::Remote(RemoteError::new(RemoteErrorCode::Git, error.to_string()));
    let server = DiffServer::open(path, ServerOptions::default()).await.map_err(unreadable)?;
    let client = DiffClient::from_transport(server.connect().map_err(unreadable)?, ClientOptions::default()).await?;
    Ok(ReviewConnection::new(client, ReviewServer::Local { _server: server }))
}

async fn open_process(Invocation { program, args, dir, env }: Invocation) -> Result<ReviewConnection, ClientError> {
    let mut child = Command::new(&program)
        .args(&args)
        .envs(&env)
        .current_dir(&dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .kill_on_drop(true)
        .spawn()
        .map_err(|error| ClientError::Transport(format!("failed to start `{program}`: {error}")))?;
    let (Some(stdin), Some(stdout)) = (child.stdin.take(), child.stdout.take()) else {
        unreachable!("both pipes were requested at spawn");
    };
    let client = DiffClient::from_messages(StreamTransport::new(stdout, stdin), ClientOptions::default())
        .await
        .map_err(|error| match error {
            ClientError::Disconnected => ClientError::Transport(format!("`{program}` exited before serving a diff")),
            error => error,
        })?;
    Ok(ReviewConnection::new(client, ReviewServer::Process { _child: child }))
}

async fn connect(url: String, headers: Vec<ConnectionHeader>) -> Result<ReviewConnection, ClientError> {
    let client = DiffClient::connect_with_headers(&url, ClientOptions::default(), headers).await?;
    Ok(ReviewConnection::new(client, ReviewServer::Remote))
}

fn failed(error: ClientError) -> Arc<ClientState> {
    Arc::new(ClientState { connection: ConnectionState::Failed(error), ..ClientState::default() })
}
