use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::thread::JoinHandle;
use std::time::Duration;
use tempfile::TempDir;
use tokio::net::UnixStream;
use tokio::sync::oneshot;

use crate::LanguageId;
use crate::daemon::LspDaemon;
use crate::error::DaemonError;
use crate::language_catalog::server_kind_for_language;
use crate::socket_path::socket_path;
use crate::uri::path_to_uri;

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
        let socket_path = socket_path(workspace_root, language);
        let _ = fs::remove_file(&socket_path);
        let _ = fs::remove_file(socket_path.with_extension("lock"));
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

/// Configure the Rust fake server to return an error for workspace symbols.
///
/// This is useful for exercising clients that must fall back to document-symbol
/// queries when the workspace-symbol request is unavailable.
pub fn use_fake_rust_server_failing_workspace_symbol() {
    unsafe { configure_fake_server(LanguageId::Rust, &["--fail-on", "workspace/symbol"]) }
}

#[doc = include_str!("docs/testing.md")]
pub trait TestProject {
    fn root(&self) -> &Path;

    fn add_file(&self, relative_path: &str, content: &str) -> Result<PathBuf, TestProjectError> {
        let path = self.root().join(relative_path);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(&path, content)?;
        Ok(path)
    }

    fn file_uri(&self, relative_path: &str) -> lsp_types::Uri {
        path_to_uri(&self.root().join(relative_path)).expect("Invalid file path")
    }

    fn file_path_str(&self, relative_path: &str) -> String {
        self.root().join(relative_path).to_str().expect("Non-UTF8 path").to_string()
    }
}

/// Error type for test project operations.
#[derive(Debug, thiserror::Error)]
pub enum TestProjectError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Command '{command}' failed:\n{stderr}")]
    CommandFailed { command: String, stderr: String },
}

/// A temporary Cargo project for testing.
pub struct CargoProject {
    temp_dir: TempDir,
}

impl TestProject for CargoProject {
    fn root(&self) -> &Path {
        self.temp_dir.path()
    }
}

impl CargoProject {
    /// Create a new minimal Cargo project.
    pub fn new(name: &str) -> Result<Self, TestProjectError> {
        let temp_dir = TempDir::new()?;
        let project = Self { temp_dir };
        project.init_cargo_toml(name)?;
        project.init_src_dir()?;
        Ok(project)
    }

    fn init_cargo_toml(&self, name: &str) -> Result<(), TestProjectError> {
        let content = format!(
            r#"[package]
name = "{name}"
version = "0.1.0"
edition = "2021"
"#
        );
        fs::write(self.root().join("Cargo.toml"), content)?;
        Ok(())
    }

    fn init_src_dir(&self) -> Result<(), TestProjectError> {
        let src_dir = self.root().join("src");
        fs::create_dir_all(&src_dir)?;

        let main_content = r#"fn main() {
    println!("Hello, world!");
}
"#;
        fs::write(src_dir.join("main.rs"), main_content)?;
        Ok(())
    }
}

const TYPESCRIPT_PACKAGE: &str = "typescript@6.0.3";
const TYPESCRIPT_LANGUAGE_SERVER_PACKAGE: &str = "typescript-language-server@5.2.0";

/// A temporary Node.js/TypeScript project for testing.
pub struct NodeProject {
    temp_dir: TempDir,
}

impl TestProject for NodeProject {
    fn root(&self) -> &Path {
        self.temp_dir.path()
    }
}

impl NodeProject {
    /// Create a new minimal Node/TypeScript project.
    ///
    /// Installs pinned TypeScript tooling locally so tests do not depend on global Node tools.
    pub fn new(name: &str) -> Result<Self, TestProjectError> {
        let temp_dir = TempDir::new()?;
        let project = Self { temp_dir };
        project.init_package_json(name)?;
        project.init_tsconfig()?;
        project.init_src_dir()?;
        project.install_typescript()?;
        Ok(project)
    }

    fn init_package_json(&self, name: &str) -> Result<(), TestProjectError> {
        let content = format!(
            r#"{{
  "name": "{name}",
  "version": "0.1.0"
}}"#
        );
        fs::write(self.root().join("package.json"), content)?;
        Ok(())
    }

    fn init_tsconfig(&self) -> Result<(), TestProjectError> {
        let content = r#"{
  "compilerOptions": {
    "strict": true,
    "noEmit": true
  }
}"#;
        fs::write(self.root().join("tsconfig.json"), content)?;
        Ok(())
    }

    fn init_src_dir(&self) -> Result<(), TestProjectError> {
        let src_dir = self.root().join("src");
        fs::create_dir_all(&src_dir)?;
        fs::write(src_dir.join("index.ts"), "")?;
        Ok(())
    }

    fn install_typescript(&self) -> Result<(), TestProjectError> {
        let args = [
            "install",
            "--save-dev",
            "--no-audit",
            "--no-fund",
            "--prefer-offline",
            TYPESCRIPT_PACKAGE,
            TYPESCRIPT_LANGUAGE_SERVER_PACKAGE,
        ];
        let output = Command::new("npm").args(args).current_dir(self.root()).output()?;

        if !output.status.success() {
            return Err(TestProjectError::CommandFailed {
                command: format!("npm {}", args.join(" ")),

                stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
            });
        }
        Ok(())
    }
}
