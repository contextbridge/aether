#![doc = include_str!("../README.md")]

pub mod cli;
pub mod components;
pub mod error;
#[allow(dead_code)]
pub mod git_diff;
pub mod keybindings;
pub mod runtime_state;
mod session_loading_buffer;
pub mod settings;
#[cfg(test)]
pub(crate) mod test_helpers;
pub mod workspace_status;

use acp_utils::client::AcpEvent;
use components::app::{App, AppInfo, EventOutcome};
use error::AppError;
use runtime_state::RuntimeState;
use std::fs::create_dir_all;
use std::future::pending;
use std::io;
use std::time::Duration;
use tokio::sync::mpsc;
use tokio::time::interval;
use tokio::{select, time};
use tracing_appender::rolling::daily;
use tracing_subscriber::EnvFilter;
use tui::{
    Component, CrosstermEvent, Event, MouseCapture, RendererCommand, TerminalConfig, TerminalRuntime, terminal_size,
};

/// Launch the wisp TUI with the given agent subprocess command.
///
/// Sets up logging, connects to the agent via ACP, and runs the interactive
/// terminal event loop until the user exits.
pub async fn run_tui(agent_command: &str) -> Result<(), AppError> {
    setup_logging(None);
    let state = RuntimeState::new(agent_command).await?;
    run_with_state(state).await
}

/// Run the TUI from an already-initialized [`RuntimeState`].
pub async fn run_with_state(state: RuntimeState) -> Result<(), AppError> {
    let RuntimeState {
        session_id,
        agent_name,
        prompt_capabilities,
        config_options,
        auth_methods,
        theme,
        event_rx,
        prompt_handle,
        working_dir,
        workspace_status,
    } = state;

    let app = App::new(AppInfo {
        session_id,
        agent_name,
        prompt_capabilities,
        config_options,
        auth_methods,
        working_dir,
        workspace_status,
        prompt_handle,
    });

    run_app(app, theme, event_rx).await
}

pub fn setup_logging(log_dir: Option<&str>) {
    let dir = log_dir.unwrap_or("/tmp/wisp-logs");
    create_dir_all(dir).ok();
    tracing_subscriber::fmt()
        .with_writer(daily(dir, "wisp.log"))
        .with_ansi(false)
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")))
        .init();
}

fn render(terminal: &mut TerminalRuntime<impl io::Write>, app: &mut App) -> Result<(), AppError> {
    terminal.render_frame(|ctx| app.render(ctx))?;
    Ok(())
}

const MAX_TERMINAL_EVENTS_PER_FRAME: usize = 128;
const MAX_ACP_EVENTS_PER_FRAME: usize = 1_000;

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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BatchOutcome {
    Continue { should_render: bool },
    Exit,
}

async fn process_terminal_event_batch(
    terminal: &mut TerminalRuntime<impl io::Write>,
    app: &mut App,
    events: Vec<CrosstermEvent>,
) -> Result<BatchOutcome, AppError> {
    let mut should_render = false;

    for event in events {
        let tui_event = match event {
            CrosstermEvent::Resize(cols, rows) => {
                terminal.on_resize((cols, rows));
                should_render = true;
                Event::try_from(CrosstermEvent::Resize(cols, rows)).ok()
            }
            event => Event::try_from(event).ok(),
        };

        let Some(tui_event) = tui_event else {
            continue;
        };

        if let Some(commands) = app.on_event(&tui_event).await {
            terminal.apply_commands(commands)?;
            should_render = true;
        }

        if app.exit_requested() {
            return Ok(BatchOutcome::Exit);
        }
    }

    Ok(BatchOutcome::Continue { should_render })
}

fn process_acp_event_batch(app: &mut App, events: Vec<AcpEvent>) -> BatchOutcome {
    let mut should_render = false;
    for event in events {
        if matches!(app.on_acp_event(event), EventOutcome::Render) {
            should_render = true;
        }
        if app.exit_requested() {
            return BatchOutcome::Exit;
        }
    }
    BatchOutcome::Continue { should_render }
}

