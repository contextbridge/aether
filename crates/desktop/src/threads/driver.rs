use crate::target::{ConnectError, ConnectMode, Connector, InstanceId, SessionTarget};
use acp_utils::client::AcpEvent;
use clankerdiff_gpui_shell::runtime;
use client_core::command::{Command, CommandResult, ReviewSource};
use client_core::runtime::CommandDispatcher;
use client_core::session_core::SessionCoreConfig;
use gpui::{Context, Task};
use std::future::Future;
use std::pin::pin;
use std::sync::Arc;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};
use tokio::task::JoinError;

/// What a thread's driver reports to the thread, in the order it happened.
pub(crate) enum DriverMessage {
    /// The agent's `launch` command started the instance, which later connections reuse.
    Launched(InstanceId),
    /// The session is open. Every later message concerns it.
    Connected(Box<SessionCoreConfig>),
    OpenReview(Option<ReviewSource>),
    ConnectFailed(ConnectError),
    Event(AcpEvent),
    Result(CommandResult),
}

/// Owns a thread's connection task on the shared Tokio runtime, and the gpui task that hands its
/// messages to the thread.
///
/// Dropping the handle aborts both. Aborting the connection task drops the agent connection,
/// which kills a local agent's process.
pub(crate) struct ThreadDriverHandle {
    commands: UnboundedSender<Command>,
    _connection: Task<Result<(), JoinError>>,
    _receive: Task<()>,
}

impl ThreadDriverHandle {
    /// Connects to `target`, first launching its instance if it needs one and has none, and
    /// drives the connection, passing every [`DriverMessage`] to `on_message` on the entity `cx`
    /// belongs to.
    pub(crate) fn spawn<T: 'static>(
        connector: Arc<dyn Connector>,
        target: SessionTarget,
        mode: ConnectMode,
        cx: &mut Context<T>,
        on_message: impl Fn(&mut T, DriverMessage, &mut Context<T>) + 'static,
    ) -> Self {
        let (commands, commands_rx) = unbounded_channel();
        let (messages, mut messages_rx) = unbounded_channel();
        let connection = runtime::spawn(cx, ThreadDriver::run(connector, target, mode, commands_rx, messages));
        let receive = cx.spawn(async move |this, cx| {
            while let Some(message) = messages_rx.recv().await {
                // Everything already queued is handled in one update, so that a burst of streamed
                // chunks notifies observers once instead of once per chunk.
                let batch = std::iter::once(message).chain(std::iter::from_fn(|| messages_rx.try_recv().ok()));
                let batch: Vec<DriverMessage> = batch.collect();
                if this
                    .update(cx, |this, cx| batch.into_iter().for_each(|message| on_message(this, message, cx)))
                    .is_err()
                {
                    break;
                }
            }
        });
        Self { commands, _connection: connection, _receive: receive }
    }

    /// Queues `command` for the connection. A command sent after the connection ended is dropped.
    pub(crate) fn send(&self, command: Command) {
        let _ = self.commands.send(command);
    }
}

/// The connection task: an agent connection and the dispatcher running commands against it.
struct ThreadDriver {
    events: UnboundedReceiver<AcpEvent>,
    dispatcher: CommandDispatcher,
    commands: UnboundedReceiver<Command>,
    messages: UnboundedSender<DriverMessage>,
}

impl ThreadDriver {
    async fn run(
        connector: Arc<dyn Connector>,
        mut target: SessionTarget,
        mode: ConnectMode,
        commands: UnboundedReceiver<Command>,
        messages: UnboundedSender<DriverMessage>,
    ) {
        if target.instance.is_none() {
            match connector.launch(&target).await {
                Ok(Some(instance)) => {
                    target.instance = Some(instance.clone());
                    if messages.send(DriverMessage::Launched(instance)).is_err() {
                        return;
                    }
                }
                Ok(None) => {}
                Err(error) => {
                    let _ = messages.send(DriverMessage::ConnectFailed(error));
                    return;
                }
            }
        }
        let session = match connector.connect(&target, mode).await {
            Ok(session) => session,
            Err(error) => {
                let _ = messages.send(DriverMessage::ConnectFailed(error));
                return;
            }
        };
        let (config, handle, events) = session.into_core(CLIENT_NAME);
        let mut driver = Self { events, dispatcher: CommandDispatcher::new(handle), commands, messages };
        if driver.messages.send(DriverMessage::Connected(Box::new(config))).is_ok() {
            driver.forward(review_source(connector.as_ref(), &target)).await;
        }
        driver.dispatcher.shutdown().await;
    }

    /// Runs commands and forwards events and results until the connection closes or the thread
    /// stops listening.
    async fn forward(&mut self, review: impl Future<Output = Option<ReviewSource>>) {
        let mut review = pin!(review);
        let mut review_pending = true;
        loop {
            let has_pending_tasks = self.dispatcher.has_pending_tasks();
            let message = tokio::select! {
                source = &mut review, if review_pending => {
                    review_pending = false;
                    Some(DriverMessage::OpenReview(source))
                }
                event = self.events.recv() => Some(DriverMessage::Event(event.unwrap_or(AcpEvent::ConnectionClosed))),
                Some(result) = self.dispatcher.next_result(), if has_pending_tasks => Some(DriverMessage::Result(result)),
                command = self.commands.recv() => match command {
                    Some(command) => self.dispatcher.dispatch(command).map(DriverMessage::Result),
                    None => return,
                },
            };
            let Some(message) = message else { continue };
            let closed = matches!(message, DriverMessage::Event(AcpEvent::ConnectionClosed));
            if self.messages.send(message).is_err() || closed {
                return;
            }
        }
    }
}

/// Labels the notices the session core appends to a conversation.
const CLIENT_NAME: &str = "desktop";

async fn review_source(connector: &dyn Connector, target: &SessionTarget) -> Option<ReviewSource> {
    let endpoint = match connector.diff_endpoint(target).await {
        Ok(endpoint) => endpoint?,
        Err(error) => return Some(ReviewSource::Unavailable(error.to_string())),
    };
    let headers = endpoint
        .headers
        .into_iter()
        .map(|(name, value)| format!("{name}: {value}").parse().map_err(|_| ConnectError::InvalidHeader { name }))
        .collect::<Result<_, _>>();
    Some(match headers {
        Ok(headers) => ReviewSource::WebSocket { url: endpoint.url, headers },
        Err(error) => ReviewSource::Unavailable(error.to_string()),
    })
}
