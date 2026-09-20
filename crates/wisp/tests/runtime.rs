#![cfg(feature = "testing")]

use acp_utils::client::{AcpClientError, AcpClientHandle, connect_acp_client};
use acp_utils::testing::{FakeAgent, duplex_pair};
use agent_client_protocol::schema::ProtocolVersion;
use agent_client_protocol::schema::v2::{Implementation, InitializeRequest, InitializeResponse, SessionId};
use agent_client_protocol::{self as acp, Agent};
use std::sync::Arc;
use tempfile::TempDir;
use tokio::task::{JoinError, LocalSet, spawn_local};
use wisp::command::{AgentCommand, Command, CommandResult, GitReviewCommand};
use wisp::file_index::index_files_with_limit;
use wisp::git_review::{DiffScope, DocumentUpdate, Event, FileDiff, FileEntry, LIVE_PROTOCOL_VERSION, ServerMessage};
use wisp::runtime::CommandDispatcher;
use wisp::session::workspace_status::WorkspaceStatus;

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
        assert_eq!(status, WorkspaceStatus::initial(&cwd));
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

            dispatcher.dispatch(Command::GitReview(GitReviewCommand::Open { session_id: "s1".to_string() }));
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
    Join(#[from] JoinError),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

async fn disconnected_client() -> Result<AcpClientHandle, TestError> {
    LocalSet::new()
        .run_until(async {
            let (agent_transport, client_transport) = duplex_pair();
            let agent = Agent.v2().on_receive_request(
                async |_: InitializeRequest, responder, _cx| {
                    responder.respond(InitializeResponse::new(ProtocolVersion::V2, Implementation::new("fake", "1")))
                },
                acp::on_receive_request!(),
            );
            let server = spawn_local(agent.connect_to(agent_transport));
            let client = connect_acp_client(
                client_transport,
                InitializeRequest::new(ProtocolVersion::V2, Implementation::new("wisp", "1")),
            )
            .await?;
            client.handle.disconnect().await;
            let _ = server.await?;
            Ok(client.handle)
        })
        .await
}
