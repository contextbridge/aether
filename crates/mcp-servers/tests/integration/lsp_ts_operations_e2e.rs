//! End-to-end tests for LSP operations (hover, definition, references, document symbols)
//! through the MCP tool layer, using TypeScript projects with typescript-language-server.
//!
//! Requirements:
//! - `npm` must be installed (the test project installs pinned TypeScript tooling locally)
//! - `aether-lspd` binary must be built (`cargo build -p aether-lspd`)
//!
//! Run with: `cargo test -p mcp-servers -- lsp_ts_operations`

use crate::common::{call_tool, connect_lsp, poll_lsp_tool};
use aether_lspd::testing::{NodeProject, TestProject};

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

#[tokio::test]
async fn test_ts_workspace_search_requires_language_and_finds_exported_symbol() {
    let project = NodeProject::new("ts_workspace_search_test").expect("Failed to create project");
    project
        .add_file("src/safety.ts", "export function isSafeToAutoMerge(): boolean { return true; }\n")
        .expect("Failed to add safety.ts");
    project
        .add_file("src/index.ts", "import { isSafeToAutoMerge } from './safety.js';\nvoid isSafeToAutoMerge();\n")
        .expect("Failed to add index.ts");

    let index_ts = project.file_path_str("src/index.ts");
    let (_server_handle, client) = connect_lsp(&project).await;
    poll_lsp_tool(&client, "lsp_document", serde_json::json!({ "file_path": index_ts }), |result| {
        result.get("symbols").and_then(|symbols| symbols.as_array()).is_some()
    })
    .await;

    let result = poll_lsp_tool(
        &client,
        "lsp_workspace_search",
        serde_json::json!({ "query": "isSafeToAutoMerge", "language": "typescript" }),
        |result| {
            result["results"]
                .as_array()
                .is_some_and(|results| results.iter().any(|entry| entry["name"] == "isSafeToAutoMerge"))
        },
    )
    .await;

    assert!(result["results"].as_array().unwrap().iter().any(|entry| entry["name"] == "isSafeToAutoMerge"));
}

#[tokio::test]
async fn test_ts_outgoing_calls_use_caller_paths_and_deduplicate_ranges() {
    let project = NodeProject::new("ts_outgoing_calls_test").expect("Failed to create project");
    project
        .add_file(
            "node_modules/.pnpm/example@1.0.0/node_modules/example/index.d.ts",
            "export declare function external(): void;\n",
        )
        .expect("Failed to add dependency declaration");
    project
        .add_file(
            "src/index.ts",
            r#"import { external } from "../node_modules/.pnpm/example@1.0.0/node_modules/example/index.js";
const logger = { info(_message: string): void {} };
function helper(): void {}
function run(): void {
    logger.info("first");
    logger.info("second");
    helper();
    external();
}
run();
"#,
        )
        .expect("Failed to add index.ts");

    let index_ts = project.file_path_str("src/index.ts");
    let (_server_handle, client) = connect_lsp(&project).await;
    let result = poll_lsp_tool(
        &client,
        "lsp_symbol",
        serde_json::json!({
            "operation": "outgoingCalls",
            "file_path": index_ts,
            "symbol": "run",
            "line": 4,
            "callScope": "all"
        }),
        |result| result["callSites"].as_array().is_some_and(|calls| !calls.is_empty()),
    )
    .await;

    let calls = result["callSites"].as_array().unwrap();
    let mut ranges = std::collections::HashSet::new();
    for call in calls {
        for site in call["callSites"].as_array().unwrap() {
            assert_eq!(site["filePath"], index_ts);
            let key = (
                site["filePath"].as_str().unwrap(),
                site["startLine"].as_u64().unwrap(),
                site["startColumn"].as_u64().unwrap(),
                site["endLine"].as_u64().unwrap(),
                site["endColumn"].as_u64().unwrap(),
            );
            assert!(ranges.insert(key), "duplicate call-site range: {site}");
        }
    }
    assert!(calls.iter().all(|call| {
        call["callSites"].as_array().is_some_and(|sites| sites.iter().all(|site| site.get("context").is_none()))
    }));
    assert!(calls.iter().any(|call| call["item"]["name"] == "helper"));
    let external = calls.iter().find(|call| call["item"]["name"] == "external").expect("external dependency call");
    assert_eq!(external["projectLocal"], false);
    assert!(
        external["item"]["displayPath"]
            .as_str()
            .is_some_and(|path| { path.starts_with("example/") && !path.contains(".pnpm") })
    );

    let project_calls = call_tool(
        &client,
        "lsp_symbol",
        serde_json::json!({
            "operation": "outgoingCalls",
            "file_path": index_ts,
            "symbol": "run",
            "line": 4,
            "callScope": "project"
        }),
    )
    .await;
    assert!(!project_calls["callSites"].as_array().unwrap().iter().any(|call| { call["item"]["name"] == "external" }));

    let context_calls = call_tool(
        &client,
        "lsp_symbol",
        serde_json::json!({
            "operation": "outgoingCalls",
            "file_path": index_ts,
            "symbol": "run",
            "line": 4,
            "callScope": "all",
            "contextLines": 1
        }),
    )
    .await;
    assert!(context_calls["callSites"].as_array().unwrap().iter().any(|call| {
        call["callSites"].as_array().is_some_and(|sites| sites.iter().any(|site| site.get("context").is_some()))
    }));
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
