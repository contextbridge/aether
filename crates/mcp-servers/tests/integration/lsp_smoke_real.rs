use crate::common::{CodingWorkspace, McpClientExt, TestResult, cargo_project, has_errors, node_project};
use clankerdiff_lspd::LanguageId;
use clankerdiff_lspd::testing::TestDaemon;
use mcp_servers::testing::TestWorkspace;
use serde_json::json;

#[tokio::test]
#[ignore = "requires rust-analyzer on PATH"]
async fn rust_analyzer_reports_type_errors_and_hover_types() -> TestResult {
    let main_rs = "fn main() {\n    let x: i32 = \"not an int\";\n    println!(\"{}\", x);\n}\n";
    let project = real_lsp(cargo_project("smoke_ra").file("src/main.rs", main_rs), LanguageId::Rust).await;

    project.poll_workspace_errors(has_errors).await;
    let lookup = json!({ "operation": "hover", "file_path": "src/main.rs", "symbol": "x", "line": 2 });
    let result = project.client.call("lsp_symbol", lookup).await?;

    assert!(result["hoverContents"].as_str().is_some_and(|hover| hover.contains("i32")), "{result}");
    Ok(())
}

#[tokio::test]
#[ignore = "installs TypeScript 7 with npm"]
async fn typescript_reports_type_errors() -> TestResult {
    let index_ts = "const x: number = \"not a number\";\nconsole.log(x);\n";
    let project = real_lsp(node_project("smoke_tsc").file("src/index.ts", index_ts), LanguageId::TypeScript).await;

    let result = project.check_errors("src/index.ts").await?;

    assert!(has_errors(&result), "{result}");
    Ok(())
}

async fn real_lsp(workspace: TestWorkspace, language: LanguageId) -> CodingWorkspace {
    let daemon = TestDaemon::builder(workspace.root(), language);
    CodingWorkspace::with_daemon(workspace, daemon).await
}
