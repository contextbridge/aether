//! Recovery tests for unresponsive or crashed language servers.
//!
//! A language server that stops responding (or exits) must not leave a zombie
//! session behind: in-flight requests must fail with a bounded error, and the
//! next request must be served by a freshly spawned server.

mod common;

use aether_lspd::testing::{TestDaemon, configure_fake_server};
use aether_lspd::{ClientError, LSP_REQUEST_TIMED_OUT, LanguageId, LspClient};
use common::{cargo_project, connect, file_uri, hover_text};
use std::path::Path;
use std::sync::Once;
use std::time::Duration;

#[tokio::test]
async fn wedged_request_fails_with_timeout_instead_of_hanging() {
    let project = cargo_project("wedge_timeout");
    let (_daemon, client) = misbehaving_daemon(project.root()).await;

    let result = client.goto_definition(file_uri(&project.path("src/main.rs")), 0, 3).await;

    assert!(matches!(result, Err(ClientError::LspError { code: LSP_REQUEST_TIMED_OUT, .. })), "{result:?}");
}

#[tokio::test]
async fn wedged_server_is_replaced_on_next_request() {
    let project = cargo_project("wedge_recovery");
    let (_daemon, client) = misbehaving_daemon(project.root()).await;
    let uri = file_uri(&project.path("src/main.rs"));

    let wedged = client.goto_definition(uri.clone(), 0, 3).await;
    assert!(wedged.is_err(), "wedged request should fail, got {wedged:?}");

    let hover = hover_text(client.hover(uri, 0, 0).await.expect("Hover after wedge should succeed"));
    assert!(hover.contains("open_count=1"), "unexpected hover from replacement server: {hover}");
}

#[tokio::test]
async fn crashed_server_is_replaced_on_next_request() {
    let project = cargo_project("crash_recovery");
    let (_daemon, client) = misbehaving_daemon(project.root()).await;
    let uri = file_uri(&project.path("src/main.rs"));

    let crashed = client.find_references(uri.clone(), 0, 0, true).await;
    assert!(crashed.is_err(), "request to crashing server should fail, got {crashed:?}");

    let hover = hover_text(client.hover(uri, 0, 0).await.expect("Hover after crash should succeed"));
    assert!(hover.contains("open_count=1"), "unexpected hover from replacement server: {hover}");
}

static SETUP: Once = Once::new();

async fn misbehaving_daemon(root: &Path) -> (TestDaemon, LspClient) {
    SETUP.call_once(|| unsafe {
        configure_fake_server(
            LanguageId::Rust,
            &["--wedge-on", "textDocument/definition", "--crash-on", "textDocument/references"],
        );
    });
    let daemon = TestDaemon::builder(root, LanguageId::Rust)
        .request_timeout(Duration::from_secs(2))
        .spawn()
        .expect("Failed to spawn daemon");
    (daemon, connect(root, LanguageId::Rust).await)
}
