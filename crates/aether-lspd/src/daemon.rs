use crate::client_connection::handle_client;
use crate::error::{DaemonError, DaemonResult};
use crate::pid_lockfile::PidLockfile;
use crate::workspace_registry::WorkspaceRegistry;
use std::fs::{create_dir_all, remove_file};
use std::future::pending;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};
use tokio::net::UnixListener;
use tokio::select;
use tokio::spawn;
use tokio::sync::{RwLock, oneshot};
use tokio::time::sleep;
use uuid::Uuid;

#[doc = include_str!("docs/daemon.md")]
pub struct LspDaemon {
    socket_path: PathBuf,
    idle_timeout: Option<Duration>,
    workspace_registry: WorkspaceRegistry,
}

impl LspDaemon {
    /// Create a daemon with socket and idle-timeout settings.
    pub fn new(socket_path: PathBuf, idle_timeout: Option<Duration>) -> Self {
        Self { socket_path, idle_timeout, workspace_registry: WorkspaceRegistry::new() }
    }

    /// Run the daemon until shutdown.
    pub async fn run(self) -> DaemonResult<()> {
        if let Some(parent) = self.socket_path.parent() {
            create_dir_all(parent).map_err(DaemonError::Io)?;
        }

        let _lockfile = PidLockfile::acquire(&self.socket_path.with_extension("lock"))
            .map_err(|e| DaemonError::LockfileError(e.to_string()))?;

        let _ = remove_file(&self.socket_path);

        let shutdown_rx = spawn_shutdown_signal_handler();

        tracing::info!("Daemon listening on {:?}", self.socket_path);
        self.run_listener_loop(shutdown_rx).await?;

        tracing::info!("Shutting down LSP servers");
        self.workspace_registry.shutdown().await;

        let _ = remove_file(&self.socket_path);
        tracing::info!("Daemon shutdown complete");

        Ok(())
    }

    /// Main listener loop that handles connections and shutdown signals.
    async fn run_listener_loop(&self, mut shutdown_rx: oneshot::Receiver<()>) -> DaemonResult<()> {
        let listener = UnixListener::bind(&self.socket_path).map_err(DaemonError::BindFailed)?;
        let client_count = Arc::new(AtomicUsize::new(0));
        let last_activity = Arc::new(RwLock::new(Instant::now()));

        loop {
            select! {
                biased;

                _ = &mut shutdown_rx => {
                    tracing::info!("Shutting down");
                    return Ok(());
                }

                result = listener.accept() => {
                    match result {
                        Ok((stream, _)) => {
                            let client_id = Uuid::new_v4();
                            let registry = self.workspace_registry.clone();
                            let client_count = Arc::clone(&client_count);
                            let last_activity = Arc::clone(&last_activity);

                            client_count.fetch_add(1, Ordering::Relaxed);
                            *last_activity.write().await = Instant::now();

                            spawn(async move {
                                handle_client(stream, registry, client_id).await;
                                client_count.fetch_sub(1, Ordering::Relaxed);
                                *last_activity.write().await = Instant::now();
                                tracing::debug!("Client {} handler complete", client_id);
                            });
                        }
                        Err(e) => {
                            tracing::warn!("Failed to accept connection: {}", e);
                        }
                    }
                }

                () = check_idle_timeout(client_count.clone(), last_activity.clone(), self.idle_timeout) => {
                    tracing::info!("Idle timeout reached, shutting down");
                    return Ok(());
                }

                () = check_workspace_liveness(&self.workspace_registry, Duration::from_secs(10)) => {
                    tracing::info!("All workspace roots deleted, shutting down");
                    return Ok(());
                }
            }
        }
    }
}

/// Compatibility wrapper for the previous daemon entrypoint.
pub async fn run_daemon(socket_path: PathBuf, idle_timeout: Option<Duration>) -> DaemonResult<()> {
    LspDaemon::new(socket_path, idle_timeout).run().await
}

/// Wait until idle timeout is reached
async fn check_idle_timeout(
    client_count: Arc<AtomicUsize>,
    last_activity: Arc<RwLock<Instant>>,
    timeout: Option<Duration>,
) {
    check_idle_timeout_with_interval(client_count, last_activity, timeout, Duration::from_secs(10)).await;
}

/// Wait until idle timeout is reached, polling at a configurable interval.
async fn check_idle_timeout_with_interval(
    client_count: Arc<AtomicUsize>,
    last_activity: Arc<RwLock<Instant>>,
    timeout: Option<Duration>,
    poll_interval: Duration,
) {
    let Some(timeout) = timeout else {
        pending::<()>().await;
        return;
    };

    loop {
        sleep(poll_interval).await;

        let count = client_count.load(Ordering::Relaxed);
        if count > 0 {
            continue;
        }

        let last = *last_activity.read().await;
        if last.elapsed() >= timeout {
            return;
        }
    }
}

/// Returns `true` when all roots are non-existent and the list is non-empty.
fn all_roots_deleted(roots: &[PathBuf]) -> bool {
    !roots.is_empty() && roots.iter().all(|root| !root.exists())
}

/// Resolves when every workspace root managed by `lsp_manager` has been deleted
/// from disk. Polls at `poll_interval`.
async fn check_workspace_liveness(workspace_registry: &WorkspaceRegistry, poll_interval: Duration) {
    loop {
        sleep(poll_interval).await;
        let roots = workspace_registry.workspace_roots().await;
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
    use tokio::time::timeout;

    #[tokio::test]
    async fn idle_timeout_none_never_completes() {
        let client_count = Arc::new(AtomicUsize::new(0));
        let last_activity = Arc::new(RwLock::new(Instant::now()));

        let result = timeout(
            Duration::from_millis(40),
            check_idle_timeout_with_interval(client_count, last_activity, None, Duration::from_millis(5)),
        )
        .await;

        assert!(result.is_err(), "None timeout should not complete");
    }

    #[tokio::test]
    async fn idle_timeout_completes_when_idle_elapsed() {
        let client_count = Arc::new(AtomicUsize::new(0));
        let stale_activity = Instant::now()
            .checked_sub(Duration::from_millis(50))
            .expect("subtracting from current instant should succeed");
        let last_activity = Arc::new(RwLock::new(stale_activity));

        let result = timeout(
            Duration::from_millis(100),
            check_idle_timeout_with_interval(
                client_count,
                last_activity,
                Some(Duration::from_millis(10)),
                Duration::from_millis(5),
            ),
        )
        .await;

        assert!(result.is_ok(), "Idle timeout should complete");
    }

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
