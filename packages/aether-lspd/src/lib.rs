#![doc = include_str!("../README.md")]

#[cfg(feature = "testing")]
pub mod testing;

mod client_connection;
mod diagnostics_store;
mod document_lifecycle;
mod file_watcher;
pub mod language_catalog;
mod pid_lockfile;
mod process_transport;
mod refresh_queue;
mod workspace_registry;
mod workspace_session;

mod client;
pub mod daemon;
pub mod error;
pub mod lsp_utils;
pub mod protocol;
pub mod socket_path;
pub mod uri;

use std::path::PathBuf;
use std::time::Duration;
use tracing_subscriber::EnvFilter;
use tracing_subscriber::fmt::writer::MakeWriterExt;

pub use client::{ClientError, ClientResult, LspClient};
pub use daemon::{LspDaemon, run_daemon};
pub use error::{DaemonError, DaemonResult};
pub use language_catalog::LanguageId;
pub use language_catalog::{
    LANGUAGE_METADATA, LanguageMetadata, LspConfig, extensions_for_alias, from_lsp_id, get_config_for_language,
    metadata_for,
};
pub use lsp_utils::symbol_kind_to_string;
pub use socket_path::{ensure_socket_dir, lockfile_path, log_file_path, socket_path};

pub use protocol::{
    DaemonRequest, DaemonResponse, InitializeRequest, LspErrorResponse, MAX_MESSAGE_SIZE, ProtocolError,
};
pub use uri::{path_to_uri, uri_to_path};

#[derive(clap::Args)]
pub struct LspdArgs {
    /// Socket path to listen on
    #[arg(long)]
    pub socket: PathBuf,

    /// Idle timeout in seconds (0 = no timeout)
    #[arg(long, default_value = "300")]
    pub idle_timeout: u64,

    /// Log level (trace, debug, info, warn, error)
    #[arg(long, default_value = "info")]
    pub log_level: String,

    /// Log file path. Required for daemon mode since stderr is redirected to /dev/null.
    #[arg(long)]
    pub log_file: Option<PathBuf>,
}

pub fn run_lspd(args: LspdArgs) -> Result<(), String> {
    let idle_timeout = if args.idle_timeout == 0 { None } else { Some(Duration::from_secs(args.idle_timeout)) };

    let runtime = tokio::runtime::Runtime::new().map_err(|e| format!("Failed to create tokio runtime: {e}"))?;

    runtime.block_on(async {
        let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(&args.log_level));

        if let Some(ref log_file) = args.log_file {
            if let Some(parent) = log_file.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            let file = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(log_file)
                .map_err(|e| format!("Failed to open log file: {e}"))?;

            tracing_subscriber::fmt()
                .with_env_filter(filter)
                .with_target(true)
                .with_ansi(false)
                .with_writer(file.with_max_level(tracing::Level::TRACE))
                .init();
        } else {
            tracing_subscriber::fmt().with_env_filter(filter).with_target(true).init();
        }

        tracing::info!("Starting LSP daemon on socket: {:?}", args.socket);
        run_daemon(args.socket, idle_timeout).await.map_err(|e| format!("Daemon error: {e}"))
    })
}
