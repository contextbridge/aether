use crate::common::{CodingWorkspace, poll_lsp_tool};
use aether_lspd::testing::node_project;
use serde_json::json;

#[tokio::test]
async fn test_ts_hover_returns_type_info() {
    let project = ts_index("ts_hover_test", "const x: number = 42;\nconsole.log(x);\n").await;

    let result = project
        .poll_symbol("src/index.ts", json!({ "operation": "hover", "symbol": "x", "line": 1 }), |r| {
            r["hoverContents"].as_str().is_some_and(|hover| !hover.is_empty())
        })
        .await;

    let hover = result["hoverContents"].as_str().unwrap();
    assert!(hover.contains("number"), "Expected hover to contain 'number', got: {hover}");
}

#[tokio::test]
async fn test_ts_goto_definition() {
    let project = ts_index(
        "ts_def_test",
        r#"function greet(): string {
    return "hello";
}

const msg = greet();
console.log(msg);
"#,
    )
    .await;

    let lookup = json!({ "operation": "definition", "file_path": "src/index.ts", "symbol": "greet", "line": 5 });
    let result = poll_lsp_tool(&project.client, "lsp_symbol", lookup, |r| {
        r["locations"].as_str().is_some_and(|locations| !locations.is_empty())
    })
    .await;

    assert_eq!(result["locations"], "src/index.ts: 1", "Expected definition at line 1 (1-indexed)");
}

#[tokio::test]
async fn test_ts_find_references() {
    let project = ts_index(
        "ts_refs_test",
        r#"function greet(): string {
    return "hello";
}

const a = greet();
const b = greet();
console.log(a, b);
"#,
    )
    .await;

    let result = project
        .poll_symbol("src/index.ts", json!({ "operation": "references", "symbol": "greet", "line": 1 }), |r| {
            r["totalCount"].as_u64().is_some_and(|count| count >= 2)
        })
        .await;

    let locations = result["locations"].as_str().unwrap();
    assert!(locations.starts_with("src/index.ts: ") && locations.ends_with("5, 6"), "{locations}");
}

async fn ts_index(name: &str, index_ts: &str) -> CodingWorkspace {
    CodingWorkspace::with_lsp(node_project(name).file("src/index.ts", index_ts)).await
}
