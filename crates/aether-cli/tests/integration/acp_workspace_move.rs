use acp_utils::notifications::{
    WorkspaceListParams, WorkspaceListResponse, WorkspaceMoveParams, WorkspaceMoveResponse, WorkspaceMoveTarget,
};
use aether_cli::acp::testing::AcpTestHarness;
use aether_cli::workspace::testing::{clone_repo, git, git_status, init_repo};
use agent_client_protocol::schema::v1::ListSessionsRequest;
use std::fs;
use std::future::Future;
use tokio::task::LocalSet;

#[tokio::test(flavor = "current_thread")]
async fn workspace_list_registers_source_repo_and_marks_it_current() {
    with_harness(|harness| async move {
        let tmp = tempfile::tempdir().unwrap();
        let repo = init_repo(tmp.path(), "repo");
        let sub = repo.join("sub");
        fs::create_dir_all(&sub).unwrap();
        harness.append_stored_session_in("s1", "2026-05-01T00:00:00Z", &sub);

        let response = list(&harness, "s1").await;

        assert_eq!(response.workspaces.len(), 1);
        let entry = &response.workspaces[0];
        assert!(entry.is_current);
        assert_eq!(entry.path, repo.canonicalize().unwrap());
    })
    .await;
}

#[tokio::test(flavor = "current_thread")]
async fn workspace_move_to_new_sibling_clones_changes_and_resets_source() {
    with_harness(|harness| async move {
        let tmp = tempfile::tempdir().unwrap();
        let repo = init_repo(tmp.path(), "repo");
        harness.append_stored_session_in("s1", "2026-05-01T00:00:00Z", &repo);

        fs::write(repo.join("committed.txt"), "modified\n").unwrap();
        git(&repo, &["add", "committed.txt"]);
        fs::write(repo.join("untracked.txt"), "new file\n").unwrap();

        let response = move_workspace(&harness, "s1", WorkspaceMoveTarget::New { name: "repo-2".to_string() })
            .await
            .expect("move succeeds");

        let clone = tmp.path().join("repo-2");
        assert_eq!(response.new_cwd, clone.canonicalize().unwrap());
        assert_eq!(fs::read_to_string(clone.join("committed.txt")).unwrap(), "modified\n");
        assert_eq!(fs::read_to_string(clone.join("untracked.txt")).unwrap(), "new file\n");
        assert!(git_status(&repo).is_empty(), "source should be clean after move");

        let sessions =
            harness.client_cx.send_request(ListSessionsRequest::new()).block_task().await.expect("list sessions");
        let session = sessions.sessions.iter().find(|s| s.session_id.0.as_ref() == "s1").expect("session exists");
        assert_eq!(session.cwd, response.new_cwd);
    })
    .await;
}

#[tokio::test(flavor = "current_thread")]
async fn workspace_move_to_existing_clean_target_applies_changes() {
    with_harness(|harness| async move {
        let tmp = tempfile::tempdir().unwrap();
        let repo = init_repo(tmp.path(), "repo");
        let clone = clone_repo(&repo, tmp.path().join("clone"));
        harness.append_stored_session_in("s1", "2026-05-01T00:00:00Z", &repo);

        fs::write(repo.join("committed.txt"), "edited\n").unwrap();

        let response = move_workspace(&harness, "s1", WorkspaceMoveTarget::Existing { path: clone.clone() })
            .await
            .expect("move succeeds");

        assert_eq!(response.new_cwd, clone.canonicalize().unwrap());
        assert_eq!(fs::read_to_string(clone.join("committed.txt")).unwrap(), "edited\n");
        assert!(git_status(&repo).is_empty(), "source should be clean after move");
    })
    .await;
}

#[tokio::test(flavor = "current_thread")]
async fn workspace_move_to_existing_target_on_different_head_fails_actionably() {
    with_harness(|harness| async move {
        let tmp = tempfile::tempdir().unwrap();
        let repo = init_repo(tmp.path(), "repo");
        let clone = clone_repo(&repo, tmp.path().join("clone"));
        harness.append_stored_session_in("s1", "2026-05-01T00:00:00Z", &repo);

        fs::write(clone.join("target-only.txt"), "target commit\n").unwrap();
        git(&clone, &["add", "."]);
        git(&clone, &["commit", "-m", "target diverges"]);
        fs::write(repo.join("committed.txt"), "edited\n").unwrap();

        let error = move_workspace(&harness, "s1", WorkspaceMoveTarget::Existing { path: clone })
            .await
            .expect_err("move should fail");

        assert!(error.message.contains("different commit"), "unexpected error: {}", error.message);
        assert_eq!(fs::read_to_string(repo.join("committed.txt")).unwrap(), "edited\n");
    })
    .await;
}

