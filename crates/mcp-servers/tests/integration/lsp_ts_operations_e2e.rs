use crate::common::{coding_with_lsp, connect, poll_lsp_tool};
use aether_lspd::testing::node_project;
use mcp_utils::client::ClientOptions;

#[tokio::test]
async fn test_ts_hover_returns_type_info() {
    let project = node_project("ts_hover_test");
    project.write("src/index.ts", "const x: number = 42;\nconsole.log(x);\n");

    let index_ts = project.path_string("src/index.ts");
    let client = connect(coding_with_lsp(&project), ClientOptions::default()).await;

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
async fn test_ts_goto_definition() {
    let project = node_project("ts_def_test");
    project.write(
        "src/index.ts",
        r#"function greet(): string {
    return "hello";
}

const msg = greet();
console.log(msg);
"#,
    );

    let client = connect(coding_with_lsp(&project), ClientOptions::default()).await;

    let result = poll_lsp_tool(
        &client,
        "lsp_symbol",
        serde_json::json!({
            "operation": "definition",
            "file_path": "src/index.ts",
            "symbol": "greet",
            "line": 5
        }),
        |r| r["locations"].as_str().is_some_and(|locations| !locations.is_empty()),
    )
    .await;

    assert_eq!(result["locations"], "src/index.ts: 1", "Expected definition at line 1 (1-indexed)");
}

#[tokio::test]
async fn test_ts_find_references() {
    let project = node_project("ts_refs_test");
    project.write(
        "src/index.ts",
        r#"function greet(): string {
    return "hello";
}

const a = greet();
const b = greet();
console.log(a, b);
"#,
    );

    let index_ts = project.path_string("src/index.ts");
    let client = connect(coding_with_lsp(&project), ClientOptions::default()).await;

    let result = poll_lsp_tool(
        &client,
        "lsp_symbol",
        serde_json::json!({
            "operation": "references",
            "file_path": index_ts,
            "symbol": "greet",
            "line": 1
        }),
        |r| r["totalCount"].as_u64().is_some_and(|count| count >= 2),
    )
    .await;

    let locations = result["locations"].as_str().unwrap();
    assert!(locations.starts_with("src/index.ts: ") && locations.ends_with("5, 6"), "{locations}");
}
