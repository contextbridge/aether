#![cfg(feature = "testing")]

use acp_utils::client::{AcpClientError, AcpClientHandle, connect_acp_client};
use acp_utils::testing::duplex_pair;
use agent_client_protocol::schema::ProtocolVersion;
use agent_client_protocol::schema::v2::{Implementation, InitializeRequest, InitializeResponse, SessionId};
use agent_client_protocol::{self as acp, Agent};
use tempfile::TempDir;
use tokio::task::{JoinError, LocalSet, spawn_local};
use wisp::command::{AgentCommand, Command, CommandResult, FailedCommand, GitCommand, GitWatchCommand};
use wisp::file_index::index_files_with_limit;
use wisp::git_review::{DiffScope, GitDiffError, GitDiffEvent, GitWatchError, GitWatchEvent};
use wisp::request::RequestId;
use wisp::runtime::CommandDispatcher;

#[path = "support/git_repo.rs"]
mod git_repo;
use git_repo::Repo;

#[tokio::test]
async fn git_watch_delivers_external_changes_without_other_pending_work() {
    let repo = Repo::init();
    let review_id = RequestId::next();
    let mut dispatcher = CommandDispatcher::new(disconnected_client().await.expect("disconnected client"));
    dispatcher.dispatch(Command::GitWatch(GitWatchCommand::Open {
        review_id,
        working_dir: repo.root.clone(),
        scope: DiffScope::Both,
    }));
    let initial = next_watch_event(&mut dispatcher).await;
    assert_eq!(initial.review_id, review_id);
    assert!(initial.result.expect("initial snapshot").snapshot.document.files.is_empty());
    assert!(dispatcher.has_pending_tasks(), "idle subscriptions must remain selectable");

    repo.write("external.rs", "fn external() {}\n");
    let expected = repo.load(DiffScope::Both).await;
    let changed = next_watch_event(&mut dispatcher).await;
    assert_eq!(changed.review_id, review_id);
    assert_eq!(*changed.result.expect("external edit snapshot").snapshot.document, expected);
    dispatcher.dispatch(Command::GitWatch(GitWatchCommand::Close { review_id }));
    assert!(!dispatcher.has_pending_tasks());
    assert!(dispatcher.next_result().await.is_none());
    dispatcher.shutdown().await;
}

#[tokio::test]
async fn git_watch_acknowledges_unchanged_refreshes_and_tracks_the_current_scope() {
    let repo = Repo::init();
    repo.write("file.txt", "original\n");
    repo.git(&["add", "."]);
    repo.git(&["commit", "-m", "initial"]);
    repo.write("file.txt", "changed\n");
    let review_id = RequestId::next();
    let mut dispatcher = CommandDispatcher::new(disconnected_client().await.expect("disconnected client"));
    dispatcher.dispatch(Command::GitWatch(GitWatchCommand::Open {
        review_id,
        working_dir: repo.root.clone(),
        scope: DiffScope::Both,
    }));
    next_watch_event(&mut dispatcher).await.result.expect("initial snapshot");
    next_git_completion(&mut dispatcher).await.result.expect("startup complete");
    dispatcher.dispatch(Command::GitWatch(GitWatchCommand::Refresh { review_id, scope: DiffScope::Both }));
    let refreshed = next_git_completion(&mut dispatcher).await;
    assert_eq!(refreshed.review_id, review_id);
    refreshed.result.expect("unchanged refresh complete");
    dispatcher.dispatch(Command::GitWatch(GitWatchCommand::Refresh { review_id, scope: DiffScope::Staged }));
    let staged = next_watch_event(&mut dispatcher).await;
    let staged = staged.result.expect("staged snapshot");
    assert_eq!(staged.snapshot.scope, DiffScope::Staged);
    assert!(staged.snapshot.document.files.is_empty());
    repo.git(&["add", "."]);
    let expected = repo.load(DiffScope::Staged).await;
    let changed = next_watch_event(&mut dispatcher).await;
    let changed = changed.result.expect("index change snapshot");
    assert_eq!(changed.snapshot.scope, DiffScope::Staged);
    assert_eq!(*changed.snapshot.document, expected);
    dispatcher.shutdown().await;
    assert!(!dispatcher.has_pending_tasks());
}

