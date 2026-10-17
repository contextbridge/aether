#[path = "../integration/common/mod.rs"]
mod common;

mod diagnostics;
mod symbols;

use aether_lspd::LanguageId;
use aether_lspd::testing::{TestDaemon, TestDaemonBuilder, TestWorkspace, configure_fake_server};
use common::CodingWorkspace;
use std::sync::Once;
use std::time::Duration;

static FAKE_SERVERS: Once = Once::new();

pub async fn fake_lsp(workspace: TestWorkspace, language: LanguageId) -> CodingWorkspace {
    let daemon = fake_daemon(&workspace, language);
    CodingWorkspace::with_daemon(workspace, daemon).await
}

pub fn fake_daemon(workspace: &TestWorkspace, language: LanguageId) -> TestDaemonBuilder {
    FAKE_SERVERS.call_once(|| unsafe {
        configure_fake_server(LanguageId::Rust, &[]);
        configure_fake_server(LanguageId::Go, &["--fail-on", "textDocument/definition"]);
        configure_fake_server(LanguageId::Python, &["--wedge-on", "textDocument/definition"]);
    });
    TestDaemon::builder(workspace.root(), language).diagnostics_settle(Duration::ZERO)
}
