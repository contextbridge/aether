use crate::common::{CargoProject, DaemonHarness, TestProject, hover_text, use_fake_rust_server};
use aether_lspd::LanguageId;
use lsp_types::PublishDiagnosticsParams;
use std::time::{Duration, Instant};

async fn poll_file_diagnostics(
    client: &aether_lspd::LspClient,
    uri: lsp_types::Uri,
    predicate: impl Fn(&[PublishDiagnosticsParams]) -> bool,
    timeout: Duration,
) -> Vec<PublishDiagnosticsParams> {
    let start = Instant::now();
    let mut last = Vec::new();
    while start.elapsed() < timeout {
        let diagnostics = client.get_diagnostics(Some(uri.clone())).await.expect("Failed to get diagnostics");
        if predicate(&diagnostics) {
            return diagnostics;
        }
        last = diagnostics;
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("file diagnostics timed out after {timeout:?}. Last result: {last:?}");
}

fn error_count(diagnostics: &[PublishDiagnosticsParams]) -> usize {
    diagnostics.iter().map(|params| params.diagnostics.len()).sum()
}

#[tokio::test]
async fn pull_diagnostics_return_without_waiting_for_push() {
    use_fake_rust_server();

    let project = CargoProject::new("pull_diagnostics").expect("Failed to create project");
    project.add_file("src/main.rs", "fn main() { let error = 1; }\n").expect("Failed to add source file");

    let harness = DaemonHarness::spawn(project.root(), LanguageId::Rust).await.expect("Failed to spawn daemon");
    let client = harness.connect().await.expect("Failed to connect client");
    let uri = project.file_uri("src/main.rs");

    let diagnostics =
        poll_file_diagnostics(&client, uri, |diagnostics| error_count(diagnostics) > 0, Duration::from_secs(10)).await;
    assert_eq!(error_count(&diagnostics), 1);

    harness.kill().await.expect("Failed to kill daemon");
}

#[tokio::test]
async fn pull_empty_clears_pushed_errors() {
    use_fake_rust_server();

    let project = CargoProject::new("pull_clears_errors").expect("Failed to create project");
    let path = project.add_file("src/main.rs", "fn main() { let error = 1; }\n").expect("Failed to add source file");

    let harness = DaemonHarness::spawn(project.root(), LanguageId::Rust).await.expect("Failed to spawn daemon");
    let client = harness.connect().await.expect("Failed to connect client");
    let uri = project.file_uri("src/main.rs");

    poll_file_diagnostics(&client, uri.clone(), |diagnostics| error_count(diagnostics) > 0, Duration::from_secs(10))
        .await;

    std::fs::write(&path, "fn main() { let ok = 1; }\n").expect("Failed to fix file");

    poll_file_diagnostics(&client, uri, |diagnostics| error_count(diagnostics) == 0, Duration::from_secs(10)).await;

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
    assert_eq!(error_count(&diagnostics), 2);

    let hover = hover_text(client.hover(project.file_uri("src/pullless_a.rs"), 0, 0).await.expect("Hover failed"));
    assert!(hover.contains("rejections=1"), "expected a single rejected pull probe, got: {hover}");

    harness.kill().await.expect("Failed to kill daemon");
}
