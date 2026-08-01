pub(crate) mod annotation;
pub(crate) mod app;
pub(crate) mod attachments;
pub mod cli;
pub(crate) mod composer;
pub(crate) mod diff;
pub(crate) mod dropped_files;
pub(crate) mod edit_buffer;
pub(crate) mod elicitation;
pub mod error;
pub(crate) mod filterable_list;
pub(crate) mod generation;
pub(crate) mod git_diff;
pub(crate) mod keybindings;
pub(crate) mod list_view;
pub(crate) mod markdown;
pub(crate) mod modal;
pub(crate) mod picker;
pub(crate) mod plan_review;
pub(crate) mod plan_tracker;
pub(crate) mod plan_view;
pub(crate) mod platform;
pub(crate) mod progress_indicator;
pub(crate) mod prompt_search;
pub(crate) mod renderer;
pub(crate) mod screens;
pub(crate) mod selection;
pub mod session;
pub(crate) mod session_config_view;
pub(crate) mod session_loading_buffer;
pub(crate) mod session_picker;
pub mod settings;
pub(crate) mod settings_overlay;
pub(crate) mod status_line;
pub(crate) mod surface;
pub(crate) mod syntax;
pub(crate) mod tasks;
pub(crate) mod terminal;
#[doc(hidden)]
pub mod test_support;
pub(crate) mod theme;
pub(crate) mod tool_calls;
pub(crate) mod transcript;
pub(crate) mod widgets;
pub(crate) mod workspace_picker;
pub(crate) mod workspace_status;
pub(crate) mod wrap;

use acp_utils::client::AcpEvent;
use app::{App, AppConfig, RuntimeEffect};
use crossterm::event::{Event, EventStream};
use crossterm::terminal::{Clear, ClearType};
use crossterm::{execute, terminal::size};
use error::AppError;
use futures::StreamExt;
use ratatui::{DefaultTerminal, TerminalOptions, Viewport};
use renderer::Renderer;
use session::Session;
use settings::UiSettings;
use std::fs::create_dir_all;
use std::future::pending;
use std::io;
use std::time::{Duration, Instant};
use terminal::{StdTerminalIo, TerminalModes, inline_viewport_height, resync_inline_viewport, run_terminal_lifecycle};
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
    let Session {
        session_id,
        agent_name,
        prompt_capabilities,
        session_capabilities,
        config_options,
        auth_methods,
        settings,
        event_rx,
        prompt_handle,
        working_dir,
        workspace_status,
    } = session;
    let renderer = Renderer::new(&settings);
    let app = App::new(AppConfig {
        session_id,
        agent_name,
        workspace_status,
        prompt_capabilities,
        session_capabilities,
        config_options,
        auth_methods,
        prompt_handle,
        working_dir,
        settings,
    });
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

const MAX_ACP_EVENTS_PER_FRAME: usize = 1_000;

async fn run_app(
    mut app: App,
    mut renderer: Renderer,
    mut event_rx: mpsc::UnboundedReceiver<AcpEvent>,
) -> Result<(), AppError> {
    let (_, terminal_height) = size()?;
    let viewport = Viewport::Inline(inline_viewport_height(terminal_height));

    // `run_terminal_lifecycle` owns the whole init -> setup -> event loop ->
    // teardown sequence and restores the terminal on every return path, including
    // a ratatui init failure that strands raw mode and a panic unwinding out of
    // the event loop.
    run_terminal_lifecycle(StdTerminalIo::new(), TerminalOptions { viewport }, |mut terminal, mut modes| {
        let app = &mut app;
        let renderer = &mut renderer;
        let event_rx = &mut event_rx;
        async move { event_loop(&mut terminal, app, renderer, event_rx, &mut modes).await }
    })
    .await?;
    Ok(())
}

async fn event_loop(
    terminal: &mut DefaultTerminal,
    app: &mut App,
    renderer: &mut Renderer,
    event_rx: &mut mpsc::UnboundedReceiver<AcpEvent>,
    modes: &mut TerminalModes<StdTerminalIo>,
) -> Result<(), AppError> {
    let mut terminal_events = EventStream::new();
    let (task_result_tx, mut task_result_rx) = mpsc::unbounded_channel();
    let mut tick_interval = {
        let mut tick = interval(Duration::from_millis(100));
        tick.set_missed_tick_behavior(MissedTickBehavior::Skip);
        tick
    };
    let mut stdout = io::stdout();

    renderer.draw(terminal, app)?;
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
                    Some(Ok(event)) => {
                        if matches!(event, Event::Resize(_, _)) {
                            resync_inline_viewport(terminal)?;
                        }
                        app.on_terminal_event(event);
                    }
                    Some(Err(e)) => return Err(AppError::Io(e)),
                    None => return Ok(()),
                }
            }

            acp_event = event_rx.recv() => {
                let Some(event) = acp_event else { return Ok(()); };
                app.on_acp_event(event);
                // Bounded so a burst of agent traffic cannot starve terminal
                // input, which is what cancelling the burst goes through.
                for _ in 1..MAX_ACP_EVENTS_PER_FRAME {
                    let Ok(event) = event_rx.try_recv() else { break };
                    app.on_acp_event(event);
                }
            }

            task_result = task_result_rx.recv() => {
                if let Some(result) = task_result {
                    app.on_task_result(result);
                }
            }

            () = tick_fut => app.on_tick(Instant::now()),
        }

        while let Some(effect) = app.take_effect() {
            match effect {
                RuntimeEffect::Spawn(task) => {
                    let result_tx = task_result_tx.clone();
                    tokio::spawn(async move {
                        let _ = result_tx.send(task.execute().await);
                    });
                }
                RuntimeEffect::SetTheme(theme) => renderer.set_theme(theme),
                RuntimeEffect::RingBell => {
                    let _ = execute!(stdout, crossterm::style::Print("\x07"));
                }
                RuntimeEffect::PurgeScrollback => {
                    // Purge only the scrollback; Clear(All) would desync the inline viewport's diff buffer.
                    execute!(stdout, Clear(ClearType::Purge))?;
                }
            }
        }
        modes.set_mouse_capture(app.needs_mouse_capture());

        if app.exit_requested() {
            return Ok(());
        }
        renderer.draw(terminal, app)?;
    }
}
