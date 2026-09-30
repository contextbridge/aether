pub mod cargo_project;
pub mod daemon_harness;

pub use cargo_project::{CargoProject, TestProject};
pub use daemon_harness::DaemonHarness;

use aether_lspd::LanguageId;
use aether_lspd::LspClient;
use aether_lspd::testing::configure_fake_server;
use lsp_types::Hover;
use lsp_types::PublishDiagnosticsParams;
use std::path::PathBuf;
use std::sync::Once;
use std::time::{Duration, Instant};

#[allow(dead_code)]
static FAKE_SERVER_ENV: Once = Once::new();

#[allow(dead_code)]
static TS_FALLBACK_ENV: Once = Once::new();

#[allow(dead_code)]
pub fn use_fake_rust_server() {
    use_fake_rust_server_with_args(&[]);
}

/// Point the daemon at the fake Python LSP server for this test binary.
///
/// The configuration is process-wide and applied exactly once: the first
/// caller's `extra_args` win and later calls with different args are silently
/// ignored. Don't mix differently-configured fake servers in one test binary.
#[allow(dead_code)]
pub fn use_fake_rust_server_with_args(extra_args: &[&str]) {
    FAKE_SERVER_ENV.call_once(|| unsafe {
        configure_fake_server(LanguageId::Rust, extra_args);
    });
}

/// Point the TypeScript-native slot at a fake server that dies during
/// `initialize` and the legacy `typescript-language-server` slot at a healthy
/// fake, so tests can exercise the spawn-time fallback between the two.
#[allow(dead_code)]
pub fn use_crashing_native_ts_server() {
    TS_FALLBACK_ENV.call_once(|| unsafe {
        let script = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/common/fake_lsp_server.py");
        let script = script.to_string_lossy().into_owned();
        let crashing = serde_json::to_string(&[script.as_str(), "--crash-on", "initialize"]).unwrap();
        let healthy = serde_json::to_string(&[script.as_str()]).unwrap();
        std::env::set_var("AETHER_LSPD_SERVER_COMMAND_TYPESCRIPT_NATIVE", "python3");
        std::env::set_var("AETHER_LSPD_SERVER_ARGS_TYPESCRIPT_NATIVE", crashing);
        std::env::set_var("AETHER_LSPD_SERVER_COMMAND_TYPESCRIPT_LANGUAGE_SERVER", "python3");
        std::env::set_var("AETHER_LSPD_SERVER_ARGS_TYPESCRIPT_LANGUAGE_SERVER", healthy);
    });
}

#[allow(dead_code)]
pub fn hover_text(hover: Option<Hover>) -> String {
    let hover = hover.expect("Expected hover result");
    match hover.contents {
        lsp_types::HoverContents::Scalar(scalar) => match scalar {
            lsp_types::MarkedString::String(text) => text,
            lsp_types::MarkedString::LanguageString(value) => value.value,
        },
        lsp_types::HoverContents::Array(values) => values
            .into_iter()
            .map(|value| match value {
                lsp_types::MarkedString::String(text) => text,
                lsp_types::MarkedString::LanguageString(value) => value.value,
            })
            .collect::<Vec<_>>()
            .join("\n"),
        lsp_types::HoverContents::Markup(markup) => markup.value,
    }
}

/// Poll diagnostics until `predicate` holds, then return the satisfying result.
#[allow(dead_code)]
pub async fn poll_diagnostics(
    client: &LspClient,
    uri: Option<lsp_types::Uri>,
    predicate: impl Fn(&[PublishDiagnosticsParams]) -> bool,
    timeout: Duration,
) -> Vec<PublishDiagnosticsParams> {
    let start = Instant::now();
    let mut last = Vec::new();

    while start.elapsed() < timeout {
        let diagnostics = client.get_diagnostics(uri.clone()).await.expect("Failed to get diagnostics");
        if predicate(&diagnostics) {
            return diagnostics;
        }
        last = diagnostics;
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    panic!("diagnostics timed out after {timeout:?}. Last result: {last:?}");
}

#[allow(dead_code)]
pub fn diagnostic_count(diagnostics: &[PublishDiagnosticsParams]) -> usize {
    diagnostics.iter().map(|params| params.diagnostics.len()).sum()
}
