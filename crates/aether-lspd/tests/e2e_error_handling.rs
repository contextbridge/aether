mod common;

use aether_lspd::LanguageId;
use common::{CargoProject, DaemonHarness, TestProject, use_fake_rust_server};

#[tokio::test]
async fn daemon_survives_client_drop() {
    use_fake_rust_server();

    let project = CargoProject::new("client_drop").expect("Failed to create project");
    let harness = DaemonHarness::spawn(project.root(), LanguageId::Rust).await.expect("Failed to spawn daemon");

    {
        let _client = harness.connect().await.expect("Failed to connect client");
    }

    let client = harness.connect().await.expect("Failed to reconnect to daemon");
    let uri = project.file_uri("src/main.rs");
    let hover = client.hover(uri, 0, 0).await.expect("Hover failed");
    assert!(hover.is_some());

    harness.kill().await.expect("Failed to kill daemon");
}
