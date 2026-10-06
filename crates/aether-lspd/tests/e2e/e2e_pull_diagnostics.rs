use crate::common::{DaemonHarness, poll_workspace_diagnostics, use_fake_servers, workspace_error_count};
use aether_lspd::{LanguageId, path_to_uri};
use std::time::Duration;
use tempfile::TempDir;

#[tokio::test]
async fn diagnostics_are_pulled_from_pull_only_server() {
    use_fake_servers();

    let root = TempDir::new().expect("Failed to create project");
    let index_ts = root.path().join("index.ts");
    std::fs::write(&index_ts, "const error = 1;\n").expect("Failed to write index.ts");

    let harness = DaemonHarness::spawn(root.path(), LanguageId::TypeScript).await.expect("Failed to spawn daemon");
    let client = harness.connect().await.expect("Failed to connect client");

    let diagnostics = client
        .get_diagnostics(Some(path_to_uri(&index_ts).expect("index.ts URI")))
        .await
        .expect("Failed to get diagnostics");

    assert_eq!(diagnostics.len(), 1);
    assert_eq!(diagnostics[0].diagnostics.len(), 1);
    assert_eq!(diagnostics[0].diagnostics[0].message, "error token");

    harness.kill().await.expect("Failed to kill daemon");
}

#[tokio::test]
async fn cancelled_pull_is_retried() {
    use_fake_servers();

    let root = TempDir::new().expect("Failed to create project");
    std::fs::write(root.path().join("index.ts"), "// cancel-first-pull\nconst error = 1;\n")
        .expect("Failed to write index.ts");

    let harness = DaemonHarness::spawn(root.path(), LanguageId::TypeScript).await.expect("Failed to spawn daemon");
    let client = harness.connect().await.expect("Failed to connect client");

    let diagnostics = client.get_diagnostics(None).await.expect("Failed to get workspace diagnostics");

    assert_eq!(workspace_error_count(&diagnostics), 1, "{diagnostics:?}");

    harness.kill().await.expect("Failed to kill daemon");
}

#[tokio::test]
async fn server_refresh_request_re_pulls_dependent_files() {
    use_fake_servers();

    let root = TempDir::new().expect("Failed to create project");
    let a_ts = root.path().join("a.ts");
    let b_ts = root.path().join("b.ts");
    std::fs::write(&a_ts, "// depends: b.ts\n").expect("Failed to write a.ts");
    std::fs::write(&b_ts, "export const ok = 1;\n").expect("Failed to write b.ts");

    let harness = DaemonHarness::spawn(root.path(), LanguageId::TypeScript).await.expect("Failed to spawn daemon");
    let client = harness.connect().await.expect("Failed to connect client");

    let initial = client.get_diagnostics(None).await.expect("Failed to get workspace diagnostics");
    assert_eq!(initial.len(), 2);
    assert_eq!(workspace_error_count(&initial), 0);

    std::fs::write(&b_ts, "export const error = 1;\n").expect("Failed to write b.ts");

    let refreshed = poll_workspace_diagnostics(
        &client,
        |diagnostics| workspace_error_count(diagnostics) == 2,
        Duration::from_secs(10),
    )
    .await;
    let a_uri = path_to_uri(&a_ts).expect("a.ts URI");
    let a_diagnostics = refreshed.iter().find(|params| params.uri == a_uri).expect("a.ts diagnostics");
    assert_eq!(a_diagnostics.diagnostics[0].message, "dependency error");

    harness.kill().await.expect("Failed to kill daemon");
}
