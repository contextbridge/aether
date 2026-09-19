use crate::client_connection::handle_client;
use crate::error::{DaemonError, DaemonResult};
use crate::pid_lockfile::PidLockfile;
use crate::workspace_registry::WorkspaceRegistry;
use std::fs::{create_dir_all, remove_file};
use std::future::pending;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::time::{Duration, Instant};
use tokio::net::UnixListener;
use tokio::select;
use tokio::spawn;
use tokio::sync::oneshot;
use tokio::time::sleep;
use uuid::Uuid;

#[doc = include_str!("docs/daemon.md")]
pub struct LspDaemon {
    socket_path: PathBuf,
    idle_timeout: Option<Duration>,
    workspace_registry: WorkspaceRegistry,
}

impl LspDaemon {
    /// Create a daemon with socket, idle-timeout, and per-request timeout settings.
    pub fn new(socket_path: PathBuf, idle_timeout: Option<Duration>, request_timeout: Duration) -> Self {
        Self { socket_path, idle_timeout, workspace_registry: WorkspaceRegistry::new(request_timeout) }
    }

    /// Run the daemon until shutdown.
    pub async fn run(self) -> DaemonResult<()> {
        self.run_until_shutdown(spawn_shutdown_signal_handler()).await
    }

    pub(crate) async fn run_until_shutdown(self, shutdown_rx: oneshot::Receiver<()>) -> DaemonResult<()> {
        if let Some(parent) = self.socket_path.parent() {
            create_dir_all(parent).map_err(DaemonError::Io)?;
        }

        let _lockfile =
            PidLockfile::acquire(&self.socket_path.with_extension("lock")).map_err(DaemonError::LockfileError)?;

        let _ = remove_file(&self.socket_path);

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
        let idle = Arc::new(IdleTracker::new());

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
                            let idle = Arc::clone(&idle);

                            idle.client_connected();

                            spawn(async move {
                                handle_client(stream, registry, client_id).await;
                                idle.client_disconnected();
                                tracing::debug!("Client {} handler complete", client_id);
                            });
                        }
                        Err(e) => {
                            tracing::warn!("Failed to accept connection: {}", e);
                        }
                    }
                }

                () = check_idle_timeout(Arc::clone(&idle), self.idle_timeout) => {
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

struct IdleTracker {
    started_at: Instant,
    client_count: AtomicUsize,
    last_activity_millis: AtomicU64,
}

impl IdleTracker {
    fn new() -> Self {
        Self { started_at: Instant::now(), client_count: AtomicUsize::new(0), last_activity_millis: AtomicU64::new(0) }
    }

    fn client_connected(&self) {
        self.record_activity();
        self.client_count.fetch_add(1, Ordering::SeqCst);
    }

    fn client_disconnected(&self) {
        self.record_activity();
        self.client_count.fetch_sub(1, Ordering::SeqCst);
    }

    fn is_idle_for(&self, timeout: Duration) -> bool {
        if self.client_count.load(Ordering::SeqCst) > 0 {
            return false;
        }

        let now = elapsed_millis(self.started_at);
        let last = self.last_activity_millis.load(Ordering::SeqCst);
        Duration::from_millis(now.saturating_sub(last)) >= timeout
    }

    fn record_activity(&self) {
        self.last_activity_millis.store(elapsed_millis(self.started_at), Ordering::SeqCst);
    }
}

/// Wait until idle timeout is reached
async fn check_idle_timeout(idle: Arc<IdleTracker>, timeout: Option<Duration>) {
    check_idle_timeout_with_interval(idle, timeout, Duration::from_secs(10)).await;
}

/// Wait until idle timeout is reached, polling at a configurable interval.
async fn check_idle_timeout_with_interval(idle: Arc<IdleTracker>, timeout: Option<Duration>, poll_interval: Duration) {
    let Some(timeout) = timeout else {
        pending::<()>().await;
        return;
    };

    loop {
        sleep(poll_interval).await;

        if idle.is_idle_for(timeout) {
            return;
        }
    }
}

fn elapsed_millis(started_at: Instant) -> u64 {
    u64::try_from(started_at.elapsed().as_millis()).unwrap_or(u64::MAX)
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

    #[tokio::test(start_paused = true)]
    async fn idle_timeout_none_never_completes() {
        let idle = Arc::new(IdleTracker::new());

        let result =
            timeout(Duration::from_millis(40), check_idle_timeout_with_interval(idle, None, Duration::from_millis(5)))
                .await;

        assert!(result.is_err(), "None timeout should not complete");
    }

    #[tokio::test(start_paused = true)]
    async fn idle_timeout_completes_when_idle_elapsed() {
        let started_at = Instant::now()
            .checked_sub(Duration::from_millis(50))
            .expect("subtracting from current instant should succeed");
        let idle = Arc::new(IdleTracker {
            started_at,
            client_count: AtomicUsize::new(0),
            last_activity_millis: AtomicU64::new(0),
        });

        let result = timeout(
            Duration::from_millis(100),
            check_idle_timeout_with_interval(idle, Some(Duration::from_millis(10)), Duration::from_millis(5)),
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
