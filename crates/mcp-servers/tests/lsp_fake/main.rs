#[path = "../integration/common/mod.rs"]
mod common;

mod diagnostics;
mod symbols;

use clankerdiff_lspd::LanguageId;
use clankerdiff_lspd::testing::{TestDaemon, TestDaemonBuilder};
use common::CodingWorkspace;
use mcp_servers::testing::TestWorkspace;

async fn fake_lsp(workspace: TestWorkspace, language: LanguageId) -> CodingWorkspace {
    let daemon = fake_daemon(&workspace, language);
    CodingWorkspace::with_daemon(workspace, daemon).await
}

fn fake_daemon(workspace: &TestWorkspace, language: LanguageId) -> TestDaemonBuilder {
    let args: &[&str] = match language {
        LanguageId::Go => &["--fail-on", "textDocument/definition"],
        LanguageId::Python => &["--wedge-on", "textDocument/definition"],
        _ => &[],
    };
    TestDaemon::builder(workspace.root(), language).fake_server(args)
}
