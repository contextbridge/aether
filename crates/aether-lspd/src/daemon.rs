use crate::client_connection::handle_client;
use crate::error::{DaemonError, DaemonResult};
use crate::pid_lockfile::PidLockfile;
use crate::socket_path::lockfile_path;
use crate::workspace_registry::WorkspaceRegistry;
use std::fs::{create_dir_all, remove_file};
use std::future::pending;
use std::os::unix::net::UnixListener as StdUnixListener;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;
use tokio::net::UnixListener;
use tokio::select;
use tokio::spawn;
use tokio::sync::oneshot;
use tokio::time::{Instant, sleep};
use uuid::Uuid;

#[doc = include_str!("docs/daemon.md")]
pub(crate) struct LspDaemon {
    socket_path: PathBuf,
    listener: StdUnixListener,
    _lockfile: PidLockfile,
    idle_timeout: Option<Duration>,
    workspace_registry: WorkspaceRegistry,
}

/// How long the daemon waits on its language servers.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Timeouts {
    /// Bounds each request; a server that exceeds it is killed and replaced.
    pub(crate) request: Duration,
    /// How long a diagnostics query waits for the server to publish fresh results.
    pub(crate) diagnostics: Duration,
    /// How long a file's diagnostics must stay unchanged before they count as final.
    pub(crate) diagnostics_settle: Duration,
}

impl LspDaemon {
    pub(crate) fn bind(socket_path: PathBuf, idle_timeout: Option<Duration>, timeouts: Timeouts) -> DaemonResult<Self> {
        if let Some(parent) = socket_path.parent() {
            create_dir_all(parent).map_err(DaemonError::Io)?;
        }
        let lockfile = PidLockfile::acquire(&lockfile_path(&socket_path)).map_err(DaemonError::LockfileError)?;

        let _ = remove_file(&socket_path);
        let listener = StdUnixListener::bind(&socket_path).map_err(DaemonError::BindFailed)?;
        listener.set_nonblocking(true).map_err(DaemonError::BindFailed)?;
        tracing::info!("Daemon listening on {:?}", socket_path);

        Ok(Self {
            socket_path,
            listener,
            _lockfile: lockfile,
            idle_timeout,
            workspace_registry: WorkspaceRegistry::new(timeouts),
        })
    }

    /// Serve clients until SIGTERM or SIGINT.
    pub(crate) async fn run(self) -> DaemonResult<()> {
        self.run_until_shutdown(spawn_shutdown_signal_handler()).await
    }

    pub(crate) async fn run_until_shutdown(self, shutdown_rx: oneshot::Receiver<()>) -> DaemonResult<()> {
        let listener = UnixListener::from_std(self.listener).map_err(DaemonError::BindFailed)?;
        run_listener_loop(&listener, &self.workspace_registry, self.idle_timeout, shutdown_rx).await;

        tracing::info!("Shutting down LSP servers");
        self.workspace_registry.shutdown().await;

        let _ = remove_file(&self.socket_path);
        tracing::info!("Daemon shutdown complete");

        Ok(())
    }
}

impl Default for Timeouts {
    fn default() -> Self {
        Self {
            request: Duration::from_secs(120),
            diagnostics: Duration::from_secs(20),
            diagnostics_settle: Duration::from_millis(600),
        }
    }
}

struct IdleState {
    client_count: usize,
    last_activity: Instant,
}

