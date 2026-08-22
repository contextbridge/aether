// Items reachable only through the `testing` harness look dead in default
// builds; the all-features lint gate still checks dead code for real.
#![cfg_attr(not(feature = "testing"), allow(dead_code, unused_imports))]

pub mod cli;
pub mod error;
pub mod settings;

// The crate's public API is the entry points above plus [`Session`]. The
// internal module graph opens up only under the `testing` feature, for the
// integration suite's cohesive harness in [`testing`].
#[cfg(not(feature = "testing"))]
mod testing;
#[cfg(feature = "testing")]
pub mod testing;

macro_rules! internal_modules {
    ($($name:ident),* $(,)?) => {
        $(
            #[cfg(feature = "testing")]
            pub mod $name;
            #[cfg(not(feature = "testing"))]
            mod $name;
        )*
    };
}

internal_modules!(
    app,
    attachment,
    conversation,
    command,
    file_index,
    git_review,
    renderer,
    request,
    runtime,
    screens,
    session,
    surfaces,
    theme,
    view,
);

pub use session::Session;

use app::App;
use error::AppError;
use renderer::Renderer;
use settings::UiSettings;
use std::fs::create_dir_all;
use tracing_appender::rolling::daily;
use tracing_subscriber::EnvFilter;

/// Launch the Wisp TUI with the given agent subprocess command.
pub async fn run_tui(agent_command: &str, settings: UiSettings, log_dir: Option<&str>) -> Result<(), AppError> {
    setup_logging(log_dir);
    let session = Session::connect(agent_command).await?;
    run_with_session(session, settings).await
}

/// Run the TUI from an already-initialized ACP session.
pub async fn run_with_session(session: Session, settings: UiSettings) -> Result<(), AppError> {
    let (app, event_rx, prompt_handle) = App::from_session(session, settings);
    runtime::run(app, Renderer::new(), event_rx, prompt_handle).await
}

fn setup_logging(log_dir: Option<&str>) {
    let dir = log_dir.unwrap_or(DEFAULT_LOG_DIR);
    create_dir_all(dir).ok();
    let _ = tracing_subscriber::fmt()
        .with_writer(daily(dir, "wisp.log"))
        .with_ansi(false)
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")))
        .try_init();
}

pub const DEFAULT_LOG_DIR: &str = "/tmp/wisp-logs";
