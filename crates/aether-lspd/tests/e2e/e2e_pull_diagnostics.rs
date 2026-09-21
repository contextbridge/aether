use crate::common::{CargoProject, DaemonHarness, TestProject, diagnostic_count, hover_text, use_fake_rust_server};
use aether_lspd::LanguageId;

#[tokio::test]
async fn pull_serves_diagnostics_when_the_server_never_pushes() {
    use_fake_rust_server();

    let project = CargoProject::new("pull_only").expect("Failed to create project");
    project.add_file("src/pushless_main.rs", "fn main() { let error = 1; }\n").expect("Failed to add source file");

    let harness = DaemonHarness::spawn(project.root(), LanguageId::Rust).await.expect("Failed to spawn daemon");
    let client = harness.connect().await.expect("Failed to connect client");
    let uri = project.file_uri("src/pushless_main.rs");

    let diagnostics = client.get_diagnostics(Some(uri)).await.expect("Failed to get diagnostics");

    assert_eq!(diagnostic_count(&diagnostics), 1);
    assert_eq!(diagnostics[0].diagnostics[0].message, "error token");

    harness.kill().await.expect("Failed to kill daemon");
}

#[tokio::test]
async fn pull_empty_report_clears_errors() {
    use_fake_rust_server();

    let project = CargoProject::new("pull_clears").expect("Failed to create project");
    let path =
        project.add_file("src/pushless_main.rs", "fn main() { let error = 1; }\n").expect("Failed to add source file");

    let harness = DaemonHarness::spawn(project.root(), LanguageId::Rust).await.expect("Failed to spawn daemon");
    let client = harness.connect().await.expect("Failed to connect client");
    let uri = project.file_uri("src/pushless_main.rs");

    let initial = client.get_diagnostics(Some(uri.clone())).await.expect("Failed to get diagnostics");
    assert_eq!(diagnostic_count(&initial), 1);

    std::fs::write(&path, "fn main() { let ok = 1; }\n").expect("Failed to fix file");

    let fixed = client.get_diagnostics(Some(uri)).await.expect("Failed to get diagnostics");
    assert_eq!(diagnostic_count(&fixed), 0);

    harness.kill().await.expect("Failed to kill daemon");
}

#[tokio::test]
async fn pull_is_probed_once_when_the_server_rejects_it() {
    use_fake_rust_server();

    let project = CargoProject::new("pull_rejected").expect("Failed to create project");
    project.add_file("src/pullless_a.rs", "fn main() { let error = 1; }\n").expect("Failed to add source file");
    project.add_file("src/pullless_b.rs", "fn main() { let error = 1; }\n").expect("Failed to add source file");

    let harness = DaemonHarness::spawn(project.root(), LanguageId::Rust).await.expect("Failed to spawn daemon");
    let client = harness.connect().await.expect("Failed to connect client");

    let diagnostics = client.get_diagnostics(None).await.expect("Failed to get workspace diagnostics");
    assert_eq!(diagnostic_count(&diagnostics), 2);

    let hover = hover_text(client.hover(project.file_uri("src/pullless_a.rs"), 0, 0).await.expect("Hover failed"));
    assert!(hover.contains("rejections=1"), "expected a single rejected pull probe, got: {hover}");

    harness.kill().await.expect("Failed to kill daemon");
}
