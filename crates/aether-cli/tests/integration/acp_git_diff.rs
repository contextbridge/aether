use acp_utils::notifications::{GitDiffClosePayload, GitDiffCommandPayload, WorkspaceStatusPayload};
use aether_cli::acp::testing::AcpTestHarness;
use aether_cli::workspace::testing::{git, init_repo};
use agent_client_protocol::schema::v2::AbsolutePath;
use clankerdiff_core::{DiffScope, RepositoryAction};
use clankerdiff_protocol::client::ClientCommand;
use clankerdiff_protocol::shared::{DocumentUpdate, Event, FileEntry, LIVE_PROTOCOL_VERSION};
use std::fs;

#[tokio::test(flavor = "current_thread")]
async fn git_diff_open_publishes_snapshot_over_duplex() {
    AcpTestHarness::run(|mut harness| async move {
        let tmp = tempfile::tempdir().unwrap();
        let repo = init_repo(tmp.path(), "repo");
        fs::write(repo.join("changed.txt"), "edited\n").unwrap();
        harness.append_stored_session_in("s1", "2026-05-01T00:00:00Z", &repo);
        live(&harness, "s1", &repo).await;

        initialize(&harness, "s1", DiffScope::Both);
        assert!(matches!(next_event(&mut harness).await, Event::Initialize { .. }));
        let Event::Document(update) = next_event(&mut harness).await else {
            panic!("open must publish a document");
        };
        assert_eq!(update.scope, DiffScope::Both);
        assert!(
            changed_paths(&update).iter().any(|path| path == "changed.txt"),
            "unexpected files: {:?}",
            changed_paths(&update)
        );
        close(&harness, "s1");
    })
    .await;
}

#[tokio::test(flavor = "current_thread")]
async fn git_diff_refresh_tracks_scope_and_reports_status_ref() {
    AcpTestHarness::run(|mut harness| async move {
        let tmp = tempfile::tempdir().unwrap();
        let repo = init_repo(tmp.path(), "repo");
        fs::write(repo.join("committed.txt"), "working edit\n").unwrap();
        git(&repo, &["add", "committed.txt"]);
        harness.append_stored_session_in("s1", "2026-05-01T00:00:00Z", &repo);
        live(&harness, "s1", &repo).await;

        initialize(&harness, "s1", DiffScope::Both);
        let _ = next_event(&mut harness).await;
        let _ = next_event(&mut harness).await;

        send(&harness, "s1", ClientCommand::SetScope(DiffScope::Staged));
        let Event::Document(update) = next_event(&mut harness).await else {
            panic!("set scope must publish a document");
        };
        assert_eq!(update.scope, DiffScope::Staged);
        assert!(changed_paths(&update).iter().any(|path| path == "committed.txt"), "staged change must be visible");
        assert!(matches!(next_event(&mut harness).await, Event::RequestResult(Ok(()))));

        let status = harness
            .client_cx
            .send_request(WorkspaceStatusPayload { session_id: "s1".to_string() })
            .block_task()
            .await
            .expect("status succeeds");
        assert!(!status.display_dir.is_empty());
    })
    .await;
}

#[tokio::test(flavor = "current_thread")]
async fn git_diff_apply_stage_commit_reaches_clean_tree() {
    AcpTestHarness::run(|mut harness| async move {
        let tmp = tempfile::tempdir().unwrap();
        let repo = init_repo(tmp.path(), "repo");
        fs::write(repo.join("file.txt"), "two\n").unwrap();
        harness.append_stored_session_in("s1", "2026-05-01T00:00:00Z", &repo);
        live(&harness, "s1", &repo).await;

        initialize(&harness, "s1", DiffScope::Both);
        let _ = next_event(&mut harness).await;
        let _ = next_event(&mut harness).await;

        send(&harness, "s1", ClientCommand::Apply(RepositoryAction::StageAll));
        let Event::Document(_) = next_event(&mut harness).await else { panic!("staging must publish") };
        assert!(matches!(next_event(&mut harness).await, Event::RequestResult(Ok(()))));

        send(&harness, "s1", ClientCommand::Apply(RepositoryAction::Commit { message: "update".to_string() }));
        let Event::Document(update) = next_event(&mut harness).await else { panic!("commit must publish") };
        assert!(update.files.is_empty(), "committed tree must be clean");
        assert!(matches!(next_event(&mut harness).await, Event::RequestResult(Ok(()))));
    })
    .await;
}

#[tokio::test(flavor = "current_thread")]
async fn git_diff_open_outside_repository_reports_not_repository() {
    AcpTestHarness::run(|mut harness| async move {
        let tmp = tempfile::tempdir().unwrap();
        harness.append_stored_session_in("s1", "2026-05-01T00:00:00Z", tmp.path());
        live(&harness, "s1", tmp.path()).await;

        initialize(&harness, "s1", DiffScope::Both);
        let Event::Error(error) = next_event(&mut harness).await else { panic!("expected an error event") };
        assert!(error.message.contains("not inside a Git worktree"), "unexpected message: {}", error.message);

        let status = harness
            .client_cx
            .send_request(WorkspaceStatusPayload { session_id: "s1".to_string() })
            .block_task()
            .await
            .expect("status succeeds outside a repo");
        assert_eq!(status.git_ref, None);
    })
    .await;
}

#[tokio::test(flavor = "current_thread")]
async fn git_diff_commands_for_unknown_sessions_are_ignored() {
    AcpTestHarness::run(|mut harness| async move {
        let tmp = tempfile::tempdir().unwrap();
        let repo = init_repo(tmp.path(), "repo");
        fs::write(repo.join("changed.txt"), "edited\n").unwrap();
        harness.append_stored_session_in("s1", "2026-05-01T00:00:00Z", &repo);
        live(&harness, "s1", &repo).await;

        initialize(&harness, "missing", DiffScope::Both);
        initialize(&harness, "s1", DiffScope::Both);

        let first = next_event(&mut harness).await;
        assert!(matches!(first, Event::Initialize { .. }), "the unknown session must not answer, got {first:?}");
        assert!(matches!(next_event(&mut harness).await, Event::Document(_)));
    })
    .await;
}

async fn live(harness: &AcpTestHarness, session_id: &str, cwd: &std::path::Path) {
    use agent_client_protocol::schema::v2::ResumeSessionRequest;
    harness
        .client_cx
        .send_request(ResumeSessionRequest::new(session_id, AbsolutePath::new(cwd)))
        .block_task()
        .await
        .expect("resume session for git diff");
}

fn initialize(harness: &AcpTestHarness, session_id: &str, scope: DiffScope) {
    send(harness, session_id, ClientCommand::Initialize { protocol_version: LIVE_PROTOCOL_VERSION, scope });
}

fn close(harness: &AcpTestHarness, session_id: &str) {
    harness
        .client_cx
        .send_notification(GitDiffClosePayload { session_id: session_id.to_string() })
        .expect("close sends");
}

fn send(harness: &AcpTestHarness, session_id: &str, command: ClientCommand) {
    harness
        .client_cx
        .send_notification(GitDiffCommandPayload { session_id: session_id.to_string(), command })
        .expect("git diff command sends");
}

async fn next_event(harness: &mut AcpTestHarness) -> Event<DocumentUpdate> {
    harness.peer.next_git_diff_notification().await.event
}

fn changed_paths(update: &DocumentUpdate) -> Vec<String> {
    update
        .files
        .iter()
        .map(|entry| match entry {
            FileEntry::Changed(file) => file.path.as_str().to_string(),
            FileEntry::Unchanged(path) => path.as_str().to_string(),
        })
        .collect()
}