#[tokio::test]
async fn git_watch_can_close_during_startup_and_replace_an_old_subscription() {
    let first = Repo::init();
    let second = Repo::init();
    second.write("second.txt", "second repository\n");
    let mut dispatcher = CommandDispatcher::new(disconnected_client().await.expect("disconnected client"));
    let first_id = RequestId::next();
    dispatcher.dispatch(Command::GitWatch(GitWatchCommand::Open {
        review_id: first_id,
        working_dir: first.root.clone(),
        scope: DiffScope::Both,
    }));
    dispatcher.dispatch(Command::GitWatch(GitWatchCommand::Close { review_id: first_id }));
    assert!(!dispatcher.has_pending_tasks());
    assert!(dispatcher.next_result().await.is_none());

    dispatcher.dispatch(Command::GitWatch(GitWatchCommand::Open {
        review_id: first_id,
        working_dir: first.root.clone(),
        scope: DiffScope::Both,
    }));
    let second_id = RequestId::next();
    dispatcher.dispatch(Command::GitWatch(GitWatchCommand::Open {
        review_id: second_id,
        working_dir: second.root.clone(),
        scope: DiffScope::Both,
    }));
    dispatcher.dispatch(Command::GitWatch(GitWatchCommand::Close { review_id: first_id }));
    let opened = next_watch_event(&mut dispatcher).await;
    assert_eq!(opened.review_id, second_id);
    assert_eq!(opened.result.expect("replacement snapshot").snapshot.document.files[0].path.as_str(), "second.txt");
    dispatcher.shutdown().await;
    assert!(!dispatcher.has_pending_tasks());
}

#[tokio::test]
async fn git_watch_retries_failed_startup_on_explicit_refresh() {
    let repo = Repo::init();
    let metadata = repo.root.join(".git");
    let saved = repo.root.join("saved-metadata");
    std::fs::rename(&metadata, &saved).expect("hide repository metadata");
    let review_id = RequestId::next();
    let mut dispatcher = CommandDispatcher::new(disconnected_client().await.expect("disconnected client"));
    dispatcher.dispatch(Command::GitWatch(GitWatchCommand::Open {
        review_id,
        working_dir: repo.root.clone(),
        scope: DiffScope::Both,
    }));
    assert!(next_watch_event(&mut dispatcher).await.result.is_err());
    std::fs::rename(saved, metadata).expect("restore repository metadata");
    repo.write("recovered.txt", "recovered\n");
    dispatcher.dispatch(Command::GitWatch(GitWatchCommand::Refresh { review_id, scope: DiffScope::Both }));
    let recovered = next_watch_event(&mut dispatcher).await;
    assert_eq!(recovered.review_id, review_id);
    assert_eq!(recovered.result.expect("recovered watch").snapshot.document.files[0].path.as_str(), "recovered.txt");
    dispatcher.shutdown().await;
    assert!(!dispatcher.has_pending_tasks());
}

#[tokio::test]
async fn git_mutations_finish_in_dispatch_order_and_shutdown_drains_them() -> Result<(), TestError> {
    use clankerdiff_ratatui::diff::RepositoryAction;
    let repo = Repo::init();
    repo.write("file.txt", "new contents\n");
    let mut dispatcher = CommandDispatcher::new(disconnected_client().await.expect("disconnected client"));
    let review_id = RequestId::next();
    dispatcher.dispatch(Command::GitWatch(GitWatchCommand::Open {
        review_id,
        working_dir: repo.root.clone(),
        scope: DiffScope::Both,
    }));
    next_watch_event(&mut dispatcher).await.result.expect("initial snapshot");
    for action in [
        RepositoryAction::StageAll,
        RepositoryAction::UnstageAll,
        RepositoryAction::StageAll,
        RepositoryAction::Commit { message: "ordered commit".into() },
    ] {
        dispatcher.dispatch(Command::Git(GitCommand::Apply { review_id, action }));
    }
    dispatcher.dispatch(Command::GitWatch(GitWatchCommand::Close { review_id }));
    assert!(dispatcher.has_pending_tasks(), "closing the watch must not cancel mutations");
    dispatcher.shutdown().await;
    assert!(!dispatcher.has_pending_tasks());
    assert_eq!(repo.git(&["show", "HEAD:file.txt"]), b"new contents\n");
    assert!(repo.git(&["status", "--porcelain"]).is_empty());
    assert!(repo.load(DiffScope::Both).await.files.is_empty());
    Ok(())
}

#[tokio::test]
async fn unchanged_git_refresh_completes_without_a_synthetic_snapshot() {
    let repo = Repo::init();
    let review_id = RequestId::next();
    let mut dispatcher = CommandDispatcher::new(disconnected_client().await.expect("disconnected client"));
    dispatcher.dispatch(Command::GitWatch(GitWatchCommand::Open {
        review_id,
        working_dir: repo.root.clone(),
        scope: DiffScope::Both,
    }));
    next_watch_event(&mut dispatcher).await.result.expect("initial snapshot");
    next_git_completion(&mut dispatcher).await.result.expect("startup complete");
    dispatcher.dispatch(Command::GitWatch(GitWatchCommand::Refresh { review_id, scope: DiffScope::Both }));
    assert!(
        matches!(dispatcher.next_result().await, Some(CommandResult::GitDiff(_))),
        "an unchanged refresh completes independently of snapshot delivery"
    );
    dispatcher.shutdown().await;
}