#[tokio::test(flavor = "current_thread")]
async fn workspace_move_to_dirty_target_fails_and_leaves_source_untouched() {
    with_harness(|harness| async move {
        let tmp = tempfile::tempdir().unwrap();
        let repo = init_repo(tmp.path(), "repo");
        let clone = clone_repo(&repo, tmp.path().join("clone"));
        harness.append_stored_session_in("s1", "2026-05-01T00:00:00Z", &repo);

        fs::write(repo.join("committed.txt"), "edited\n").unwrap();
        fs::write(clone.join("dirty.txt"), "target change\n").unwrap();

        let error = move_workspace(&harness, "s1", WorkspaceMoveTarget::Existing { path: clone })
            .await
            .expect_err("move should fail");

        assert!(error.message.contains("uncommitted changes"), "unexpected error: {}", error.message);
        assert_eq!(fs::read_to_string(repo.join("committed.txt")).unwrap(), "edited\n");
    })
    .await;
}

#[tokio::test(flavor = "current_thread")]
async fn workspace_move_rejects_workspace_from_different_repository() {
    with_harness(|harness| async move {
        let tmp = tempfile::tempdir().unwrap();
        let repo = init_repo(tmp.path(), "repo");
        let other = init_repo(tmp.path(), "other");
        harness.append_stored_session_in("s1", "2026-05-01T00:00:00Z", &repo);

        let error = move_workspace(&harness, "s1", WorkspaceMoveTarget::Existing { path: other })
            .await
            .expect_err("move should fail");

        assert!(error.message.contains("different repository"), "unexpected error: {}", error.message);
    })
    .await;
}

#[tokio::test(flavor = "current_thread")]
async fn workspace_move_rejects_invalid_new_names() {
    with_harness(|harness| async move {
        let tmp = tempfile::tempdir().unwrap();
        let repo = init_repo(tmp.path(), "repo");
        init_repo(tmp.path(), "taken");
        harness.append_stored_session_in("s1", "2026-05-01T00:00:00Z", &repo);

        for name in ["", "a/b", "..", "taken"] {
            let result = move_workspace(&harness, "s1", WorkspaceMoveTarget::New { name: name.to_string() }).await;
            assert!(result.is_err(), "name {name:?} should be rejected");
        }
    })
    .await;
}

#[tokio::test(flavor = "current_thread")]
async fn workspace_list_omits_deleted_workspaces() {
    with_harness(|harness| async move {
        let tmp = tempfile::tempdir().unwrap();
        let repo = init_repo(tmp.path(), "repo");
        harness.append_stored_session_in("s1", "2026-05-01T00:00:00Z", &repo);

        move_workspace(&harness, "s1", WorkspaceMoveTarget::New { name: "repo-2".to_string() })
            .await
            .expect("move succeeds");
        harness.append_stored_session_in("s2", "2026-05-02T00:00:00Z", &repo);
        assert_eq!(list(&harness, "s2").await.workspaces.len(), 2);

        fs::remove_dir_all(tmp.path().join("repo-2")).unwrap();

        let response = list(&harness, "s2").await;
        assert_eq!(response.workspaces.len(), 1);
        assert!(response.workspaces[0].is_current);
    })
    .await;
}

#[tokio::test(flavor = "current_thread")]
async fn workspace_move_relocates_session_to_matching_subdirectory() {
    with_harness(|harness| async move {
        let tmp = tempfile::tempdir().unwrap();
        let repo = init_repo(tmp.path(), "repo");
        let sub = repo.join("sub");
        fs::create_dir_all(&sub).unwrap();
        fs::write(sub.join("file.txt"), "content\n").unwrap();
        git(&repo, &["add", "."]);
        git(&repo, &["commit", "-m", "add subdir"]);
        harness.append_stored_session_in("s1", "2026-05-01T00:00:00Z", &sub);

        let response = move_workspace(&harness, "s1", WorkspaceMoveTarget::New { name: "repo-2".to_string() })
            .await
            .expect("move succeeds");

        assert_eq!(response.new_cwd, tmp.path().join("repo-2").canonicalize().unwrap().join("sub"));
    })
    .await;
}

async fn with_harness<F, Fut>(body: F)
where
    F: FnOnce(AcpTestHarness) -> Fut,
    Fut: Future<Output = ()>,
{
    LocalSet::new()
        .run_until(async move {
            let harness = AcpTestHarness::start().await;
            body(harness).await;
        })
        .await;
}

async fn list(harness: &AcpTestHarness, session_id: &str) -> WorkspaceListResponse {
    harness
        .client_cx
        .send_request(WorkspaceListParams { session_id: session_id.to_string() })
        .block_task()
        .await
        .expect("workspace list succeeds")
}

async fn move_workspace(
    harness: &AcpTestHarness,
    session_id: &str,
    target: WorkspaceMoveTarget,
) -> Result<WorkspaceMoveResponse, agent_client_protocol::Error> {
    harness
        .client_cx
        .send_request(WorkspaceMoveParams { session_id: session_id.to_string(), target })
        .block_task()
        .await
}
