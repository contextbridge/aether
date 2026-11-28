pub mod app;
pub mod cli;
pub mod command;
pub mod conversation;
pub mod error;
pub mod platform;
pub mod renderer;
pub mod runtime;
pub mod screens;
pub mod settings;
pub mod surfaces;
#[cfg(any(test, feature = "testing"))]
pub mod testing;
pub mod theme;
pub mod view;

use agent_client_protocol::v2::schema::{Implementation, SessionId};
use agent_client_protocol::{AcpAgent, Client, ConnectTo};
use foxfire_core::connection::{Connection, Session};

use app::{App, AppConfig};
use error::AppError;
use renderer::Renderer;
use settings::UiSettings;
use std::env::current_dir;
use std::fs::create_dir_all;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use tracing_appender::rolling::{RollingFileAppender, Rotation};
use tracing_subscriber::EnvFilter;

pub const DEFAULT_LOG_DIR: &str = "/tmp/foxfire-logs";
const MAX_LOG_FILES: usize = 7;

/// Launch the Foxfire TUI with the given agent subprocess command.
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

pub fn client_info() -> Implementation {
    Implementation::new("foxfire", env!("CARGO_PKG_VERSION"))
}

/// Logs to `dir/<file_name>.<date>`, rotating daily and keeping the most recent week of files.
pub fn init_file_logging(dir: &Path, file_name: &str, default_filter: &str) {
    create_dir_all(dir).ok();
    let Ok(appender) = RollingFileAppender::builder()
        .rotation(Rotation::DAILY)
        .filename_prefix(file_name)
        .max_log_files(MAX_LOG_FILES)
        .build(dir)
    else {
        return;
    };
    let _ = tracing_subscriber::fmt()
        .with_writer(appender)
        .with_ansi(false)
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(default_filter)))
        .try_init();
}

/// Run the TUI from an already-initialized ACP session.
async fn run_with_session(session: Session, settings: UiSettings) -> Result<(), AppError> {
    let (session, session_runtime) = session.into_core();
    let app = App::new(AppConfig { session, settings });
    runtime::run(app, Renderer::new(), session_runtime).await
}

fn setup_logging(log_dir: Option<&Path>) {
    init_file_logging(log_dir.unwrap_or(Path::new(DEFAULT_LOG_DIR)), "foxfire.log", "info");
}
