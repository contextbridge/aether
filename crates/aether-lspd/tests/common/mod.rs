#![allow(dead_code, unused_imports)]

pub mod cargo_project;
pub mod daemon_harness;

pub use cargo_project::{CargoProject, TestProject};
pub use daemon_harness::DaemonHarness;

use aether_lspd::testing::configure_fake_server;
use aether_lspd::{LanguageId, LspClient};
use lsp_types::{Hover, PublishDiagnosticsParams};
use std::sync::Once;
use std::time::{Duration, Instant};

static FAKE_SERVERS: Once = Once::new();

pub fn use_fake_servers() {
    FAKE_SERVERS.call_once(|| unsafe {
        configure_fake_server(LanguageId::Rust, &[]);
        configure_fake_server(LanguageId::TypeScript, &["--pull-diagnostics"]);
        configure_fake_server(LanguageId::Go, &["--crash-on", "initialize"]);
        configure_fake_server(LanguageId::C, &["--fail-on", "initialize"]);
    });
}

pub async fn poll_workspace_diagnostics(
    client: &LspClient,
    predicate: impl Fn(&[PublishDiagnosticsParams]) -> bool,
    timeout: Duration,
) -> Vec<PublishDiagnosticsParams> {
    let start = Instant::now();
    let mut last = Vec::new();

    while start.elapsed() < timeout {
        let diagnostics = client.get_diagnostics(None).await.expect("Failed to get workspace diagnostics");
        if predicate(&diagnostics) {
            return diagnostics;
        }
        last = diagnostics;
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    panic!("workspace diagnostics timed out after {timeout:?}. Last result: {last:?}");
}

pub fn workspace_error_count(diagnostics: &[PublishDiagnosticsParams]) -> usize {
    diagnostics.iter().map(|params| params.diagnostics.len()).sum()
}

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
