#![doc = include_str!("docs/testing.md")]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::thread::JoinHandle;
use std::time::Duration;
use tokio::net::UnixStream;
use tokio::sync::oneshot;

use crate::LanguageId;
use crate::daemon::LspDaemon;
use crate::error::DaemonError;
use crate::language_catalog::server_kind_for_language;
use crate::socket_path::{lockfile_path, socket_path};
use crate::uri::path_to_uri;
pub use utils::testing::TestWorkspace;

/// An in-process daemon with explicit configuration and deterministic shutdown.
pub struct TestDaemon {
    shutdown_tx: Option<oneshot::Sender<()>>,
    task: Option<JoinHandle<Result<(), TestDaemonError>>>,
}

impl TestDaemon {
    pub async fn spawn(
        workspace_root: &Path,
        language: LanguageId,
        request_timeout: Duration,
    ) -> Result<Self, TestDaemonError> {
        remove_stale_daemon_files(workspace_root, language);
        let socket_path = socket_path(workspace_root, language);
        let (shutdown_tx, shutdown_rx) = oneshot::channel();
        let daemon_socket_path = socket_path.clone();
        let task = std::thread::spawn(move || {
            tokio::runtime::Runtime::new()
                .map_err(TestDaemonError::Runtime)?
                .block_on(LspDaemon::new(daemon_socket_path, None, request_timeout).run_until_shutdown(shutdown_rx))
                .map_err(TestDaemonError::Daemon)
        });
        let mut daemon = Self { shutdown_tx: Some(shutdown_tx), task: Some(task) };

        while UnixStream::connect(&socket_path).await.is_err() {
            if daemon.task.as_ref().is_some_and(JoinHandle::is_finished) {
                daemon.finish()?;
                return Err(TestDaemonError::ExitedBeforeReady);
            }
            tokio::task::yield_now().await;
        }

        Ok(daemon)
    }

    pub fn shutdown(mut self) -> Result<(), TestDaemonError> {
        if let Some(shutdown_tx) = self.shutdown_tx.take() {
            let _ = shutdown_tx.send(());
        }
        self.finish()
    }

    fn finish(&mut self) -> Result<(), TestDaemonError> {
        let task = self.task.take().expect("test daemon task should be present");
        task.join().map_err(|_| TestDaemonError::ThreadPanicked)??;
        Ok(())
    }
}

impl Drop for TestDaemon {
    fn drop(&mut self) {
        if let Some(shutdown_tx) = self.shutdown_tx.take() {
            let _ = shutdown_tx.send(());
        }
        if let Some(task) = self.task.take() {
            let _ = task.join();
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum TestDaemonError {
    #[error("Daemon failed: {0}")]
    Daemon(#[from] DaemonError),
    #[error("Daemon exited before its socket was ready")]
    ExitedBeforeReady,
    #[error("Failed to create daemon runtime: {0}")]
    Runtime(std::io::Error),
    #[error("Daemon thread panicked")]
    ThreadPanicked,
}

/// Configure one language to use the shared fake Python server in the current test process.
///
/// # Safety
///
/// This mutates process-global environment variables. Call it only from an isolated test binary
/// before starting any daemon or other thread that can read language-server configuration.
pub unsafe fn configure_fake_server(language: LanguageId, extra_args: &[&str]) {
    let server_kind = server_kind_for_language(language).expect("language should have a configured server");
    let script = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/common/fake_lsp_server.py");
    let mut args = vec![script.to_string_lossy().into_owned()];
    args.extend(extra_args.iter().map(ToString::to_string));
    let env_key = server_kind.env_key();

    unsafe {
        std::env::set_var(format!("AETHER_LSPD_SERVER_COMMAND_{env_key}"), "python3");
        std::env::set_var(
            format!("AETHER_LSPD_SERVER_ARGS_{env_key}"),
            serde_json::to_string(&args).expect("fake server arguments should serialize"),
        );
    }
}

pub fn cargo_project(name: &str) -> TestWorkspace {
    let manifest = format!(
        r#"[package]
name = "{name}"
version = "0.1.0"
edition = "2021"
"#
    );
    let main = r#"fn main() {
    println!("Hello, world!");
}
"#;
    TestWorkspace::new().file("Cargo.toml", manifest).file("src/main.rs", main)
}

pub fn node_project(name: &str) -> TestWorkspace {
    let package = format!(
        r#"{{
  "name": "{name}",
  "version": "0.1.0"
}}"#
    );
    let tsconfig = r#"{
  "compilerOptions": {
    "strict": true,
    "noEmit": true
  }
}"#;
    let project =
        TestWorkspace::new().file("package.json", package).file("tsconfig.json", tsconfig).file("src/index.ts", "");

    let args = ["install", "--save-dev", "--no-audit", "--no-fund", "--prefer-offline", TYPESCRIPT_PACKAGE];
    let output = Command::new("npm").args(args).current_dir(project.root()).output().expect("failed to run npm");
    assert!(output.status.success(), "npm {} failed:\n{}", args.join(" "), String::from_utf8_lossy(&output.stderr));
    project
}

pub fn file_uri(path: &Path) -> lsp_types::Uri {
    path_to_uri(path).expect("Invalid file path")
}

pub fn remove_stale_daemon_files(workspace_root: &Path, language: LanguageId) {
    let socket = socket_path(workspace_root, language);
    let _ = fs::remove_file(lockfile_path(&socket));
    let _ = fs::remove_file(socket);
}

const TYPESCRIPT_PACKAGE: &str = "typescript@7.0.2";
