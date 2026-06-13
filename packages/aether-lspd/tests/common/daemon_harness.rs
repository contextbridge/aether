use aether_lspd::{ClientError, LanguageId, LspClient, lockfile_path, socket_path};
use std::fs;
use std::path::{Path, PathBuf};

/// Error type for daemon harness operations
#[derive(Debug, thiserror::Error)]
pub enum HarnessError {
    #[error("Failed to spawn daemon: {0}")]
    SpawnFailed(String),
    #[error("Client error: {0}")]
    ClientError(#[from] ClientError),
    #[error("Failed to kill daemon: {0}")]
    KillFailed(String),
}

/// Test harness for the LSP daemon
pub struct DaemonHarness {
    socket_path: PathBuf,
    lockfile_path: PathBuf,
    workspace_root: PathBuf,
    language: LanguageId,
}

impl DaemonHarness {
    /// Spawn a daemon for testing using `connect`
    pub async fn spawn(workspace_root: &Path, language: LanguageId) -> Result<Self, HarnessError> {
        let sock_path = socket_path(workspace_root, language);
        let lock_path = lockfile_path(&sock_path);

        let _ = fs::remove_file(&sock_path);
        let _ = fs::remove_file(&lock_path);

        let _client =
            LspClient::connect(workspace_root, language).await.map_err(|e| HarnessError::SpawnFailed(e.to_string()))?;

        Ok(Self {
            socket_path: sock_path,
            lockfile_path: lock_path,
            workspace_root: workspace_root.to_path_buf(),
            language,
        })
    }

    /// Connect a client to the running daemon
    pub async fn connect(&self) -> Result<LspClient, HarnessError> {
        LspClient::connect(&self.workspace_root, self.language).await.map_err(HarnessError::ClientError)
    }

    /// Kill the daemon by reading PID from lockfile and sending SIGTERM
    pub async fn kill(&self) -> Result<(), HarnessError> {
        let pid_str = fs::read_to_string(&self.lockfile_path)
            .map_err(|e| HarnessError::KillFailed(format!("Failed to read lockfile: {e}")))?;

        let pid: i32 =
            pid_str.trim().parse().map_err(|e| HarnessError::KillFailed(format!("Failed to parse PID: {e}")))?;

        #[cfg(unix)]
        {
            let result = unsafe { libc::kill(pid, libc::SIGTERM) };
            if result != 0 {
                let err = std::io::Error::last_os_error();

                if err.raw_os_error() != Some(libc::ESRCH) {
                    return Err(HarnessError::KillFailed(format!("kill({pid}, SIGTERM) failed: {err}")));
                }
            }
        }

        for _ in 0..100 {
            #[cfg(unix)]
            {
                if unsafe { libc::kill(pid, 0) } != 0 {
                    break;
                }
            }
            tokio::task::yield_now().await;
        }

        let _ = fs::remove_file(&self.socket_path);

        Ok(())
    }
}

impl Drop for DaemonHarness {
    fn drop(&mut self) {
        if let Ok(pid_str) = fs::read_to_string(&self.lockfile_path)
            && let Ok(pid) = pid_str.trim().parse::<i32>()
        {
            #[cfg(unix)]
            unsafe {
                libc::kill(pid, libc::SIGTERM);
            }
        }
        let _ = fs::remove_file(&self.socket_path);
        let _ = fs::remove_file(&self.lockfile_path);
    }
}
