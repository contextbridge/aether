use crate::app::App;
use crate::app::message::Message;
use crate::command::Command;
use crate::error::AppError;
use crate::renderer::Renderer;
use crate::session::terminal::{TerminalSession, inline_viewport_height};
use acp_utils::client::{AcpEvent, AcpPromptHandle};
use crossterm::event::{Event, EventStream};
use crossterm::terminal::size;
use futures::StreamExt;
use ratatui::Viewport;
use std::collections::VecDeque;
use std::future::pending;
use std::time::{Duration, Instant};
use tokio::select;
use tokio::sync::mpsc;
use tokio::time::{MissedTickBehavior, interval};

use super::CommandDispatcher;

const MAX_ACP_EVENTS_PER_FRAME: usize = 1_000;

pub async fn run(
    mut app: App,
    mut renderer: Renderer,
    mut event_rx: mpsc::UnboundedReceiver<AcpEvent>,
    prompt_handle: AcpPromptHandle,
) -> Result<(), AppError> {
    let (_, terminal_height) = size()?;
    let viewport = Viewport::Inline(inline_viewport_height(terminal_height));
    let mut session = TerminalSession::enter(viewport)?;
    let mut dispatcher = CommandDispatcher::new(prompt_handle);

    let initial_commands = app.take_commands();
    dispatch_commands(&mut dispatcher, &mut app, initial_commands);
    let result = run_event_loop(&mut session, &mut app, &mut renderer, &mut event_rx, &mut dispatcher).await;
    drop(session);
    dispatcher.shutdown().await;
    result
}

async fn run_event_loop(
    session: &mut TerminalSession,
    app: &mut App,
    renderer: &mut Renderer,
    event_rx: &mut mpsc::UnboundedReceiver<AcpEvent>,
    dispatcher: &mut CommandDispatcher,
) -> Result<(), AppError> {
    let mut terminal_events = EventStream::new();
    let mut tick_interval = {
        let mut tick = interval(Duration::from_millis(100));
        tick.set_missed_tick_behavior(MissedTickBehavior::Skip);
        tick
    };
    renderer.draw(session.terminal_mut(), app)?;
    loop {
        let tick_fut = async {
            if !app.wants_tick() {
                pending::<()>().await;
            }
            tick_interval.tick().await;
        };
        let has_pending_tasks = dispatcher.has_pending_tasks();

        select! {
            terminal_event = terminal_events.next() => {
                match terminal_event {
                    Some(Ok(event)) => {
                        if matches!(event, Event::Resize(..)) {
                            session.resync_inline_viewport()?;
                        }
                        let commands = app.update(Message::Terminal(event));
                        dispatch_commands(dispatcher, app, commands);
                    }
                    Some(Err(error)) => return Err(AppError::Io(error)),
                    None => return Ok(()),
                }
            }

            acp_event = event_rx.recv() => {
                let Some(event) = acp_event else { return Ok(()); };
                let commands = app.update(Message::Agent(Box::new(event)));
                dispatch_commands(dispatcher, app, commands);
                for _ in 1..MAX_ACP_EVENTS_PER_FRAME {
                    let Ok(event) = event_rx.try_recv() else { break };
                    let commands = app.update(Message::Agent(Box::new(event)));
                    dispatch_commands(dispatcher, app, commands);
                }
            }

            result = dispatcher.next_result(), if has_pending_tasks => {
                if let Some(result) = result {
                    let commands = app.update(Message::CommandFinished(result));
                    dispatch_commands(dispatcher, app, commands);
                }
            }

            () = tick_fut => {
                let commands = app.update(Message::Tick(Instant::now()));
                dispatch_commands(dispatcher, app, commands);
            },
        }

        session.set_mouse_capture(app.needs_mouse_capture());

        if app.exit_requested() {
            return Ok(());
        }
        renderer.draw(session.terminal_mut(), app)?;
    }
}

fn dispatch_commands(dispatcher: &mut CommandDispatcher, app: &mut App, commands: Vec<Command>) {
    let mut pending = VecDeque::from(commands);
    while let Some(command) = pending.pop_front() {
        let Some(result) = dispatcher.dispatch(command) else { continue };
        pending.extend(app.update(Message::CommandFinished(result)));
    }
}
