pub mod app;
pub mod attachments;
pub mod cli;
pub mod composer;
pub mod diff;
pub mod error;
pub mod keybindings;
pub mod markdown;
pub mod picker;
pub mod presentation;
pub mod render;
pub mod session;
pub mod settings;
pub mod syntax;
pub mod theme;
pub mod tool_calls;
pub mod transcript;
pub mod workspace_status;
pub mod wrap;

use acp_utils::client::AcpEvent;
use app::{App, AppConfig};
use crossterm::event::{
    DisableBracketedPaste, EnableBracketedPaste, Event, EventStream, KeyboardEnhancementFlags,
    PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
};
use crossterm::execute;
use error::AppError;
use futures::StreamExt;
use presentation::TranscriptRenderer;
use ratatui::{DefaultTerminal, TerminalOptions, Viewport};
use session::Session;
use settings::UiSettings;
use std::fs::create_dir_all;
use std::future::pending;
use std::io;
use std::time::{Duration, Instant};
use tokio::select;
use tokio::sync::mpsc;
use tokio::time::{MissedTickBehavior, interval};
use tracing_appender::rolling::daily;
use tracing_subscriber::EnvFilter;

/// Launch the experimental ratatui TUI with the given agent subprocess command.
pub async fn run_tui(agent_command: &str, settings: UiSettings) -> Result<(), AppError> {
    setup_logging(None);
    let session = Session::connect(agent_command, settings).await?;
    run_with_session(session).await
}

/// Run the TUI from an already-initialized ACP session.
pub async fn run_with_session(session: Session) -> Result<(), AppError> {
    let Session { session_id, agent_name, settings, event_rx, prompt_handle, working_dir, workspace_status, .. } =
        session;
    let renderer = TranscriptRenderer::new(&settings);
    let app = App::new(AppConfig { session_id, agent_name, workspace_status, prompt_handle, working_dir, settings });
    run_app(app, renderer, event_rx).await
}

pub fn setup_logging(log_dir: Option<&str>) {
    let dir = log_dir.unwrap_or(DEFAULT_LOG_DIR);
    create_dir_all(dir).ok();
    let _ = tracing_subscriber::fmt()
        .with_writer(daily(dir, "wisp-next.log"))
        .with_ansi(false)
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")))
        .try_init();
}

pub const DEFAULT_LOG_DIR: &str = "/tmp/wisp-next-logs";

const VIEWPORT_HEIGHT: u16 = 15;
const MAX_ACP_EVENTS_PER_FRAME: usize = 1_000;

async fn run_app(
    mut app: App,
    mut renderer: TranscriptRenderer,
    mut event_rx: mpsc::UnboundedReceiver<AcpEvent>,
) -> Result<(), AppError> {
    let mut terminal = ratatui::init_with_options(TerminalOptions { viewport: Viewport::Inline(VIEWPORT_HEIGHT) });
    let mut stdout = io::stdout();
    let flags = KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES
        | KeyboardEnhancementFlags::REPORT_ALTERNATE_KEYS
        | KeyboardEnhancementFlags::REPORT_ALL_KEYS_AS_ESCAPE_CODES;
    let keyboard_enhancement_enabled = execute!(stdout, PushKeyboardEnhancementFlags(flags)).is_ok();

    let result = match execute!(stdout, EnableBracketedPaste) {
        Ok(()) => event_loop(&mut terminal, &mut app, &mut renderer, &mut event_rx).await,
        Err(e) => Err(AppError::Io(e)),
    };

    let _ = execute!(stdout, DisableBracketedPaste);
    if keyboard_enhancement_enabled {
        let _ = execute!(stdout, PopKeyboardEnhancementFlags);
    }
    ratatui::restore();
    result
}

async fn event_loop(
    terminal: &mut DefaultTerminal,
    app: &mut App,
    renderer: &mut TranscriptRenderer,
    event_rx: &mut mpsc::UnboundedReceiver<AcpEvent>,
) -> Result<(), AppError> {
    let mut terminal_events = EventStream::new();
    let mut tick_interval = {
        let mut tick = interval(Duration::from_millis(100));
        tick.set_missed_tick_behavior(MissedTickBehavior::Skip);
        tick
    };

    render::sync_terminal(terminal, app, renderer)?;
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
        render::sync_terminal(terminal, app, renderer)?;
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
