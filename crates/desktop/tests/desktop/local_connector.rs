use aether_desktop::target::{AgentCommandLine, ConnectError, ConnectMode, Connector, LocalConnector, SessionTarget};
use aether_desktop::testing::FAKE_AGENT_LOCK_FILE;
use std::fs::{File, TryLockError};
use std::path::Path;
use tokio::task::spawn_blocking;

#[tokio::test]
async fn a_local_agent_runs_in_the_threads_directory_and_dies_with_its_connection() {
    let cwd = tempfile::tempdir().unwrap();

    let session = LocalConnector.connect(&fake_agent_in(cwd.path()), ConnectMode::NewSession).await.unwrap();

    assert_eq!(session.response.session_id.0.as_ref(), "local-session");
    let lock = File::open(cwd.path().join(FAKE_AGENT_LOCK_FILE)).expect("the agent started in the thread's directory");
    assert!(matches!(lock.try_lock(), Err(TryLockError::WouldBlock)), "the agent is running");

    drop(session);
    // Blocks until the agent's process is gone and its lock with it.
    spawn_blocking(move || lock.lock()).await.unwrap().unwrap();
}

#[tokio::test]
async fn an_agent_command_that_does_not_exist_fails_to_start() {
    let cwd = tempfile::tempdir().unwrap();
    let target =
        SessionTarget::Local { cwd: cwd.path().into(), agent: AgentCommandLine::new("no-such-agent-binary acp") };

    let error = LocalConnector.connect(&target, ConnectMode::NewSession).await.err().expect("connecting fails");

    assert!(matches!(error, ConnectError::Spawn { .. }), "{error}");
}

fn fake_agent_in(cwd: &Path) -> SessionTarget {
    SessionTarget::Local { cwd: cwd.into(), agent: AgentCommandLine::new(env!("CARGO_BIN_EXE_fake-acp-agent")) }
}
