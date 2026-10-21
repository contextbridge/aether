// Items reachable only through the `testing` harness look dead and
// unreachable in default builds; the all-features lint gate still checks both.
#![cfg_attr(not(feature = "testing"), allow(dead_code, unused_imports, unreachable_pub))]

pub mod cli;
pub mod error;
pub mod settings;

// The crate's public API is the entry points above. The
// internal module graph opens up only under the `testing` feature, for the
// integration suite's cohesive harness in [`testing`].
#[cfg(all(test, not(feature = "testing")))]
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

internal_modules! {
    app,
    conversation,
    command,
    git_review,
    renderer,
    runtime,
    screens,
    session,
    surfaces,
    theme,
    view,
}

use agent_client_protocol::schema::v2::{Implementation, SessionId};
use agent_client_protocol::{Client, ConnectTo};
use client_core::session::Session;

use app::App;
use error::AppError;
use renderer::Renderer;
use settings::UiSettings;
use std::fs::create_dir_all;
use std::path::Path;
use tracing_appender::rolling::daily;
use tracing_subscriber::EnvFilter;

/// Launch the Wisp TUI with the given agent subprocess command.
pub async fn run_tui(agent_command: &str, settings: UiSettings, log_dir: Option<&str>) -> Result<(), AppError> {
    setup_logging(log_dir.map(Path::new));
    let session = Session::connect(client_info(), agent_command).await?;
    run_with_session(session, settings).await
}

/// Launch the TUI attached to an Aether remote host over an established transport.
pub async fn run_remote_tui(
    transport: impl ConnectTo<Client> + 'static,
    requested_session: Option<SessionId>,
    settings: UiSettings,
    log_dir: Option<&Path>,
) -> Result<(), AppError> {
    setup_logging(log_dir);
    let session = Session::connect_remote_to(client_info(), transport, requested_session).await?;
    run_with_session(session, settings).await
}

/// Run the TUI from an already-initialized ACP session.
pub async fn run_with_session(session: Session, settings: UiSettings) -> Result<(), AppError> {
    let (app, event_rx, client_handle) = App::from_session(session, settings);
    runtime::run(app, Renderer::new(), event_rx, client_handle).await
}

/// How Wisp identifies itself to agents during ACP initialization.
pub fn client_info() -> Implementation {
    Implementation::new("wisp", env!("CARGO_PKG_VERSION"))
}

pub fn init_file_logging(dir: &Path, file_name: &str, default_filter: &str) {
    create_dir_all(dir).ok();
    let _ = tracing_subscriber::fmt()
        .with_writer(daily(dir, file_name))
        .with_ansi(false)
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(default_filter)))
        .try_init();
}

fn setup_logging(log_dir: Option<&Path>) {
    init_file_logging(log_dir.unwrap_or(Path::new(DEFAULT_LOG_DIR)), "wisp.log", "info");
}

pub const DEFAULT_LOG_DIR: &str = "/tmp/wisp-logs";
