//! End-to-end tests for LSP operations (hover, definition, references, document symbols)
//! through the MCP tool layer, using TypeScript projects with typescript-language-server.
//!
//! Requirements:
//! - `typescript-language-server` must be installed and in PATH
//! - `typescript` must be installed (globally or in PATH)
//! - `aether-lspd` binary must be built (`cargo build -p aether-lspd`)
//!
//! Run with: `cargo test -p mcp-servers -- lsp_ts_operations`

mod common;

use aether_lspd::testing::{NodeProject, TestProject};
use common::{connect_lsp, poll_lsp_tool};

/// Test: hover returns type information for a TypeScript variable
#[tokio::test]
async fn test_ts_hover_returns_type_info() {
    let project = NodeProject::new("ts_hover_test").expect("Failed to create project");
    project.add_file("src/index.ts", "const x: number = 42;\nconsole.log(x);\n").expect("Failed to add file");

    let index_ts = project.file_path_str("src/index.ts");
    let (_server_handle, client) = connect_lsp(&project).await;

    let result = poll_lsp_tool(
        &client,
        "lsp_symbol",
        serde_json::json!({
            "operation": "hover",
            "file_path": index_ts,
            "symbol": "x",
            "line": 1
        }),
        |r| r.get("hoverContents").and_then(|h| h.as_str()).is_some_and(|s| !s.is_empty()),
    )
    .await;

    let hover = result["hoverContents"].as_str().unwrap();
    assert!(hover.contains("number"), "Expected hover to contain 'number', got: {hover}");
}

/// Test: goto definition resolves to the correct function definition in TypeScript
#[tokio::test]
async fn test_ts_goto_definition() {
    let project = NodeProject::new("ts_def_test").expect("Failed to create project");
    project
        .add_file(
            "src/index.ts",
            r#"function greet(): string {
    return "hello";
}

const msg = greet();
console.log(msg);
"#,
        )
        .expect("Failed to add file");

    let index_ts = project.file_path_str("src/index.ts");
    let (_server_handle, client) = connect_lsp(&project).await;

    let result = poll_lsp_tool(
        &client,
        "lsp_symbol",
        serde_json::json!({
            "operation": "definition",
            "file_path": index_ts,
            "symbol": "greet",
            "line": 5
        }),
        |r| r.get("locations").and_then(|l| l.as_array()).is_some_and(|a| !a.is_empty()),
    )
    .await;

    let locations = result["locations"].as_array().unwrap();
    assert!(!locations.is_empty(), "Expected at least one definition location");

    let first = &locations[0];
    let start_line = first["startLine"].as_u64().unwrap();
    assert_eq!(start_line, 1, "Expected definition at line 1 (1-indexed)");
}

/// Test: find references returns all usages of a symbol in TypeScript
#[tokio::test]
async fn test_ts_find_references() {
    let project = NodeProject::new("ts_refs_test").expect("Failed to create project");
    project
        .add_file(
            "src/index.ts",
            r#"function greet(): string {
    return "hello";
}

const a = greet();
const b = greet();
console.log(a, b);
"#,
        )
        .expect("Failed to add file");

    let index_ts = project.file_path_str("src/index.ts");
    let (_server_handle, client) = connect_lsp(&project).await;

    let result = poll_lsp_tool(
        &client,
        "lsp_symbol",
        serde_json::json!({
            "operation": "references",
            "file_path": index_ts,
            "symbol": "greet",
            "line": 1
        }),
        |r| r.get("locations").and_then(|l| l.as_array()).is_some_and(|a| a.len() >= 2),
    )
    .await;

    let locations = result["locations"].as_array().unwrap();
    assert!(locations.len() >= 2, "Expected at least 2 references to greet, got {}", locations.len());
}

/// Test: document symbols returns functions and interfaces in TypeScript
#[tokio::test]
async fn test_ts_document_symbols() {
    let project = NodeProject::new("ts_docsym_test").expect("Failed to create project");
    project
        .add_file(
            "src/index.ts",
            r"interface Point {
    x: number;
    y: number;
}

function distance(a: Point, b: Point): number {
    return Math.sqrt((a.x - b.x) ** 2 + (a.y - b.y) ** 2);
}

function main(): void {
    const p1: Point = { x: 0, y: 0 };
    const p2: Point = { x: 3, y: 4 };
    console.log(distance(p1, p2));
}

main();
",
        )
        .expect("Failed to add file");

    let index_ts = project.file_path_str("src/index.ts");
    let (_server_handle, client) = connect_lsp(&project).await;

    let result = poll_lsp_tool(
        &client,
        "lsp_document",
        serde_json::json!({
            "file_path": index_ts
        }),
        |r| r.get("symbols").and_then(|s| s.as_array()).is_some_and(|a| !a.is_empty()),
    )
    .await;

    let symbols = result["symbols"].as_array().unwrap();
    let names: Vec<&str> = symbols.iter().filter_map(|s| s.get("name").and_then(|n| n.as_str())).collect();

    assert!(names.contains(&"Point"), "Expected 'Point' in document symbols, got: {names:?}");
    assert!(names.contains(&"distance"), "Expected 'distance' in document symbols, got: {names:?}");
    assert!(names.contains(&"main"), "Expected 'main' in document symbols, got: {names:?}");
}