async fn run_listener_loop(
    listener: &UnixListener,
    workspace_registry: &WorkspaceRegistry,
    idle_timeout: Option<Duration>,
    mut shutdown_rx: oneshot::Receiver<()>,
) {
    let idle = Arc::new(Mutex::new(IdleState { client_count: 0, last_activity: Instant::now() }));

    loop {
        select! {
            biased;

            _ = &mut shutdown_rx => {
                tracing::info!("Shutting down");
                return;
            }

            result = listener.accept() => {
                match result {
                    Ok((stream, _)) => {
                        let client_id = Uuid::new_v4();
                        let registry = workspace_registry.clone();
                        let idle = Arc::clone(&idle);

                        {
                            let mut state = idle.lock().unwrap_or_else(PoisonError::into_inner);
                            state.client_count += 1;
                            state.last_activity = Instant::now();
                        }

                        spawn(async move {
                            handle_client(stream, registry, client_id).await;
                            {
                                let mut state = idle.lock().unwrap_or_else(PoisonError::into_inner);
                                state.client_count -= 1;
                                state.last_activity = Instant::now();
                            }
                            tracing::debug!("Client {} handler complete", client_id);
                        });
                    }
                    Err(e) => {
                        tracing::warn!("Failed to accept connection: {}", e);
                    }
                }
            }

            () = check_idle_timeout(&idle, idle_timeout) => {
                tracing::info!("Idle timeout reached, shutting down");
                return;
            }

            () = check_workspace_liveness(workspace_registry, Duration::from_secs(10)) => {
                tracing::info!("All workspace roots deleted, shutting down");
                return;
            }
        }
    }
}

async fn check_idle_timeout(idle: &Mutex<IdleState>, timeout: Option<Duration>) {
    let Some(timeout) = timeout else {
        pending::<()>().await;
        return;
    };

    loop {
        sleep(Duration::from_secs(10)).await;

        let state = idle.lock().unwrap_or_else(PoisonError::into_inner);
        if state.client_count == 0 && state.last_activity.elapsed() >= timeout {
            return;
        }
    }
}

/// Returns `true` when all roots are non-existent and the list is non-empty.
fn all_roots_deleted(roots: &[PathBuf]) -> bool {
    !roots.is_empty() && roots.iter().all(|root| !root.exists())
}

/// Resolves when every registered workspace root has been deleted from disk.
async fn check_workspace_liveness(workspace_registry: &WorkspaceRegistry, poll_interval: Duration) {
    loop {
        sleep(poll_interval).await;
        let roots = workspace_registry.workspace_roots();
        if all_roots_deleted(&roots) {
            return;
        }
    }
}

/// Spawn a task to handle shutdown signals (SIGTERM, SIGINT)
fn spawn_shutdown_signal_handler() -> oneshot::Receiver<()> {
    let (tx, rx) = oneshot::channel::<()>();

    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};
        spawn(async move {
            let mut sigterm = signal(SignalKind::terminate()).expect("Failed to register SIGTERM handler");

            let mut sigint = signal(SignalKind::interrupt()).expect("Failed to register SIGINT handler");

            select! {
                _ = sigterm.recv() => {
                    tracing::info!("Received SIGTERM");
                }
                _ = sigint.recv() => {
                    tracing::info!("Received SIGINT");
                }
            }
            let _ = tx.send(());
        });
    }

    rx
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_roots_deleted_empty_returns_false() {
        assert!(!all_roots_deleted(&[]));
    }

    #[test]
    fn all_roots_deleted_existing_dir_returns_false() {
        let dir = tempfile::tempdir().unwrap();
        assert!(!all_roots_deleted(&[dir.path().to_path_buf()]));
    }

    #[test]
    fn all_roots_deleted_nonexistent_returns_true() {
        let gone = PathBuf::from("/tmp/aether-lspd-test-nonexistent-dir-that-does-not-exist");
        assert!(all_roots_deleted(&[gone]));
    }

    #[test]
    fn all_roots_deleted_mixed_returns_false() {
        let dir = tempfile::tempdir().unwrap();
        let gone = PathBuf::from("/tmp/aether-lspd-test-nonexistent-dir-that-does-not-exist");
        assert!(!all_roots_deleted(&[dir.path().to_path_buf(), gone]));
    }

    #[test]
    fn all_roots_deleted_after_tempdir_drop() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().to_path_buf();
        assert!(!all_roots_deleted(std::slice::from_ref(&root)));
        drop(dir);
        assert!(all_roots_deleted(&[root]));
    }
}