async fn run_app(
    mut app: App,
    theme: tui::Theme,
    mut event_rx: mpsc::UnboundedReceiver<acp_utils::client::AcpEvent>,
) -> Result<(), AppError> {
    let size = terminal_size().unwrap_or((80, 24));
    let mut terminal = TerminalRuntime::new(
        io::stdout(),
        theme,
        size,
        TerminalConfig { bracketed_paste: true, mouse_capture: MouseCapture::Disabled },
    )?;
    let mut tick_interval = {
        let mut tick = interval(Duration::from_millis(100));
        tick.set_missed_tick_behavior(time::MissedTickBehavior::Skip);
        tick
    };

    let mut last_mouse_capture = false;
    render(&mut terminal, &mut app)?;
    loop {
        let tick_fut = async {
            if !app.wants_tick() {
                pending::<()>().await;
            }
            tick_interval.tick().await;
        };

        select! {
            terminal_event = terminal.next_event() => {
                let Some(first_event) = terminal_event else {
                    return Ok(());
                };

                let events = collect_batch(first_event, MAX_TERMINAL_EVENTS_PER_FRAME, || terminal.try_next_event());
                if events.len() > 1 {
                    tracing::debug!(count = events.len(), "processing terminal event batch");
                }

                match process_terminal_event_batch(&mut terminal, &mut app, events).await? {
                    BatchOutcome::Exit => return Ok(()),
                    BatchOutcome::Continue { should_render: true } => render(&mut terminal, &mut app)?,
                    BatchOutcome::Continue { .. } => {}
                }
            }

            app_event = event_rx.recv() => {
                let Some(event) = app_event else { return Ok(()); };
                let events = collect_batch(event, MAX_ACP_EVENTS_PER_FRAME, || event_rx.try_recv().ok());
                if events.len() > 1 {
                    tracing::debug!(count = events.len(), "processing ACP event batch");
                }
                match process_acp_event_batch(&mut app, events) {
                    BatchOutcome::Exit => return Ok(()),
                    BatchOutcome::Continue { should_render: true } => render(&mut terminal, &mut app)?,
                    BatchOutcome::Continue { .. } => {}
                }
            }

            () = tick_fut => {
                app.on_event(&Event::Tick).await;
                if app.exit_requested() { return Ok(()); }
                render(&mut terminal, &mut app)?;
            }
        }

        let capture = app.needs_mouse_capture();
        if last_mouse_capture != capture {
            terminal.apply_commands(vec![RendererCommand::SetMouseCapture(capture)])?;
            last_mouse_capture = capture;
        }
    }
}

#[cfg(test)]
mod tests {
    use acp_utils::notifications::ContextClearedParams;

    use crate::components::app::test_helpers::make_app;

    use super::*;
    use std::collections::VecDeque;

    #[test]
    fn collect_batch_includes_first_event() {
        let events = collect_batch(CrosstermEvent::Resize(80, 24), 16, || None);

        assert_eq!(events, vec![CrosstermEvent::Resize(80, 24)]);
    }

    #[test]
    fn collect_batch_drains_until_empty() {
        let mut queued = VecDeque::from([
            CrosstermEvent::Resize(81, 24),
            CrosstermEvent::Resize(82, 24),
            CrosstermEvent::Resize(83, 24),
        ]);

        let events = collect_batch(CrosstermEvent::Resize(80, 24), 16, || queued.pop_front());

        assert_eq!(events.len(), 4);
        assert_eq!(events[0], CrosstermEvent::Resize(80, 24));
        assert_eq!(events[3], CrosstermEvent::Resize(83, 24));
    }

    #[test]
    fn collect_batch_respects_max() {
        let mut next_width = 1;
        let events = collect_batch(CrosstermEvent::Resize(0, 24), 4, || {
            next_width += 1;
            Some(CrosstermEvent::Resize(next_width, 24))
        });

        assert_eq!(events.len(), 4);
    }

    #[test]
    fn process_acp_event_batch_exits_on_connection_closed() {
        let mut app = make_app();
        let outcome = process_acp_event_batch(
            &mut app,
            vec![AcpEvent::ContextCleared(ContextClearedParams::default()), AcpEvent::ConnectionClosed],
        );

        assert_eq!(outcome, BatchOutcome::Exit);
    }

    #[test]
    fn process_acp_event_batch_continue_renders_when_any_event_dirties() {
        let mut app = make_app();
        let outcome =
            process_acp_event_batch(&mut app, vec![AcpEvent::ContextCleared(ContextClearedParams::default())]);
        assert_eq!(outcome, BatchOutcome::Continue { should_render: true });
    }

    #[test]
    fn process_acp_event_batch_empty_input_does_not_render() {
        let mut app = make_app();
        let outcome = process_acp_event_batch(&mut app, vec![]);
        assert_eq!(outcome, BatchOutcome::Continue { should_render: false });
    }
}
