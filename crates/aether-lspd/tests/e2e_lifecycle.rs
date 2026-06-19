mod common;

use aether_lspd::{LanguageId, LspClient, lockfile_path, socket_path};
use common::{CargoProject, DaemonHarness, TestProject, hover_text, use_fake_rust_server};
use lsp_types::PublishDiagnosticsParams;
use std::time::{Duration, Instant};

async fn poll_workspace_diagnostics(
    client: &LspClient,
    predicate: impl Fn(&[PublishDiagnosticsParams]) -> bool,
    timeout: Duration,
) -> Vec<PublishDiagnosticsParams> {
    let start = Instant::now();
    let mut last = Vec::new();

    while start.elapsed() < timeout {
        let diagnostics = client.get_diagnostics(None).await.expect("Failed to get workspace diagnostics");
        if predicate(&diagnostics) {
            return diagnostics;
        }
        last = diagnostics;
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    panic!("workspace diagnostics timed out after {timeout:?}. Last result: {last:?}");
}

fn workspace_error_count(diagnostics: &[PublishDiagnosticsParams]) -> usize {
    diagnostics.iter().map(|params| params.diagnostics.len()).sum()
}

#[tokio::test]
async fn daemon_persists_after_client_disconnect() {
    use_fake_rust_server();

    let project = CargoProject::new("disconnect_persists").expect("Failed to create project");
    let socket_path = socket_path(project.root(), LanguageId::Rust);
    let lockfile_path = lockfile_path(&socket_path);

    let client = LspClient::connect(project.root(), LanguageId::Rust).await.expect("Failed to spawn daemon");
    client.disconnect().await.expect("Failed to disconnect client");

    assert!(lockfile_path.exists(), "Daemon should continue running after a client disconnects");
}

#[tokio::test]
async fn multiple_clients_share_fake_server_session() {
    use_fake_rust_server();

    let project = CargoProject::new("shared_session").expect("Failed to create project");

    let harness = DaemonHarness::spawn(project.root(), LanguageId::Rust).await.expect("Failed to spawn daemon");
    let client1 = harness.connect().await.expect("Failed to connect client 1");
    let client2 = harness.connect().await.expect("Failed to connect client 2");

    let uri = project.file_uri("src/main.rs");

    let hover1 = hover_text(client1.hover(uri.clone(), 0, 0).await.expect("Hover failed"));
    let hover2 = hover_text(client2.hover(uri.clone(), 0, 0).await.expect("Hover failed"));

    assert!(!hover1.is_empty());
    assert_eq!(hover1, hover2);

    harness.kill().await.expect("Failed to kill daemon");
}

#[tokio::test]
async fn diagnostics_are_available_across_clients_without_explicit_open() {
    use_fake_rust_server();

    let project = CargoProject::new("shared_diagnostics").expect("Failed to create project");
    project.add_file("src/main.rs", "fn main() { let error = 1; }\n").expect("Failed to add source file");

    let harness = DaemonHarness::spawn(project.root(), LanguageId::Rust).await.expect("Failed to spawn daemon");
    let client1 = harness.connect().await.expect("Failed to connect client 1");
    let client2 = harness.connect().await.expect("Failed to connect client 2");
    let uri = project.file_uri("src/main.rs");

    let diagnostics1 = client1.get_diagnostics(Some(uri.clone())).await.expect("Failed to get diagnostics");
    let diagnostics2 = client2.get_diagnostics(Some(uri.clone())).await.expect("Failed to get diagnostics");

    assert_eq!(diagnostics1.len(), 1);
    assert_eq!(diagnostics2.len(), 1);
    assert_eq!(diagnostics1[0].diagnostics.len(), 1);
    assert_eq!(diagnostics2[0].diagnostics.len(), 1);

    harness.kill().await.expect("Failed to kill daemon");
}

#[tokio::test]
async fn workspace_bootstrap_diagnostics_are_available_without_explicit_open() {
    use_fake_rust_server();

    let project = CargoProject::new("workspace_bootstrap").expect("Failed to create project");
    project.add_file("src/main.rs", "fn main() { let error = 1; }\n").expect("Failed to add source file");

    let harness = DaemonHarness::spawn(project.root(), LanguageId::Rust).await.expect("Failed to spawn daemon");
    let client = harness.connect().await.expect("Failed to connect client");

    let diagnostics = poll_workspace_diagnostics(
        &client,
        |diagnostics| workspace_error_count(diagnostics) > 0,
        Duration::from_secs(10),
    )
    .await;

    assert_eq!(diagnostics.len(), 1);
    assert_eq!(workspace_error_count(&diagnostics), 1);

    harness.kill().await.expect("Failed to kill daemon");
}

#[tokio::test]
async fn workspace_diagnostics_refresh_after_external_edit_without_explicit_open() {
    use_fake_rust_server();

    let project = CargoProject::new("workspace_external_refresh").expect("Failed to create project");
    project.add_file("src/main.rs", "fn main() { let ok = 1; }\n").expect("Failed to add source file");

    let harness = DaemonHarness::spawn(project.root(), LanguageId::Rust).await.expect("Failed to spawn daemon");
    let client = harness.connect().await.expect("Failed to connect client");
    let main_rs = project.root().join("src/main.rs");

    let initial =
        poll_workspace_diagnostics(&client, |diagnostics| diagnostics.len() == 1, Duration::from_secs(10)).await;
    assert_eq!(workspace_error_count(&initial), 0);

    std::fs::write(&main_rs, "fn main() { let error = 1; }\n").expect("Failed to write file");

    let refreshed = poll_workspace_diagnostics(
        &client,
        |diagnostics| workspace_error_count(diagnostics) > 0,
        Duration::from_secs(10),
    )
    .await;
    assert_eq!(refreshed.len(), 1);
    assert_eq!(workspace_error_count(&refreshed), 1);

    harness.kill().await.expect("Failed to kill daemon");
}
