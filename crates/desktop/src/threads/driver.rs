use super::thread::Thread;
use crate::target::{AgentConnector, ConnectError, SessionTarget};
use agent_client_protocol::schema::v2::SessionId;
use clankerdiff_gpui_shell::runtime;
use client_core::command::{Command, ReviewSource};
use client_core::runtime::{Input, SessionRuntime};
use client_core::session_core::SessionCoreConfig;
use gpui::{Context, Task};
use std::future::Future;
use std::pin::pin;
use std::sync::Arc;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};
use tokio::task::JoinError;

pub(crate) enum DriverMessage {
    Launched(String),
    Connected(Box<SessionCoreConfig>),
    OpenReview(Option<ReviewSource>),
    ConnectFailed(ConnectError),
    Input(Input),
}

pub(crate) struct ThreadDriverHandle {
    commands: UnboundedSender<Command>,
    _connection: Task<Result<(), JoinError>>,
    _receive: Task<()>,
}

impl ThreadDriverHandle {
    pub(crate) fn spawn(
        connector: Arc<AgentConnector>,
        target: SessionTarget,
        session_id: Option<SessionId>,
        cx: &mut Context<Thread>,
    ) -> Self {
        let (commands, commands_rx) = unbounded_channel();
        let (messages, mut messages_rx) = unbounded_channel();
        let connection = runtime::spawn(cx, drive(connector, target, session_id, commands_rx, messages));
        let receive = cx.spawn(async move |this, cx| {
            while let Some(message) = messages_rx.recv().await {
                let batch = std::iter::once(message).chain(std::iter::from_fn(|| messages_rx.try_recv().ok()));
                let batch: Vec<DriverMessage> = batch.collect();
                let handled = this.update(cx, |thread, cx| {
                    for message in batch {
                        thread.on_driver_message(message, cx);
                    }
                });
                if handled.is_err() {
                    break;
                }
            }
        });
        Self { commands, _connection: connection, _receive: receive }
    }

    pub(crate) fn send(&self, command: Command) {
        let _ = self.commands.send(command);
    }
}

async fn drive(
    connector: Arc<AgentConnector>,
    mut target: SessionTarget,
    session_id: Option<SessionId>,
    commands: UnboundedReceiver<Command>,
    messages: UnboundedSender<DriverMessage>,
) {
    match connector.launch(&target).await {
        Ok(Some(launched)) => {
            target.set_instance(launched.clone());
            if messages.send(DriverMessage::Launched(launched)).is_err() {
                return;
            }
        }
        Ok(None) => {}
        Err(error) => {
            let _ = messages.send(DriverMessage::ConnectFailed(error));
            return;
        }
    }
    let session = match connector.connect(&target, session_id).await {
        Ok(session) => session,
        Err(error) => {
            let _ = messages.send(DriverMessage::ConnectFailed(error));
            return;
        }
    };
    let (config, mut runtime) = session.into_core();
    if messages.send(DriverMessage::Connected(Box::new(config))).is_ok() {
        forward(&mut runtime, review_source(&connector, &target), commands, &messages).await;
    }
    runtime.shutdown().await;
}

async fn forward(
    runtime: &mut SessionRuntime,
    review: impl Future<Output = Option<ReviewSource>>,
    mut commands: UnboundedReceiver<Command>,
    messages: &UnboundedSender<DriverMessage>,
) {
    let mut review = pin!(review);
    let mut review_pending = true;
    loop {
        let message = tokio::select! {
            source = &mut review, if review_pending => {
                review_pending = false;
                DriverMessage::OpenReview(source)
            }
            input = runtime.next() => DriverMessage::Input(input),
            command = commands.recv() => match command {
                Some(command) => {
                    runtime.dispatch(command);
                    continue;
                }
                None => return,
            },
        };
        let closed =
            matches!(message, DriverMessage::Input(Input::Event(acp_utils::client::AcpEvent::ConnectionClosed)));
        if messages.send(message).is_err() || closed {
            return;
        }
    }
}

async fn review_source(connector: &AgentConnector, target: &SessionTarget) -> Option<ReviewSource> {
    let endpoint = match connector.diff_endpoint(target).await {
        Ok(endpoint) => endpoint?,
        Err(error) => return Some(ReviewSource::Unavailable(error.to_string())),
    };
    Some(match endpoint.connection_headers() {
        Ok(headers) => ReviewSource::WebSocket { url: endpoint.url, headers },
        Err(error) => ReviewSource::Unavailable(error.to_string()),
    })
}
