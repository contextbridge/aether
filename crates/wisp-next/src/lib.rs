pub mod annotation;
pub mod app;
pub mod attachments;
pub mod cli;
pub mod composer;
pub mod diff;
pub mod dropped_files;
pub mod edit_buffer;
pub mod effects;
pub mod elicitation;
pub mod error;
pub mod filterable_list;
pub mod generation;
pub mod git_diff;
pub mod keybindings;
pub mod list_view;
pub mod markdown;
pub mod modal;
pub mod picker;
pub mod plan_review;
pub mod plan_tracker;
pub mod plan_view;
pub mod platform;
pub mod presentation;
pub mod progress_indicator;
pub mod prompt_search;
pub mod render;
pub mod render_context;
pub mod screens;
pub mod selection;
pub mod session;
pub mod session_config_view;
pub mod session_loading_buffer;
pub mod session_picker;
pub mod settings;
pub mod settings_overlay;
pub mod status_line;
pub mod surface;
pub mod syntax;
pub mod theme;
pub mod tool_calls;
pub mod transcript;
pub mod widgets;
pub mod workspace_picker;
pub mod workspace_status;
pub mod wrap;

use acp_utils::client::AcpEvent;
use app::{App, AppConfig};
use crossterm::event::{
    DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture, EventStream,
    KeyboardEnhancementFlags, PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
};
use crossterm::{execute, terminal::size};
use error::AppError;
use futures::StreamExt;
use presentation::Presenter;
use ratatui::{DefaultTerminal, TerminalOptions, Viewport};
use session::Session;
use settings::UiSettings;
use std::fs::create_dir_all;
use std::future::pending;
use std::io::{self, Write};
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
    let presenter = Presenter::new(&settings);
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
    run_app(app, presenter, event_rx).await
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

pub fn inline_viewport_height(terminal_height: u16) -> u16 {
    if terminal_height == 0 { 0 } else { terminal_height.saturating_sub(INLINE_SCROLLBACK_RESERVE).max(1) }
}

const INLINE_SCROLLBACK_RESERVE: u16 = 2;
const MAX_ACP_EVENTS_PER_FRAME: usize = 1_000;

async fn run_app(
    mut app: App,
    mut presenter: Presenter,
    mut event_rx: mpsc::UnboundedReceiver<AcpEvent>,
) -> Result<(), AppError> {
    let (_, terminal_height) = size()?;
    let viewport_height = inline_viewport_height(terminal_height);
    let mut terminal = ratatui::init_with_options(TerminalOptions { viewport: Viewport::Inline(viewport_height) });
    let mut stdout = io::stdout();
    let flags = KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES
        | KeyboardEnhancementFlags::REPORT_ALTERNATE_KEYS
        | KeyboardEnhancementFlags::REPORT_ALL_KEYS_AS_ESCAPE_CODES;
    let keyboard_enhancement_enabled = execute!(stdout, PushKeyboardEnhancementFlags(flags)).is_ok();

    let result = match execute!(stdout, EnableBracketedPaste) {
        Ok(()) => event_loop(&mut terminal, &mut app, &mut presenter, &mut event_rx, &mut stdout).await,
        Err(e) => Err(AppError::Io(e)),
    };

    let _ = execute!(stdout, DisableMouseCapture);
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
    presenter: &mut Presenter,
    event_rx: &mut mpsc::UnboundedReceiver<AcpEvent>,
    stdout: &mut impl Write,
) -> Result<(), AppError> {
    let mut terminal_events = EventStream::new();
    let (effect_result_tx, mut effect_result_rx) = mpsc::unbounded_channel();
    let mut tick_interval = {
        let mut tick = interval(Duration::from_millis(100));
        tick.set_missed_tick_behavior(MissedTickBehavior::Skip);
        tick
    };
    let mut capture_enabled = false;

    render::sync_terminal(terminal, app, presenter)?;
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
                    Some(Ok(event)) => app.on_terminal_event(event),
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

            effect_result = effect_result_rx.recv() => {
                if let Some(result) = effect_result {
                    app.on_effect_result(result);
                }
            }

            () = tick_fut => app.on_tick(Instant::now()),
        }

        process_terminal_state(app, stdout, &mut capture_enabled);

        while let Some(effect) = app.take_effect() {
            let event_tx = effect_result_tx.clone();
            tokio::spawn(async move {
                let _ = event_tx.send(effect.execute().await);
            });
        }

        if let Some(theme) = app.take_pending_theme() {
            presenter.set_theme(theme);
        }

        if app.exit_requested() {
            process_terminal_state(app, stdout, &mut capture_enabled);
            return Ok(());
        }
        render::sync_terminal(terminal, app, presenter)?;
    }
}

fn process_terminal_state(app: &mut App, stdout: &mut impl Write, capture_enabled: &mut bool) {
    if app.take_bell() {
        let _ = execute!(stdout, crossterm::style::Print("\x07"));
    }

    let needs_capture = app.needs_mouse_capture();
    if needs_capture && !*capture_enabled {
        let _ = execute!(stdout, EnableMouseCapture);
        *capture_enabled = true;
    } else if !needs_capture && *capture_enabled {
        let _ = execute!(stdout, DisableMouseCapture);
        *capture_enabled = false;
    }
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
