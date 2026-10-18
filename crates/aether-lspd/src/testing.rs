#![doc = include_str!("docs/testing.md")]

use std::future::{Future, poll_fn};
use std::path::{Path, PathBuf};
use std::pin::pin;
use std::process::Command;
use std::sync::{Arc, mpsc};
use std::thread::JoinHandle;
use std::time::Duration;
use tokio::sync::{oneshot, watch};

use crate::LanguageId;
use crate::daemon::{LspDaemon, Timeouts};
use crate::error::DaemonError;
use crate::socket_path::socket_path;
use crate::uri::path_to_uri;
pub use utils::testing::TestWorkspace;

/// An in-process daemon with explicit configuration and deterministic shutdown.
pub struct TestDaemon {
    shutdown_tx: Option<oneshot::Sender<()>>,
    task: Option<JoinHandle<Result<(), DaemonError>>>,
    clock: Option<tokio::runtime::Handle>,
}

/// Configures a [`TestDaemon`] before it starts. Unset timeouts match the production daemon.
pub struct TestDaemonBuilder {
    workspace_root: PathBuf,
    language: LanguageId,
    timeouts: Timeouts,
    hooks: TestHooks,
    paused_time: bool,
}

impl TestDaemon {
    pub fn spawn(workspace_root: &Path, language: LanguageId) -> Result<Self, TestDaemonError> {
        Self::builder(workspace_root, language).spawn()
    }

    pub fn builder(workspace_root: &Path, language: LanguageId) -> TestDaemonBuilder {
        TestDaemonBuilder {
            workspace_root: workspace_root.to_path_buf(),
            language,
            timeouts: Timeouts::default(),
            hooks: TestHooks::default(),
            paused_time: false,
        }
    }

    pub async fn advance_time(&self, duration: Duration) {
        self.clock
            .as_ref()
            .expect("enable paused_time on the daemon builder")
            .spawn(async move { tokio::time::advance(duration).await })
            .await
            .expect("daemon clock task should complete");
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

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TestPoint {
    ReadyWait,
    RequestReceived(String),
    DocumentOpen(lsp_types::Uri),
    DocumentSyncWait(lsp_types::Uri),
    DiagnosticsWait(lsp_types::Uri),
}

#[derive(Clone, Default)]
pub struct TestGate {
    state: Arc<watch::Sender<(bool, bool)>>,
}

impl TestGate {
    pub async fn reached(&self) {
        let _ = self.state.subscribe().wait_for(|(reached, _)| *reached).await;
    }

    pub fn release(&self) {
        self.state.send_modify(|(_, released)| *released = true);
    }

    fn reach(&self) {
        self.state.send_modify(|(reached, _)| *reached = true);
    }

    async fn pause(&self) {
        self.reach();
        let _ = self.state.subscribe().wait_for(|(_, released)| *released).await;
    }
}

impl TestDaemonBuilder {
    pub fn gate(mut self, point: TestPoint, gate: &TestGate) -> Self {
        self.hooks.0.push((point, gate.clone()));
        self
    }

    pub fn paused_time(mut self) -> Self {
        self.paused_time = true;
        self
    }

    pub fn request_timeout(mut self, timeout: Duration) -> Self {
        self.timeouts.request = timeout;
        self
    }

    pub fn diagnostics_timeout(mut self, timeout: Duration) -> Self {
        self.timeouts.diagnostics = timeout;
        self
    }

    pub fn diagnostics_settle(mut self, settle: Duration) -> Self {
        self.timeouts.diagnostics_settle = settle;
        self
    }

    pub fn spawn(self) -> Result<TestDaemon, TestDaemonError> {
        let socket_path = socket_path(&self.workspace_root, self.language);
        let daemon = LspDaemon::bind(socket_path, None, self.timeouts)?.with_test_hooks(self.hooks);
        let runtime = if self.paused_time {
            tokio::runtime::Builder::new_current_thread().enable_all().start_paused(true).build()
        } else {
            tokio::runtime::Builder::new_multi_thread().enable_all().build()
        }
        .map_err(TestDaemonError::Runtime)?;
        let clock = self.paused_time.then(|| runtime.handle().clone());
        let (shutdown_tx, shutdown_rx) = oneshot::channel();
        let task = std::thread::spawn(move || {
            // A pending blocking task disables Tokio's automatic clock advancement during external I/O.
            let (release_tx, release_rx) = mpsc::channel::<()>();
            let hold_clock = self.paused_time.then(|| runtime.spawn_blocking(move || release_rx.recv()));
            let result = runtime.block_on(daemon.run_until_shutdown(shutdown_rx));
            drop(release_tx);
            if let Some(hold_clock) = hold_clock {
                runtime.block_on(hold_clock).expect("clock guard should finish").ok();
            }
            result
        });
        Ok(TestDaemon { shutdown_tx: Some(shutdown_tx), task: Some(task), clock })
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
    let env_key = language.server().expect("language should have a configured server").env_key;
    let script = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/common/fake_lsp_server.py");
    let mut args = vec![script.to_string_lossy().into_owned()];
    args.extend(extra_args.iter().map(ToString::to_string));

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

#[derive(Clone, Default)]
pub(crate) struct TestHooks(Vec<(TestPoint, TestGate)>);

impl TestHooks {
    pub(crate) async fn pause(&self, point: TestPoint) {
        if let Some(gate) = self.gate(&point) {
            gate.pause().await;
        }
    }

    pub(crate) async fn observe_pending<T>(&self, point: TestPoint, future: impl Future<Output = T>) -> T {
        let mut future = pin!(future);
        poll_fn(|cx| {
            let result = future.as_mut().poll(cx);
            if result.is_pending()
                && let Some(gate) = self.gate(&point)
            {
                gate.reach();
            }
            result
        })
        .await
    }

    fn gate(&self, point: &TestPoint) -> Option<&TestGate> {
        self.0.iter().find_map(|(at, gate)| (at == point).then_some(gate))
    }
}

const TYPESCRIPT_PACKAGE: &str = "typescript@7.0.2";
