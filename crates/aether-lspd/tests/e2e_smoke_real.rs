mod common;

use aether_lspd::LanguageId;
use common::{CargoProject, DaemonHarness, TestProject, hover_text};

#[tokio::test]
#[ignore = "requires rust-analyzer on PATH"]
async fn rust_analyzer_smoke_hover() {
    let project = CargoProject::new("smoke_ra").expect("Failed to create project");
    let content = "fn main() { let value = 1; }\n";
    project.add_file("src/main.rs", content).expect("Failed to add source file");

    let harness = DaemonHarness::spawn(project.root(), LanguageId::Rust).await.expect("Failed to spawn daemon");
    let client = harness.connect().await.expect("Failed to connect client");
    let uri = project.file_uri("src/main.rs");

    let hover = hover_text(client.hover(uri, 0, 0).await.expect("Hover failed"));
    assert!(!hover.is_empty());

    harness.kill().await.expect("Failed to kill daemon");
}
