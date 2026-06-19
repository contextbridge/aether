//! End-to-end tests for LSP operations (hover, definition, references, document symbols)
//! through the MCP tool layer, using Rust projects with rust-analyzer.
//!
//! Requirements:
//! - `rust-analyzer` must be installed and in PATH
//! - `aether-lspd` binary must be built (`cargo build -p aether-lspd`)
//!
//! Run with: `cargo test -p mcp-servers -- lsp_operations`

mod common;

use aether_lspd::testing::{CargoProject, TestProject};
use common::{call_tool, connect_lsp, poll_lsp_tool};

/// Test: hover returns type information for a Rust variable
#[tokio::test]
async fn test_hover_returns_type_info() {
    let project = CargoProject::new("hover_test").expect("Failed to create project");
    project
        .add_file(
            "src/main.rs",
            r#"fn main() {
    let x: Vec<i32> = vec![1, 2, 3];
    println!("{:?}", x);
}
"#,
        )
        .expect("Failed to add file");

    let main_rs = project.file_path_str("src/main.rs");
    let (_server_handle, client) = connect_lsp(&project).await;

    let result = poll_lsp_tool(
        &client,
        "lsp_symbol",
        serde_json::json!({
            "operation": "hover",
            "file_path": main_rs,
            "symbol": "x",
            "line": 2
        }),
        |r| r.get("hoverContents").and_then(|h| h.as_str()).is_some_and(|s| s.contains("Vec")),
    )
    .await;

    let hover = result["hoverContents"].as_str().unwrap();
    assert!(hover.contains("Vec") && hover.contains("i32"), "Expected hover to contain Vec<i32>, got: {hover}");
}

/// Test: goto definition resolves to the correct function definition
#[tokio::test]
async fn test_goto_definition() {
    let project = CargoProject::new("definition_test").expect("Failed to create project");
    project
        .add_file(
            "src/main.rs",
            r#"fn greet() -> &'static str {
    "hello"
}

fn main() {
    let msg = greet();
    println!("{}", msg);
}
"#,
        )
        .expect("Failed to add file");

    let main_rs = project.file_path_str("src/main.rs");
    let (_server_handle, client) = connect_lsp(&project).await;

    let result = poll_lsp_tool(
        &client,
        "lsp_symbol",
        serde_json::json!({
            "operation": "definition",
            "file_path": main_rs,
            "symbol": "greet",
            "line": 6
        }),
        |r| r.get("locations").and_then(|l| l.as_array()).is_some_and(|a| !a.is_empty()),
    )
    .await;

    let locations = result["locations"].as_array().unwrap();
    assert!(!locations.is_empty(), "Expected at least one definition location");

    // LocationResult uses 1-indexed startLine (camelCase)
    let first = &locations[0];
    let start_line = first["startLine"].as_u64().unwrap();
    assert_eq!(start_line, 1, "Expected definition at line 1 (1-indexed)");
}

