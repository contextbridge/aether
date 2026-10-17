#![allow(dead_code, unused_imports)]

pub(crate) use aether_lspd::testing::{TestWorkspace, cargo_project, file_uri};

use aether_lspd::testing::{TestDaemon, configure_fake_server};
use aether_lspd::{LanguageId, LspClient};
use lsp_types::{Hover, HoverContents, MarkedString, PublishDiagnosticsParams};
use serde_json::{Value, json};
use std::path::Path;
use std::sync::Once;
use std::time::Duration;

static FAKE_SERVERS: Once = Once::new();

pub(crate) fn use_fake_servers() {
    FAKE_SERVERS.call_once(|| unsafe {
        configure_fake_server(LanguageId::Rust, &[]);
        configure_fake_server(LanguageId::TypeScript, &["--pull-diagnostics"]);
        configure_fake_server(LanguageId::Go, &["--crash-on", "initialize"]);
        configure_fake_server(LanguageId::C, &["--fail-on", "initialize"]);
        configure_fake_server(LanguageId::Python, &["--garble-before", "textDocument/hover"]);
    });
}

/// A daemon serving the fake server, which publishes once per open, so diagnostics need no settle window.
pub(crate) async fn fake_daemon(root: &Path, language: LanguageId) -> (TestDaemon, LspClient) {
    use_fake_servers();
    let daemon =
        TestDaemon::builder(root, language).diagnostics_settle(Duration::ZERO).spawn().expect("Failed to spawn daemon");
    (daemon, connect(root, language).await)
}

pub(crate) async fn start_daemon(root: &Path, language: LanguageId) -> (TestDaemon, LspClient) {
    let daemon = TestDaemon::spawn(root, language).expect("Failed to spawn daemon");
    (daemon, connect(root, language).await)
}

pub(crate) async fn connect(root: &Path, language: LanguageId) -> LspClient {
    LspClient::connect(root, language).await.expect("Failed to connect client")
}

/// Sends a request only the fake server understands, such as `fake/finishLoading`.
pub(crate) async fn fake_request(client: &LspClient, method: &str) {
    client.call(method, &json!({}), || Value::Null).await.unwrap_or_else(|error| panic!("{method} failed: {error}"));
}

pub(crate) async fn wait_for_workspace_diagnostics(
    client: &LspClient,
    predicate: impl Fn(&[PublishDiagnosticsParams]) -> bool,
) -> Vec<PublishDiagnosticsParams> {
    loop {
        let diagnostics = client.get_diagnostics(None).await.expect("Failed to get workspace diagnostics");
        if predicate(&diagnostics) {
            return diagnostics;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

pub(crate) fn workspace_error_count(diagnostics: &[PublishDiagnosticsParams]) -> usize {
    diagnostics.iter().map(|params| params.diagnostics.len()).sum()
}

pub(crate) fn hover_text(hover: Option<Hover>) -> String {
    let marked = |value: MarkedString| match value {
        MarkedString::String(text) => text,
        MarkedString::LanguageString(value) => value.value,
    };
    match hover.expect("Expected hover result").contents {
        HoverContents::Scalar(value) => marked(value),
        HoverContents::Array(values) => values.into_iter().map(marked).collect::<Vec<_>>().join("\n"),
        HoverContents::Markup(markup) => markup.value,
    }
}
