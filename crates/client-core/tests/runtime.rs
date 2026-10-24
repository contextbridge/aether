use acp_utils::client::AcpClientError;
use acp_utils::testing::{FakeAgent, disconnected_client};
use agent_client_protocol::schema::v2::SessionId;
use clankerdiff_client::protocol::server::ServerMessage;
use clankerdiff_client::protocol::shared::{DocumentUpdate, Event, FileEntry, LIVE_PROTOCOL_VERSION};
use clankerdiff_core::{DiffScope, FileDiff};
use client_core::command::{AgentCommand, Command, CommandResult, GitReviewCommand, ReviewSource};
use client_core::composer::file_index::index_files_with_limit;
use client_core::runtime::CommandDispatcher;
use client_core::workspace_status::initial_workspace_status;
use std::sync::Arc;
use tempfile::TempDir;
use tokio::task::LocalSet;
use utils::testing::TestWorkspace;

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

    assert!(matches!(result, Some(CommandResult::Cancel(Err(_)))));
    assert!(!dispatcher.has_pending_tasks());
    assert!(dispatcher.next_result().await.is_none());
    dispatcher.shutdown().await;
    Ok(())
}

#[tokio::test]
async fn workspace_status_falls_back_to_the_session_path_without_an_agent() -> Result<(), TestError> {
    let mut dispatcher = CommandDispatcher::new(disconnected_client().await?);
    let first = TempDir::new()?;
    let second = TempDir::new()?;

    for cwd in [first.path(), second.path()] {
        dispatcher.dispatch(Command::Agent(AgentCommand::FetchWorkspaceStatus {
            session_id: "s".to_string(),
            cwd: cwd.to_path_buf(),
        }));
    }

    let mut resolved = Vec::new();
    while resolved.len() < 2 {
        let Some(CommandResult::WorkspaceResolved { cwd, status }) = dispatcher.next_result().await else {
            panic!("expected workspace status");
        };
        assert_eq!(status, initial_workspace_status(&cwd));
        resolved.push(cwd);
    }
    assert!(resolved.contains(&second.path().to_path_buf()));
    assert!(!dispatcher.has_pending_tasks());
    Ok(())
}

#[tokio::test]
async fn git_review_publishes_the_agents_document() -> Result<(), TestError> {
    LocalSet::new()
        .run_until(async {
            let client = FakeAgent::default().build().await?;
            let mut dispatcher = CommandDispatcher::new(client.handle.clone());

            dispatcher.dispatch(Command::GitReview(GitReviewCommand::Open(ReviewSource::Agent {
                session_id: "s1".to_string(),
            })));
            for message in git_diff_messages() {
                dispatcher.dispatch(Command::GitReview(GitReviewCommand::Forward(message)));
            }

            let snapshot = loop {
                if let Some(CommandResult::GitReview(state)) = dispatcher.next_result().await
                    && let Some(snapshot) = state.snapshot.clone()
                {
                    break snapshot;
                }
            };
            assert!(
                snapshot.document.files.iter().any(|file| file.path.as_str() == "changed.txt"),
                "the published snapshot must carry the agent's document"
            );

            dispatcher.shutdown().await;
            Ok::<(), TestError>(())
        })
        .await
}

#[tokio::test]
async fn a_git_review_of_a_local_working_tree_reads_it_through_a_diff_server_of_its_own() -> Result<(), TestError> {
    let repository = TestWorkspace::new().git_init().file("changed.txt", "edited\n");
    let mut dispatcher = CommandDispatcher::new(disconnected_client().await?);

    dispatcher.dispatch(Command::GitReview(GitReviewCommand::Open(ReviewSource::Local(repository.root().into()))));

    let snapshot = loop {
        if let Some(CommandResult::GitReview(state)) = dispatcher.next_result().await
            && let Some(snapshot) = state.snapshot.clone()
        {
            break snapshot;
        }
    };
    assert_eq!(snapshot.document.files.iter().map(|file| file.path.as_str()).collect::<Vec<_>>(), ["changed.txt"]);
    dispatcher.shutdown().await;
    Ok(())
}

#[tokio::test]
async fn a_git_review_outside_a_repository_fails_with_the_reason() -> Result<(), TestError> {
    let outside = TempDir::new()?;
    let mut dispatcher = CommandDispatcher::new(disconnected_client().await?);

    dispatcher.dispatch(Command::GitReview(GitReviewCommand::Open(ReviewSource::Local(outside.path().into()))));

    let Some(CommandResult::GitReview(state)) = dispatcher.next_result().await else {
        panic!("expected the review's state");
    };
    assert_eq!(state.label(), "failed");
    assert!(!dispatcher.has_pending_tasks(), "a failed review is closed");
    Ok(())
}

#[tokio::test]
async fn an_unavailable_git_review_fails_with_the_reason() -> Result<(), TestError> {
    let mut dispatcher = CommandDispatcher::new(disconnected_client().await?);

    let result = dispatcher
        .dispatch(Command::GitReview(GitReviewCommand::Open(ReviewSource::Unavailable("no diff here".to_owned()))));

    let Some(CommandResult::GitReview(state)) = result else {
        panic!("expected the review's state");
    };
    assert_eq!(state.label(), "failed");
    assert!(state.status().is_some_and(|status| status.contains("no diff here")));
    assert!(!dispatcher.has_pending_tasks());
    Ok(())
}

/// The live-protocol messages a server sends for an initialization and its first
/// document, fed straight into the review client's transport.
fn git_diff_messages() -> Vec<ServerMessage> {
    vec![
        Event::Initialize { protocol_version: LIVE_PROTOCOL_VERSION, repository_root: "/repo".to_string() },
        Event::Document(DocumentUpdate {
            scope: DiffScope::Both,
            repo_root: "/repo".to_string(),
            files: vec![FileEntry::Changed(Arc::new(FileDiff::from_texts("changed.txt", "", "edited\n").unwrap()))],
        }),
    ]
}

#[derive(Debug, thiserror::Error)]
enum TestError {
    #[error(transparent)]
    Client(#[from] AcpClientError),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}
