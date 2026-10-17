use crate::common::{CodingWorkspace, McpClientExt, TestResult, call_tool_error};
use crate::{fake_daemon, fake_lsp};
use aether_lspd::LanguageId;
use aether_lspd::testing::{TestWorkspace, cargo_project};
use serde_json::{Value, json};
use std::time::Duration;

const MAIN_RS: &str = r#"fn example_fn() -> i32 {
    7
}

fn main() {
    let value = example_fn();
    println!("{}", value);
}
"#;

#[tokio::test]
async fn hover_finds_the_symbol_when_the_line_hint_is_stale() -> TestResult {
    let project = rust_main().await;

    let result = project.client.call("lsp_symbol", lookup("hover", "src/main.rs", "value", Some(1))).await?;

    let hover = result["hoverContents"].as_str().unwrap_or_default();
    assert!(hover.contains("line=5; character=8"), "{result}");
    Ok(())
}

#[tokio::test]
async fn definition_without_a_line_hint_resolves_through_document_symbols() -> TestResult {
    let project = rust_main().await;

    let result = project.client.call("lsp_symbol", lookup("definition", "src/main.rs", "example_fn", None)).await?;

    assert_eq!(result["locations"], "src/main.rs: 1");
    Ok(())
}

#[tokio::test]
async fn references_are_listed_by_file_and_line() -> TestResult {
    let project = rust_main().await;

    let result = project.client.call("lsp_symbol", lookup("references", "src/main.rs", "example_fn", Some(1))).await?;

    assert_eq!(result["totalCount"], 2);
    assert_eq!(result["locations"], "src/main.rs: 1, 6");
    Ok(())
}

#[tokio::test]
async fn rename_applies_the_server_edits() -> TestResult {
    let project = rust_main().await;

    let rename = json!({ "file_path": "src/main.rs", "symbol": "example_fn", "new_name": "renamed_fn", "line": 1 });
    let result = project.client.call("lsp_rename", rename).await?;

    assert_eq!(result["success"], true);
    assert_eq!((result["filesAffected"].as_u64(), result["totalEdits"].as_u64()), (Some(1), Some(2)));
    assert_eq!(result["changes"], "src/main.rs: 1, 6");
    assert_eq!(project.read("src/main.rs")?, MAIN_RS.replace("example_fn", "renamed_fn"));
    Ok(())
}

#[tokio::test]
async fn symbol_lookup_reports_language_server_errors() {
    let workspace = TestWorkspace::new().file("lib.go", "func example_fn() {}\n");
    let project = fake_lsp(workspace, LanguageId::Go).await;

    let error =
        call_tool_error(&project.client, "lsp_symbol", lookup("definition", "lib.go", "example_fn", Some(1))).await;

    assert!(error.contains("fake server rejected request"), "{error}");
}

#[tokio::test]
async fn symbol_lookup_recovers_after_the_language_server_times_out() -> TestResult {
    let workspace = TestWorkspace::new().file("example.py", "def example_fn():\n    pass\n");
    let daemon = fake_daemon(&workspace, LanguageId::Python).request_timeout(Duration::from_secs(1));
    let project = CodingWorkspace::with_daemon(workspace, daemon).await;

    let definition = lookup("definition", "example.py", "example_fn", Some(1));
    let error = call_tool_error(&project.client, "lsp_symbol", definition).await;
    assert!(error.contains("timed out after 1s"), "{error}");

    let result = project.client.call("lsp_symbol", lookup("hover", "example.py", "example_fn", Some(1))).await?;
    let hover = result["hoverContents"].as_str().unwrap_or_default();
    assert!(hover.contains("def example_fn"), "a fresh server should answer: {result}");
    Ok(())
}

async fn rust_main() -> CodingWorkspace {
    fake_lsp(cargo_project("symbols").file("src/main.rs", MAIN_RS), LanguageId::Rust).await
}

fn lookup(operation: &str, file_path: &str, symbol: &str, line: Option<u32>) -> Value {
    json!({ "operation": operation, "file_path": file_path, "symbol": symbol, "line": line })
}