/// Test: find references returns all usages of a symbol
#[tokio::test]
async fn test_find_references() {
    let project = CargoProject::new("references_test").expect("Failed to create project");
    project
        .add_file(
            "src/main.rs",
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
        .expect("Failed to add file");

    let main_rs = project.file_path_str("src/main.rs");
    let (_server_handle, client) = connect_lsp(&project).await;

    let result = poll_lsp_tool(
        &client,
        "lsp_symbol",
        serde_json::json!({
            "operation": "references",
            "file_path": main_rs,
            "symbol": "greet",
            "line": 1
        }),
        |r| r.get("locations").and_then(|l| l.as_array()).is_some_and(|a| a.len() >= 2),
    )
    .await;

    let locations = result["locations"].as_array().unwrap();
    // At least the 2 call sites (and possibly the definition if include_declaration defaults true)
    assert!(locations.len() >= 2, "Expected at least 2 references to greet, got {}", locations.len());
}

/// Test: `lsp_rename` applies workspace edits for a Rust symbol
#[tokio::test]
async fn test_lsp_rename_applies_workspace_edits() {
    let project = CargoProject::new("rename_test").expect("Failed to create project");
    project
        .add_file(
            "src/lib.rs",
            r#"pub fn greet() -> &'static str {
    "hello"
}
"#,
        )
        .expect("Failed to add lib.rs");
    project
        .add_file(
            "src/main.rs",
            r#"fn main() {
    let a = rename_test::greet();
    let b = rename_test::greet();
    println!("{} {}", a, b);
}
"#,
        )
        .expect("Failed to add main.rs");

    let lib_rs = project.file_path_str("src/lib.rs");
    let (_server_handle, client) = connect_lsp(&project).await;

    // Wait for rust-analyzer to fully index both files before calling rename.
    // Rename is a side-effecting operation that writes to disk, so it must not
    // be polled — a partial rename on the first attempt would corrupt state for
    // subsequent retries. We verify indexing is complete by polling references
    // until rust-analyzer sees usages in main.rs.
    poll_lsp_tool(
        &client,
        "lsp_symbol",
        serde_json::json!({
            "operation": "references",
            "file_path": lib_rs,
            "symbol": "greet",
            "line": 1
        }),
        |r| {
            r.get("locations").and_then(|v| v.as_array()).is_some_and(|refs| {
                refs.iter()
                    .any(|loc| loc.get("filePath").and_then(|p| p.as_str()).is_some_and(|p| p.ends_with("main.rs")))
            })
        },
    )
    .await;

    let result = call_tool(
        &client,
        "lsp_rename",
        serde_json::json!({
            "file_path": lib_rs,
            "symbol": "greet",
            "new_name": "say_hello",
            "line": 1
        }),
    )
    .await;

    assert_eq!(result["success"].as_bool(), Some(true));
    assert_eq!(result["oldName"].as_str(), Some("greet"));
    assert_eq!(result["newName"].as_str(), Some("say_hello"));

    let files_affected = result["filesAffected"].as_u64().unwrap();
    let total_edits = result["totalEdits"].as_u64().unwrap();
    assert!(files_affected >= 2, "expected at least 2 files, got {files_affected}");
    assert!(total_edits >= 3, "expected at least 3 edits, got {total_edits}");

    let changes = result["changes"].as_array().unwrap();
    let changed_paths: Vec<&str> =
        changes.iter().filter_map(|entry| entry.get("filePath").and_then(|v| v.as_str())).collect();

    assert!(
        changed_paths.iter().any(|path| path.ends_with("src/lib.rs")),
        "expected lib.rs in changes, got {changed_paths:?}"
    );
    assert!(
        changed_paths.iter().any(|path| path.ends_with("src/main.rs")),
        "expected main.rs in changes, got {changed_paths:?}"
    );

    let lib_content =
        std::fs::read_to_string(project.root().join("src/lib.rs")).expect("failed to read lib.rs after rename");
    let main_content =
        std::fs::read_to_string(project.root().join("src/main.rs")).expect("failed to read main.rs after rename");

    assert!(
        lib_content.contains("say_hello") && !lib_content.contains("greet"),
        "expected lib.rs to be renamed, got: {lib_content}"
    );
    assert!(
        main_content.contains("say_hello") && !main_content.contains("greet"),
        "expected main.rs to be renamed, got: {main_content}"
    );
}

/// Test: document symbols returns structs and functions
#[tokio::test]
async fn test_document_symbols() {
    let project = CargoProject::new("docsym_test").expect("Failed to create project");
    project
        .add_file(
            "src/main.rs",
            r#"struct Point {
    x: f64,
    y: f64,
}

fn distance(a: &Point, b: &Point) -> f64 {
    ((a.x - b.x).powi(2) + (a.y - b.y).powi(2)).sqrt()
}

fn main() {
    let p1 = Point { x: 0.0, y: 0.0 };
    let p2 = Point { x: 3.0, y: 4.0 };
    println!("{}", distance(&p1, &p2));
}
"#,
        )
        .expect("Failed to add file");

    let main_rs = project.file_path_str("src/main.rs");
    let (_server_handle, client) = connect_lsp(&project).await;

    let result = poll_lsp_tool(
        &client,
        "lsp_document",
        serde_json::json!({
            "file_path": main_rs
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
