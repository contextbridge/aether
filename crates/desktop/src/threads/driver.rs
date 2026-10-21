use crate::runtime;
use crate::target::{ConnectError, ConnectMode, Connector, SessionTarget};
use acp_utils::client::{AcpClient, AcpEvent};
use client_core::command::{Command, CommandResult};
use client_core::runtime::{CommandDispatcher, NoHost};
use client_core::session::Session;
use client_core::session_core::SessionCoreConfig;
use gpui::{Context, Task};
use std::convert::Infallible;
use std::sync::Arc;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};
use tokio::task::JoinError;

/// What a thread's driver reports to the thread, in the order it happened.
pub(crate) enum DriverMessage {
    /// The session is open. Every later message concerns it.
    Connected(Box<SessionCoreConfig>),
    ConnectFailed(ConnectError),
    Event(AcpEvent),
    Result(CommandResult<Infallible>),
}

/// Owns a thread's connection task on the shared Tokio runtime, and the gpui task that hands its
/// messages to the thread.
///
/// Dropping the handle aborts both. Aborting the connection task drops the agent connection,
/// which kills a local agent's process.
pub(crate) struct ThreadDriverHandle {
    commands: UnboundedSender<Command<Infallible>>,
    _connection: Task<Result<(), JoinError>>,
    _receive: Task<()>,
}

impl ThreadDriverHandle {
    /// Connects to `target` and drives the connection, passing every [`DriverMessage`] to
    /// `on_message` on the entity `cx` belongs to.
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
                if this.update(cx, |this, cx| on_message(this, message, cx)).is_err() {
                    break;
                }
            }
        });
        Self { commands, _connection: connection, _receive: receive }
    }

    /// Queues `command` for the connection. A command sent after the connection ended is dropped.
    pub(crate) fn send(&self, command: Command<Infallible>) {
        let _ = self.commands.send(command);
    }
}

/// The connection task: an agent connection and the dispatcher running commands against it.
struct ThreadDriver {
    events: UnboundedReceiver<AcpEvent>,
    dispatcher: CommandDispatcher<NoHost>,
    commands: UnboundedReceiver<Command<Infallible>>,
    messages: UnboundedSender<DriverMessage>,
}

impl ThreadDriver {
    async fn run(
        connector: Arc<dyn Connector>,
        target: SessionTarget,
        mode: ConnectMode,
        commands: UnboundedReceiver<Command<Infallible>>,
        messages: UnboundedSender<DriverMessage>,
    ) {
        let session = match connector.connect(&target, mode).await {
            Ok(session) => session,
            Err(error) => {
                let _ = messages.send(DriverMessage::ConnectFailed(error));
                return;
            }
        };
        let Session { client, response, working_dir, workspace_status, workspace_access } = session;
        let AcpClient { initialize_response, event_rx, handle } = client;
        let config = SessionCoreConfig {
            initialize_response,
            session_response: response,
            working_dir,
            workspace_status,
            workspace_access,
            client_name: CLIENT_NAME.to_owned(),
        };
        let mut driver =
            Self { events: event_rx, dispatcher: CommandDispatcher::new(handle, NoHost), commands, messages };
        if driver.messages.send(DriverMessage::Connected(Box::new(config))).is_ok() {
            driver.forward().await;
        }
        driver.dispatcher.shutdown().await;
    }

    /// Runs commands and forwards events and results until the connection closes or the thread
    /// stops listening.
    async fn forward(&mut self) {
        loop {
            let has_pending_tasks = self.dispatcher.has_pending_tasks();
            let message = tokio::select! {
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