#[tokio::test]
async fn git_actions_publish_watcher_updates_without_explicit_refresh() {
    use clankerdiff_ratatui::diff::{RepositoryAction, StageState};
    let repo = Repo::init();
    repo.write("file.txt", "new contents\n");
    let review_id = RequestId::next();
    let mut dispatcher = CommandDispatcher::new(disconnected_client().await.expect("disconnected client"));
    dispatcher.dispatch(Command::GitWatch(GitWatchCommand::Open {
        review_id,
        working_dir: repo.root.clone(),
        scope: DiffScope::Both,
    }));
    next_watch_event(&mut dispatcher).await.result.expect("initial snapshot");
    next_git_completion(&mut dispatcher).await.result.expect("startup complete");
    dispatcher.dispatch(Command::Git(GitCommand::Apply { review_id, action: RepositoryAction::StageAll }));
    let mut completed = false;
    let mut staged = false;
    while !completed || !staged {
        match dispatcher.next_result().await {
            Some(CommandResult::GitDiff(event)) => {
                assert_eq!(event.review_id, review_id);
                event.result.expect("stage completed");
                completed = true;
            }
            Some(CommandResult::GitWatch(event)) => {
                assert_eq!(event.review_id, review_id);
                let state = event.result.expect("watcher state");
                assert!(state.error.is_none());
                staged = state.snapshot.document.files.iter().any(|file| file.staged == StageState::Staged);
            }
            _ => panic!("expected Git state or completion"),
        }
    }
    assert!(repo.git(&["diff", "--name-only", "--cached"]).starts_with(b"file.txt"));
    dispatcher.shutdown().await;
}

#[tokio::test]
async fn replacing_a_review_discards_its_pending_scope_completion() {
    use futures::FutureExt;
    let repo = Repo::init();
    let replacement = Repo::init();
    replacement.write("replacement.txt", "replacement\n");
    let mut dispatcher = CommandDispatcher::new(disconnected_client().await.expect("disconnected client"));
    let old_id = RequestId::next();
    dispatcher.dispatch(Command::GitWatch(GitWatchCommand::Open {
        review_id: old_id,
        working_dir: repo.root.clone(),
        scope: DiffScope::Both,
    }));
    next_watch_event(&mut dispatcher).await.result.expect("initial snapshot");
    next_git_completion(&mut dispatcher).await.result.expect("startup complete");
    dispatcher.dispatch(Command::GitWatch(GitWatchCommand::Refresh { review_id: old_id, scope: DiffScope::Staged }));
    let _ = dispatcher.next_result().now_or_never();
    let new_id = RequestId::next();
    dispatcher.dispatch(Command::GitWatch(GitWatchCommand::Open {
        review_id: new_id,
        working_dir: replacement.root.clone(),
        scope: DiffScope::Both,
    }));
    let Some(CommandResult::GitWatch(event)) = dispatcher.next_result().await else { panic!("replacement snapshot") };
    assert_eq!(event.review_id, new_id);
    assert_eq!(event.result.unwrap().snapshot.document.files[0].path.as_str(), "replacement.txt");
    let completion = next_git_completion(&mut dispatcher).await;
    assert_eq!(completion.review_id, new_id);
    completion.result.expect("replacement startup complete");
    dispatcher.dispatch(Command::GitWatch(GitWatchCommand::Close { review_id: new_id }));
    assert!(!dispatcher.has_pending_tasks());
    assert!(dispatcher.next_result().await.is_none());
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
async fn failed_git_startup_reports_a_typed_error_without_pending_work() {
    let mut dispatcher = CommandDispatcher::new(disconnected_client().await.expect("disconnected client"));
    let outside_repository = TempDir::new().unwrap();
    let review_id = RequestId::next();
    dispatcher.dispatch(Command::GitWatch(GitWatchCommand::Open {
        review_id,
        working_dir: outside_repository.path().to_path_buf(),
        scope: DiffScope::Both,
    }));
    assert!(dispatcher.has_pending_tasks());
    let event = next_watch_event(&mut dispatcher).await;
    assert_eq!(event.review_id, review_id);
    assert!(matches!(&*event.result.unwrap_err(), GitWatchError::Git(GitDiffError::NotRepository)));
    assert!(!dispatcher.has_pending_tasks());
    assert!(dispatcher.next_result().await.is_none());
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

async fn next_watch_event(dispatcher: &mut CommandDispatcher) -> GitWatchEvent {
    loop {
        match dispatcher.next_result().await {
            Some(CommandResult::GitWatch(event)) => return event,
            Some(CommandResult::GitDiff(event)) => event.result.expect("operation completed"),
            _ => panic!("expected a Git watch event"),
        }
    }
}

async fn next_git_completion(dispatcher: &mut CommandDispatcher) -> GitDiffEvent {
    loop {
        match dispatcher.next_result().await {
            Some(CommandResult::GitDiff(event)) => return event,
            Some(CommandResult::GitWatch(event)) => {
                event.result.expect("watch remains healthy");
            }
            _ => panic!("expected a Git operation completion"),
        }
    }
}
