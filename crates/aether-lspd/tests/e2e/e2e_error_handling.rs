use crate::common::{DaemonHarness, cargo_project, file_uri, use_fake_servers};
use aether_lspd::LanguageId;

#[tokio::test]
async fn daemon_survives_client_drop() {
    use_fake_servers();

    let project = cargo_project("client_drop");
    let harness = DaemonHarness::spawn(project.root(), LanguageId::Rust).await.expect("Failed to spawn daemon");

    {
        let _client = harness.connect().await.expect("Failed to connect client");
    }

    let client = harness.connect().await.expect("Failed to reconnect to daemon");
    let uri = file_uri(&project.path("src/main.rs"));
    let hover = client.hover(uri, 0, 0).await.expect("Hover failed");
    assert!(hover.is_some());

    harness.kill().await.expect("Failed to kill daemon");
}
