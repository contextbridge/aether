use crate::common::{connect_lsp, poll_lsp_tool};
use aether_lspd::testing::{NodeProject, TestProject};

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

    let (_server_handle, client) = connect_lsp(&project).await;

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
        |r| r["totalCount"].as_u64().is_some_and(|count| count >= 2),
    )
    .await;

    let locations = result["locations"].as_str().unwrap();
    assert!(locations.starts_with("src/index.ts: ") && locations.ends_with("5, 6"), "{locations}");
}
