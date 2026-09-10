#![cfg(feature = "testing")]

use acp_utils::client::{AcpClientError, AcpClientHandle, connect_acp_client};
use acp_utils::testing::duplex_pair;
use agent_client_protocol::schema::ProtocolVersion;
use agent_client_protocol::schema::v1::{InitializeRequest, InitializeResponse, SessionId};
use agent_client_protocol::{self as acp, Agent};
use tempfile::TempDir;
use tokio::task::{JoinError, LocalSet, spawn_local};
use wisp::command::{AgentCommand, Command, CommandResult, FailedCommand, GitCommand};
use wisp::file_index::index_files_with_limit;
use wisp::git_review::{DiffScope, GitDiffError, GitDiffEvent};
use wisp::request::RequestId;
use wisp::runtime::CommandDispatcher;

#[path = "support/git_repo.rs"]
mod git_repo;
use git_repo::Repo;

#[tokio::test]
async fn git_mutations_finish_in_dispatch_order_and_shutdown_drains_them() -> Result<(), TestError> {
    use clankerdiff_core::RepositoryAction;
    let repo = Repo::init();
    repo.write("file.txt", "new contents\n");
    let mut dispatcher = CommandDispatcher::new(disconnected_client().await?);
    for action in [
        RepositoryAction::StageAll,
        RepositoryAction::UnstageAll,
        RepositoryAction::StageAll,
        RepositoryAction::Commit { message: "ordered commit".into() },
    ] {
        dispatcher.dispatch(Command::Git(GitCommand::Apply {
            request_id: RequestId::next(),
            repo_root: repo.root.clone(),
            action,
        }));
    }
    dispatcher.shutdown().await;
    assert!(!dispatcher.has_pending_tasks());
    assert_eq!(repo.git(&["show", "HEAD:file.txt"]), b"new contents\n");
    assert!(repo.git(&["status", "--porcelain"]).is_empty());
    assert!(repo.load(DiffScope::Both).await.files.is_empty());
    Ok(())
}

#[test]
fn file_index_limit_counts_only_indexed_files() {
    let root = TempDir::new().unwrap();
    std::fs::write(root.path().join("file.rs"), "fn main() {}\n").unwrap();

    let files = index_files_with_limit(root.path(), 1);

    assert_eq!(files.len(), 1);
    assert_eq!(files[0].display_name, "file.rs");
}

#[tokio::test]
async fn closed_agent_connection_becomes_a_reducer_visible_failure() -> Result<(), TestError> {
    let mut dispatcher = CommandDispatcher::new(disconnected_client().await?);

    let result = dispatcher.dispatch(Command::Agent(AgentCommand::Cancel { session_id: SessionId::new("session") }));

    assert!(result.is_none());
    assert!(dispatcher.has_pending_tasks());
    assert!(matches!(
        dispatcher.next_result().await,
        Some(CommandResult::Failed { command: FailedCommand::Other("cancel"), .. })
    ));
    dispatcher.shutdown().await;
    Ok(())
}

#[tokio::test]
async fn supervised_git_reads_report_completion() -> Result<(), TestError> {
    let mut dispatcher = CommandDispatcher::new(disconnected_client().await?);
    let outside_repository = TempDir::new()?;
    let request_id = RequestId::from(7);

    assert!(
        dispatcher
            .dispatch(Command::Git(GitCommand::Load {
                request_id,
                working_dir: outside_repository.path().to_path_buf(),
                scope: DiffScope::Both,
            }))
            .is_none()
    );
    assert!(dispatcher.has_pending_tasks());

    assert!(matches!(
        dispatcher.next_result().await,
        Some(CommandResult::GitDiff(GitDiffEvent::Loaded {
            request_id: actual,
            result: Err(GitDiffError::NotRepository),
        })) if actual == request_id
    ));
    assert!(!dispatcher.has_pending_tasks());
    Ok(())
}

#[tokio::test]
async fn theme_load_failure_is_returned_as_a_typed_result() -> Result<(), TestError> {
    use wisp::command::FilesystemCommand;
    use wisp::settings::{ThemeSettings, UiSettings};
    use wisp::theme::{ThemeApplicationError, ThemeLoadError};

    let mut dispatcher = CommandDispatcher::new(disconnected_client().await?);
    dispatcher.dispatch(Command::Filesystem(FilesystemCommand::ApplyTheme {
        settings: Box::new(UiSettings {
            theme: ThemeSettings::File { file: "../invalid.json".into() },
            ..UiSettings::default()
        }),
    }));
    assert!(matches!(dispatcher.next_result().await,
        Some(CommandResult::ThemeApplied(Err(ThemeApplicationError::Load(ThemeLoadError::InvalidFile(file)))))
        if file == "../invalid.json"
    ));
    assert!(!dispatcher.has_pending_tasks());
    Ok(())
}

#[tokio::test]
async fn superseded_workspace_reads_are_cancelled() -> Result<(), TestError> {
    let mut dispatcher = CommandDispatcher::new(disconnected_client().await?);
    let first = TempDir::new()?;
    let second = TempDir::new()?;
    let second_path = second.path().to_path_buf();

    dispatcher.dispatch(Command::ResolveWorkspace { cwd: first.path().to_path_buf() });
    dispatcher.dispatch(Command::ResolveWorkspace { cwd: second_path.clone() });

    assert!(matches!(
        dispatcher.next_result().await,
        Some(CommandResult::WorkspaceResolved { cwd, .. }) if cwd == second_path
    ));
    assert!(!dispatcher.has_pending_tasks());
    Ok(())
}

#[derive(Debug, thiserror::Error)]
enum TestError {
    #[error(transparent)]
    Client(#[from] AcpClientError),
    #[error(transparent)]
    Join(#[from] JoinError),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

async fn disconnected_client() -> Result<AcpClientHandle, TestError> {
    LocalSet::new()
        .run_until(async {
            let (agent_transport, client_transport) = duplex_pair();
            let agent = Agent.builder().on_receive_request(
                async |_: InitializeRequest, responder, _cx| {
                    responder.respond(InitializeResponse::new(ProtocolVersion::V1))
                },
                acp::on_receive_request!(),
            );
            let server = spawn_local(agent.connect_to(agent_transport));
            let client = connect_acp_client(client_transport, InitializeRequest::new(ProtocolVersion::V1)).await?;
            client.handle.disconnect().await;
            let _ = server.await?;
            Ok(client.handle)
        })
        .await
}
