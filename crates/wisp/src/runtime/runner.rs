use crate::app::App;
use crate::app::message::Message;
use crate::command::{Command, HostCommand};
use crate::error::AppError;
use crate::platform::terminal::{TerminalSession, inline_viewport_height};
use crate::renderer::Renderer;
use client_core::runtime::{Input, SessionRuntime};
use crossterm::event::{Event, EventStream};
use crossterm::terminal::size;
use futures::StreamExt;
use ratatui::Viewport;
use std::future::pending;
use std::time::{Duration, Instant};
use tokio::select;
use tokio::time::{MissedTickBehavior, interval};

use super::TuiHost;

const MAX_ACP_EVENTS_PER_FRAME: usize = 1_000;

pub(crate) async fn run(mut app: App, mut renderer: Renderer, runtime: SessionRuntime) -> Result<(), AppError> {
    let mut runtimes = Runtimes { session: runtime, host: TuiHost::default() };
    let result = async {
        let (_, terminal_height) = size()?;
        let viewport = Viewport::Inline(inline_viewport_height(terminal_height));
        let mut session = TerminalSession::enter(viewport)?;
        runtimes.dispatch(app.take_commands());
        run_event_loop(&mut session, &mut app, &mut renderer, &mut runtimes).await
    }
    .await;
    runtimes.session.shutdown().await;
    runtimes.host.shutdown().await;
    result
}

pub trait Host {
    fn execute(&mut self, command: HostCommand);
}

pub(crate) struct Runtimes<THost> {
    pub(crate) session: SessionRuntime,
    pub(crate) host: THost,
}

impl<THost: Host> Runtimes<THost> {
    pub(crate) fn deliver(&mut self, app: &mut App, message: Message) {
        let commands = app.update(message);
        self.dispatch(commands);
    }

    pub(crate) fn dispatch(&mut self, commands: Vec<Command>) {
        for command in commands {
            match command {
                Command::Core(command) => self.session.dispatch(command),
                Command::Host(command) => self.host.execute(command),
            }
        }
    }
}

async fn run_event_loop(
    session: &mut TerminalSession,
    app: &mut App,
    renderer: &mut Renderer,
    runtimes: &mut Runtimes<TuiHost>,
) -> Result<(), AppError> {
    let mut terminal_events = EventStream::new();
    let mut tick_interval = {
        let mut tick = interval(Duration::from_millis(100));
        tick.set_missed_tick_behavior(MissedTickBehavior::Skip);
        tick
    };
    renderer.draw(session.terminal_mut(), app)?;
    loop {
        let tick = async {
            if !app.wants_tick() {
                pending::<()>().await;
            }
            tick_interval.tick().await;
        };
        let message = select! {
            terminal_event = terminal_events.next() => match terminal_event {
                Some(Ok(event)) => {
                    if matches!(event, Event::Resize(..)) {
                        session.resync_inline_viewport()?;
                    }
                    Message::Terminal(event)
                }
                Some(Err(error)) => return Err(AppError::Io(error)),
                None => return Ok(()),
            },
            input = runtimes.session.next() => Message::Session(Box::new(input)),
            result = runtimes.host.next() => Message::Host(Box::new(result)),
            () = tick => Message::Tick(Instant::now()),
        };
        let agent_event = matches!(&message, Message::Session(input) if matches!(**input, Input::Event(_)));
        runtimes.deliver(app, message);
        if agent_event {
            for _ in 1..MAX_ACP_EVENTS_PER_FRAME {
                let Some(event) = runtimes.session.try_next_event() else { break };
                runtimes.deliver(app, Message::Session(Box::new(Input::Event(event))));
            }
        }

        session.set_mouse_capture(app.needs_mouse_capture());

        if let Some(result) = app.exit_result() {
            return result;
        }
        renderer.draw(session.terminal_mut(), app)?;
    }
}
