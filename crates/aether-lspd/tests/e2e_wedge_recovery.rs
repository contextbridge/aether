//! Recovery tests for unresponsive or crashed language servers.
//!
//! A language server that stops responding (or exits) must not leave a zombie
//! session behind: in-flight requests must fail with a bounded error, and the
//! next request must be served by a freshly spawned server.

mod common;

use aether_lspd::{ClientError, LSP_REQUEST_TIMED_OUT, LanguageId};
use common::{CargoProject, DaemonHarness, TestProject, hover_text, use_fake_rust_server_with_args};
use std::sync::Once;

static SETUP: Once = Once::new();

/// Fake server that never answers `workspace/symbol` and exits on
/// `textDocument/references`, with a short daemon request timeout so wedge
/// detection fires quickly.
fn use_misbehaving_fake_server() {
    SETUP.call_once(|| {
        unsafe { std::env::set_var("AETHER_LSPD_REQUEST_TIMEOUT", "2") };
        use_fake_rust_server_with_args(&["--wedge-on", "workspace/symbol", "--crash-on", "textDocument/references"]);
    });
}

#[tokio::test]
async fn wedged_request_fails_with_timeout_instead_of_hanging() {
    use_misbehaving_fake_server();

    let project = CargoProject::new("wedge_timeout").expect("Failed to create project");
    let harness = DaemonHarness::spawn(project.root(), LanguageId::Rust).await.expect("Failed to spawn daemon");
    let client = harness.connect().await.expect("Failed to connect");

    let result = client.workspace_symbol("example".to_string()).await;
    match result {
        Err(ClientError::LspError { code, message }) => {
            assert_eq!(code, LSP_REQUEST_TIMED_OUT, "unexpected error: {message}");
        }
        other => panic!("Expected timeout error for wedged request, got {other:?}"),
    }

    harness.kill().await.expect("Failed to kill daemon");
}

#[tokio::test]
async fn wedged_server_is_replaced_on_next_request() {
    use_misbehaving_fake_server();

    let project = CargoProject::new("wedge_recovery").expect("Failed to create project");
    project.add_file("src/main.rs", "fn main() {}\n").expect("Failed to add file");
    let harness = DaemonHarness::spawn(project.root(), LanguageId::Rust).await.expect("Failed to spawn daemon");
    let client = harness.connect().await.expect("Failed to connect");
    let uri = project.file_uri("src/main.rs");

    let wedged = client.workspace_symbol("example".to_string()).await;
    assert!(wedged.is_err(), "wedged request should fail, got {wedged:?}");

    let hover = hover_text(client.hover(uri, 0, 0).await.expect("Hover after wedge should succeed"));
    assert!(hover.contains("open_count=1"), "unexpected hover from replacement server: {hover}");

    harness.kill().await.expect("Failed to kill daemon");
}

#[tokio::test]
async fn crashed_server_is_replaced_on_next_request() {
    use_misbehaving_fake_server();

    let project = CargoProject::new("crash_recovery").expect("Failed to create project");
    project.add_file("src/main.rs", "fn main() {}\n").expect("Failed to add file");
    let harness = DaemonHarness::spawn(project.root(), LanguageId::Rust).await.expect("Failed to spawn daemon");
    let client = harness.connect().await.expect("Failed to connect");
    let uri = project.file_uri("src/main.rs");

    let crashed = client.find_references(uri.clone(), 0, 0, true).await;
    assert!(crashed.is_err(), "request to crashing server should fail, got {crashed:?}");

    let hover = hover_text(client.hover(uri, 0, 0).await.expect("Hover after crash should succeed"));
    assert!(hover.contains("open_count=1"), "unexpected hover from replacement server: {hover}");

    harness.kill().await.expect("Failed to kill daemon");
}
