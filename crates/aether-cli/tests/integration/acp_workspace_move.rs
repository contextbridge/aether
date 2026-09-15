use acp_utils::notifications::{
    WorkspaceListParams, WorkspaceListResponse, WorkspaceMoveParams, WorkspaceMoveResponse, WorkspaceMoveTarget,
};
use aether_cli::acp::testing::AcpTestHarness;
use aether_cli::workspace::testing::{clone_repo, git, git_status, init_repo};
use agent_client_protocol::schema::v2::{
    AbsolutePath, ListSessionsRequest, PromptRequest, ResumeSessionRequest, StopReason,
};
use std::fs;

#[tokio::test(flavor = "current_thread")]
async fn workspace_list_registers_source_repo_and_marks_it_current() {
    AcpTestHarness::run(|harness| async move {
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
    AcpTestHarness::run(|harness| async move {
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
        assert_eq!(session.cwd.0, response.new_cwd);
    })
    .await;
}

#[tokio::test(flavor = "current_thread")]
async fn workspace_move_to_existing_clean_target_applies_changes() {
    AcpTestHarness::run(|harness| async move {
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
    AcpTestHarness::run(|harness| async move {
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

        assert!(error.to_string().contains("different commit"), "unexpected error: {error}");
        assert_eq!(fs::read_to_string(repo.join("committed.txt")).unwrap(), "edited\n");
    })
    .await;
}

#[tokio::test(flavor = "current_thread")]
async fn workspace_move_to_dirty_target_fails_and_leaves_source_untouched() {
    AcpTestHarness::run(|harness| async move {
        let tmp = tempfile::tempdir().unwrap();
        let repo = init_repo(tmp.path(), "repo");
        let clone = clone_repo(&repo, tmp.path().join("clone"));
        harness.append_stored_session_in("s1", "2026-05-01T00:00:00Z", &repo);

        fs::write(repo.join("committed.txt"), "edited\n").unwrap();
        fs::write(clone.join("dirty.txt"), "target change\n").unwrap();

        let error = move_workspace(&harness, "s1", WorkspaceMoveTarget::Existing { path: clone })
            .await
            .expect_err("move should fail");

        assert!(error.to_string().contains("uncommitted changes"), "unexpected error: {error}");
        assert_eq!(fs::read_to_string(repo.join("committed.txt")).unwrap(), "edited\n");
    })
    .await;
}

#[tokio::test(flavor = "current_thread")]
async fn workspace_move_rejects_workspace_from_different_repository() {
    AcpTestHarness::run(|harness| async move {
        let tmp = tempfile::tempdir().unwrap();
        let repo = init_repo(tmp.path(), "repo");
        let other = init_repo(tmp.path(), "other");
        harness.append_stored_session_in("s1", "2026-05-01T00:00:00Z", &repo);

        let error = move_workspace(&harness, "s1", WorkspaceMoveTarget::Existing { path: other })
            .await
            .expect_err("move should fail");

        assert!(error.to_string().contains("different repository"), "unexpected error: {error}");
    })
    .await;
}

#[tokio::test(flavor = "current_thread")]
async fn workspace_move_rejects_invalid_new_names() {
    AcpTestHarness::run(|harness| async move {
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
    AcpTestHarness::run(|harness| async move {
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
    AcpTestHarness::run(|harness| async move {
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

#[tokio::test(flavor = "current_thread")]
async fn live_workspace_move_stops_old_actor_and_allows_restore_in_new_cwd() {
    AcpTestHarness::run(|mut harness| async move {
        let tmp = tempfile::tempdir().unwrap();
        let repo = init_repo(tmp.path(), "repo");
        harness.append_stored_session_in("live", "2026-05-01T00:00:00Z", &repo);
        harness
            .client_cx
            .send_request(ResumeSessionRequest::new("live", AbsolutePath::new(repo)))
            .block_task()
            .await
            .expect("restore source");
        let moved = move_workspace(&harness, "live", WorkspaceMoveTarget::New { name: "moved".into() })
            .await
            .expect("move live workspace");
        assert!(
            harness
                .client_cx
                .send_request(PromptRequest::new("live", vec!["old actor".into()]))
                .block_task()
                .await
                .is_err(),
            "move must remove the old actor"
        );
        harness
            .client_cx
            .send_request(ResumeSessionRequest::new("live", AbsolutePath::new(moved.new_cwd)))
            .block_task()
            .await
            .expect("restore moved session");
        harness
            .client_cx
            .send_request(PromptRequest::new("live", vec!["new workspace".into()]))
            .block_task()
            .await
            .expect("prompt restored actor");
        harness.expect_idle(&"live".into(), StopReason::EndTurn).await;
        harness.resume_agent().assert_saw(&["new workspace"]);
        harness.shutdown().await;
    })
    .await;
}

#[tokio::test(flavor = "current_thread")]
async fn failed_and_unrelated_moves_preserve_live_actor() {
    AcpTestHarness::run(|mut harness| async move {
        let tmp = tempfile::tempdir().unwrap();
        let repo = init_repo(tmp.path(), "repo");
        harness.append_stored_session_in("other", "2026-05-01T00:00:00Z", &repo);
        let active = harness.insert_agent_switching_session().await;
        harness.append_stored_session_in(active.session_id().0.as_ref(), "2026-05-01T00:00:00Z", &repo);
        assert!(
            move_workspace(&harness, active.session_id().0.as_ref(), WorkspaceMoveTarget::New { name: "..".into() })
                .await
                .is_err()
        );
        move_workspace(&harness, "other", WorkspaceMoveTarget::New { name: "moved".into() })
            .await
            .expect("move unrelated saved session");
        harness
            .client_cx
            .send_request(PromptRequest::new(active.session_id().clone(), vec!["still here".into()]))
            .block_task()
            .await
            .expect("original actor still accepts prompts");
        harness.expect_idle(active.session_id(), StopReason::EndTurn).await;
        active.planner().assert_saw(&["still here"]);
        harness.shutdown().await;
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
