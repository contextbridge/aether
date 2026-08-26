#![cfg(feature = "testing")]

use acp_utils::client::AcpClientHandle;
use agent_client_protocol::schema::v1::SessionId;
use tempfile::TempDir;
use wisp::command::{AgentCommand, Command, CommandResult, FailedCommand, GitCommand};
use wisp::file_index::index_files_with_limit;
use wisp::git_review::{DiffScope, GitDiffError, GitDiffEvent};
use wisp::request::RequestId;
use wisp::runtime::CommandDispatcher;

#[test]
fn file_index_limit_counts_only_indexed_files() {
    let root = TempDir::new().unwrap();
    std::fs::write(root.path().join("file.rs"), "fn main() {}\n").unwrap();

    let files = index_files_with_limit(root.path(), 1);

    assert_eq!(files.len(), 1);
    assert_eq!(files[0].display_name, "file.rs");
}

#[tokio::test]
async fn closed_agent_connection_becomes_a_reducer_visible_failure() {
    let mut dispatcher = CommandDispatcher::new(AcpClientHandle::detached());

    let result = dispatcher.dispatch(Command::Agent(AgentCommand::Cancel { session_id: SessionId::new("session") }));

    assert!(result.is_none());
    assert!(dispatcher.has_pending_tasks());
    assert!(matches!(
        dispatcher.next_result().await,
        Some(CommandResult::Failed { command: FailedCommand::Other("cancel"), .. })
    ));
    dispatcher.shutdown().await;
}

#[tokio::test]
async fn supervised_git_reads_report_completion() {
    let mut dispatcher = CommandDispatcher::new(AcpClientHandle::detached());
    let outside_repository = TempDir::new().unwrap();
    let request_id = RequestId::from(7);

    assert!(
        dispatcher
            .dispatch(Command::Git(GitCommand::Load {
                request_id,
                working_dir: outside_repository.path().to_path_buf(),
                repo_root: None,
                scope: DiffScope::Both,
            }))
            .is_none()
    );
    assert!(dispatcher.has_pending_tasks());

    assert!(matches!(
        dispatcher.next_result().await,
        Some(CommandResult::GitDiff(GitDiffEvent::Loaded {
            request_id: actual,
            result: Err(GitDiffError::NotARepository),
        })) if actual == request_id
    ));
    assert!(!dispatcher.has_pending_tasks());
}

#[tokio::test]
async fn superseded_workspace_reads_are_cancelled() {
    let mut dispatcher = CommandDispatcher::new(AcpClientHandle::detached());
    let first = TempDir::new().unwrap();
    let second = TempDir::new().unwrap();
    let second_path = second.path().to_path_buf();

    dispatcher.dispatch(Command::ResolveWorkspace { cwd: first.path().to_path_buf() });
    dispatcher.dispatch(Command::ResolveWorkspace { cwd: second_path.clone() });

    assert!(matches!(
        dispatcher.next_result().await,
        Some(CommandResult::WorkspaceResolved { cwd, .. }) if cwd == second_path
    ));
    assert!(!dispatcher.has_pending_tasks());
}
