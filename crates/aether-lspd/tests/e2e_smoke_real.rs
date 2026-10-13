mod common;

use aether_lspd::LanguageId;
use common::{DaemonHarness, cargo_project, file_uri, hover_text};

#[tokio::test]
#[ignore = "requires rust-analyzer on PATH"]
async fn rust_analyzer_smoke_hover() {
    let project = cargo_project("smoke_ra");
    let content = "fn main() { let value = 1; }\n";
    project.write("src/main.rs", content);

    let harness = DaemonHarness::spawn(project.root(), LanguageId::Rust).await.expect("Failed to spawn daemon");
    let client = harness.connect().await.expect("Failed to connect client");
    let uri = file_uri(&project.path("src/main.rs"));

    let hover = hover_text(client.hover(uri, 0, 0).await.expect("Hover failed"));
    assert_ne!(hover, "");

    harness.kill().await.expect("Failed to kill daemon");
}
