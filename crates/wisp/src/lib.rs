// Items reachable only through the `testing` harness look dead and
// unreachable in default builds; the all-features lint gate still checks both.
#![cfg_attr(not(feature = "testing"), allow(dead_code, unused_imports, unreachable_pub))]

pub mod cli;
pub mod error;
pub mod settings;

// internal module graph opens up only under the `testing` feature, for the
// integration suite's cohesive harness in [`testing`].
#[cfg(all(test, not(feature = "testing")))]
mod testing;
#[cfg(feature = "testing")]
pub mod testing;

#[cfg(feature = "testing")]
pub mod app;
#[cfg(not(feature = "testing"))]
mod app;
#[cfg(feature = "testing")]
pub mod command;
#[cfg(not(feature = "testing"))]
mod command;
#[cfg(feature = "testing")]
pub mod conversation;
#[cfg(not(feature = "testing"))]
mod conversation;
#[cfg(feature = "testing")]
pub mod platform;
#[cfg(not(feature = "testing"))]
mod platform;
#[cfg(feature = "testing")]
pub mod renderer;
#[cfg(not(feature = "testing"))]
mod renderer;
#[cfg(feature = "testing")]
pub mod runtime;
#[cfg(not(feature = "testing"))]
mod runtime;
#[cfg(feature = "testing")]
pub mod screens;
#[cfg(not(feature = "testing"))]
mod screens;
#[cfg(feature = "testing")]
pub mod surfaces;
#[cfg(not(feature = "testing"))]
mod surfaces;
#[cfg(feature = "testing")]
pub mod theme;
#[cfg(not(feature = "testing"))]
mod theme;
#[cfg(feature = "testing")]
pub mod view;
#[cfg(not(feature = "testing"))]
mod view;

use agent_client_protocol::schema::v2::{Implementation, SessionId};
use agent_client_protocol::{AcpAgent, Client, ConnectTo};
use client_core::connection::{Connection, Session};

use app::{App, AppConfig};
use error::AppError;
use platform::Platform;
use renderer::Renderer;
use settings::UiSettings;
use std::env::current_dir;
use std::fs::create_dir_all;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use tracing_appender::rolling::daily;
use tracing_subscriber::EnvFilter;

/// Launch the Wisp TUI with the given agent subprocess command.
pub async fn run_tui(agent_command: &str, settings: UiSettings, log_dir: Option<&str>) -> Result<(), AppError> {
    setup_logging(log_dir.map(Path::new));
    let agent = AcpAgent::from_str(agent_command).map_err(AppError::InvalidAgentCommand)?;
    let working_dir = current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let session = Connection::connect_to(client_info(), agent).await?.open(None, working_dir).await?;
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
    let connection = Connection::connect_to(client_info(), transport).await?;
    run_with_session(connection.open_remote(requested_session).await?, settings).await
}

/// Run the TUI from an already-initialized ACP session.
pub async fn run_with_session(session: Session, settings: UiSettings) -> Result<(), AppError> {
    let (session, session_runtime) = session.into_core();
    let app = App::new(AppConfig { session, settings, platform: Platform::system() });
    runtime::run(app, Renderer::new(), session_runtime).await
}

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
