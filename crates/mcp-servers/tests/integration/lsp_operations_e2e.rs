//! End-to-end tests for LSP operations (hover, definition, references, rename)
//! through the MCP tool layer, using Rust projects with rust-analyzer.
//!

use crate::common::{CodingWorkspace, McpClientExt, TestResult};
use aether_lspd::testing::cargo_project;
use serde_json::json;

#[tokio::test]
async fn test_hover_falls_back_from_stale_line_hint() {
    let project = rust_main(
        "hover_test",
        r#"fn main() {
    let x: Vec<i32> = vec![1, 2, 3];
    println!("{:?}", x);
}
"#,
    )
    .await;

    let result = project
        .poll_symbol("src/main.rs", json!({ "operation": "hover", "symbol": "x", "line": 1 }), |r| {
            r["hoverContents"].as_str().is_some_and(|hover| hover.contains("Vec"))
        })
        .await;

    let hover = result["hoverContents"].as_str().unwrap();
    assert!(hover.contains("Vec") && hover.contains("i32"), "Expected hover to contain Vec<i32>, got: {hover}");
}

#[tokio::test]
async fn test_goto_definition() {
    let project = rust_main(
        "definition_test",
        r#"fn greet() -> &'static str {
    "hello"
}

fn main() {
    let msg = greet();
    println!("{}", msg);
}
"#,
    )
    .await;

    let result = project
        .poll_symbol("src/main.rs", json!({ "operation": "definition", "symbol": "greet", "line": 6 }), |r| {
            r["locations"].as_str().is_some_and(|locations| !locations.is_empty())
        })
        .await;

    assert_eq!(result["locations"], "src/main.rs: 1", "Expected definition at line 1 (1-indexed)");
}

#[tokio::test]
async fn test_find_references() {
    let project = rust_main(
        "references_test",
        r#"fn greet() -> &'static str {
    "hello"
}

fn main() {
    let a = greet();
    let b = greet();
    println!("{} {}", a, b);
}
"#,
    )
    .await;

    let result = project
        .poll_symbol("src/main.rs", json!({ "operation": "references", "symbol": "greet", "line": 1 }), |r| {
            r["totalCount"].as_u64().is_some_and(|count| count >= 2)
        })
        .await;

    let locations = result["locations"].as_str().unwrap();
    assert!(
        locations.starts_with("src/main.rs: ") && locations.ends_with("6, 7"),
        "Expected references to greet on lines 6 and 7, got {locations}"
    );
}

#[tokio::test]
async fn test_lsp_rename_falls_back_from_stale_line_hint() -> TestResult {
    let project =
        cargo_project("rename_test").file("src/lib.rs", "pub fn greet() -> &'static str {\n    \"hello\"\n}\n").file(
            "src/main.rs",
            r#"fn main() {
    let a = rename_test::greet();
    let b = rename_test::greet();
    println!("{} {}", a, b);
}
"#,
        );
    let project = CodingWorkspace::with_lsp(project).await;
    let lib_rs = project.path("src/lib.rs");

    project
        .poll_symbol("src/lib.rs", json!({ "operation": "references", "symbol": "greet", "line": 1 }), |r| {
            r["locations"].as_str().is_some_and(|locations| locations.contains("src/main.rs"))
        })
        .await;

    let result = project
        .client
        .call("lsp_rename", json!({ "file_path": lib_rs, "symbol": "greet", "new_name": "say_hello", "line": 2 }))
        .await?;

    assert_eq!(result["success"], true);
    assert_eq!(result["oldName"], "greet");
    assert_eq!(result["newName"], "say_hello");
    let files_affected = result["filesAffected"].as_u64().unwrap();
    let total_edits = result["totalEdits"].as_u64().unwrap();
    assert!(files_affected >= 2, "expected at least 2 files, got {files_affected}");
    assert!(total_edits >= 3, "expected at least 3 edits, got {total_edits}");

    let changes = result["changes"].as_str().unwrap();
    assert!(changes.contains("src/lib.rs: 1"), "expected lib.rs in changes, got {changes}");
    assert!(changes.contains("src/main.rs: 2, 3"), "expected main.rs in changes, got {changes}");

    for file in ["src/lib.rs", "src/main.rs"] {
        let content = project.read(file)?;
        assert!(content.contains("say_hello") && !content.contains("greet"), "expected {file} renamed: {content}");
    }
    Ok(())
}

async fn rust_main(name: &str, main_rs: &str) -> CodingWorkspace {
    CodingWorkspace::with_lsp(cargo_project(name).file("src/main.rs", main_rs)).await
}
