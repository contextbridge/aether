pub mod app;
pub mod cli;
pub mod composer;
pub mod error;
pub mod render;
pub mod tool_calls;
pub mod transcript;

use acp_utils::client::AcpEvent;
use app::{App, AppConfig};
use crossterm::event::{DisableBracketedPaste, EnableBracketedPaste, Event, EventStream};
use crossterm::execute;
use error::AppError;
use futures::StreamExt;
use ratatui::{DefaultTerminal, TerminalOptions, Viewport};
use std::future::pending;
use std::io;
use std::time::{Duration, Instant};
use tokio::select;
use tokio::sync::mpsc;
use tokio::time::{MissedTickBehavior, interval};
use wisp::runtime_state::RuntimeState;
use wisp::settings::WispSettings;

/// Launch the experimental ratatui TUI with the given agent subprocess command.
pub async fn run_tui(agent_command: &str, settings: WispSettings) -> Result<(), AppError> {
    wisp::setup_logging(Some(DEFAULT_LOG_DIR));
    let state = RuntimeState::new(agent_command, settings).await?;
    run_with_state(state).await
}

/// Run the TUI from an already-initialized [`RuntimeState`].
pub async fn run_with_state(state: RuntimeState) -> Result<(), AppError> {
    let RuntimeState { session_id, agent_name, settings, event_rx, prompt_handle, workspace_status, .. } = state;

    let app = App::new(AppConfig { session_id, agent_name, workspace_status, prompt_handle, settings });
    run_app(app, event_rx).await
}

pub const DEFAULT_LOG_DIR: &str = "/tmp/wisp-next-logs";

const VIEWPORT_HEIGHT: u16 = 15;
const MAX_ACP_EVENTS_PER_FRAME: usize = 1_000;

async fn run_app(mut app: App, mut event_rx: mpsc::UnboundedReceiver<AcpEvent>) -> Result<(), AppError> {
    let mut terminal = ratatui::init_with_options(TerminalOptions { viewport: Viewport::Inline(VIEWPORT_HEIGHT) });

    let result = match execute!(io::stdout(), EnableBracketedPaste) {
        Ok(()) => event_loop(&mut terminal, &mut app, &mut event_rx).await,
        Err(e) => Err(AppError::Io(e)),
    };

    let _ = execute!(io::stdout(), DisableBracketedPaste);
    ratatui::restore();
    result
}

async fn event_loop(
    terminal: &mut DefaultTerminal,
    app: &mut App,
    event_rx: &mut mpsc::UnboundedReceiver<AcpEvent>,
) -> Result<(), AppError> {
    let mut terminal_events = EventStream::new();
    let mut tick_interval = {
        let mut tick = interval(Duration::from_millis(100));
        tick.set_missed_tick_behavior(MissedTickBehavior::Skip);
        tick
    };

    render::sync_terminal(terminal, app)?;
    loop {
        let tick_fut = async {
            if !app.wants_tick() {
                pending::<()>().await;
            }
            tick_interval.tick().await;
        };

        select! {
            terminal_event = terminal_events.next() => {
                match terminal_event {
                    Some(Ok(event)) => on_terminal_event(terminal, app, event)?,
                    Some(Err(e)) => return Err(AppError::Io(e)),
                    None => return Ok(()),
                }
            }

            acp_event = event_rx.recv() => {
                let Some(first_event) = acp_event else { return Ok(()); };
                for event in collect_batch(first_event, MAX_ACP_EVENTS_PER_FRAME, || event_rx.try_recv().ok()) {
                    app.on_acp_event(event);
                }
            }

            () = tick_fut => app.on_tick(Instant::now()),
        }

        if app.exit_requested() {
            return Ok(());
        }
        render::sync_terminal(terminal, app)?;
    }
}

fn on_terminal_event(terminal: &mut DefaultTerminal, app: &mut App, event: Event) -> Result<(), AppError> {
    match event {
        Event::Key(key) => app.on_key(key),
        Event::Paste(text) => app.on_paste(&text),
        Event::Resize(_, _) => terminal.autoresize()?,
        _ => {}
    }
    Ok(())
}

fn collect_batch<T>(first: T, max: usize, mut try_next: impl FnMut() -> Option<T>) -> Vec<T> {
    let mut events = vec![first];
    while events.len() < max {
        match try_next() {
            Some(event) => events.push(event),
            None => break,
        }
    }
    events
}
