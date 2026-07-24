pub mod cargo_project;
pub mod daemon_harness;

pub use cargo_project::{CargoProject, TestProject};
pub use daemon_harness::DaemonHarness;

use lsp_types::Hover;
use std::path::PathBuf;
use std::sync::Once;

#[allow(dead_code)]
static FAKE_SERVER_ENV: Once = Once::new();

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
    FAKE_SERVER_ENV.call_once(|| {
        let script = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests").join("common").join("fake_lsp_server.py");
        let mut args = vec![script.to_string_lossy().to_string()];
        args.extend(extra_args.iter().map(ToString::to_string));
        unsafe {
            std::env::set_var("AETHER_LSPD_SERVER_COMMAND_RUST_ANALYZER", "python3");
            std::env::set_var("AETHER_LSPD_SERVER_ARGS_RUST_ANALYZER", serde_json::to_string(&args).unwrap());
        }
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
