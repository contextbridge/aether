mod common;

use aether_lspd::LanguageId;
use common::{CargoProject, DaemonHarness, TestProject, hover_text, use_fake_rust_server};
use lsp_types::{DocumentSymbolResponse, GotoDefinitionResponse};

#[tokio::test]
async fn request_helpers_round_trip_through_fake_server() {
    use_fake_rust_server();

    let project = CargoProject::new("request_contracts").expect("Failed to create project");
    let content = r#"fn example_fn() -> i32 {
    7
}

fn main() {
    let value = example_fn();
    println!("{}", value);
}
"#;
    project.add_file("src/main.rs", content).expect("Failed to add file");

    let harness = DaemonHarness::spawn(project.root(), LanguageId::Rust).await.expect("Failed to spawn daemon");
    let client = harness.connect().await.expect("Failed to connect");
    let uri = project.file_uri("src/main.rs");

    let hover = hover_text(client.hover(uri.clone(), 0, 0).await.expect("Hover request failed"));
    assert!(hover.contains("open_count=1"));
    assert!(hover.contains("example_fn"));

    let definition = client.goto_definition(uri.clone(), 4, 16).await.expect("Goto definition failed");
    match definition {
        GotoDefinitionResponse::Array(locations) => {
            assert_eq!(locations.len(), 1);
            assert_eq!(locations[0].uri, uri);
            assert_eq!(locations[0].range.start.line, 0);
        }
        other => panic!("Expected array goto definition result, got {other:?}"),
    }

    let implementation = client.goto_implementation(uri.clone(), 4, 16).await.expect("Goto implementation failed");
    match implementation {
        GotoDefinitionResponse::Array(locations) => {
            assert_eq!(locations.len(), 1);
            assert_eq!(locations[0].uri, uri);
            assert_eq!(locations[0].range.start.line, 0);
        }
        other => panic!("Expected array goto implementation result, got {other:?}"),
    }

    let references = client.find_references(uri.clone(), 0, 3, true).await.expect("Find references failed");
    assert_eq!(references.len(), 2);
    assert!(references.iter().all(|location| location.uri == uri));

    let document_symbols = client.document_symbol(uri.clone()).await.expect("Document symbol failed");
    match document_symbols {
        DocumentSymbolResponse::Flat(symbols) => {
            assert!(symbols.iter().any(|symbol| symbol.name == "ExampleStruct"));
            assert!(symbols.iter().any(|symbol| symbol.name == "example_fn"));
        }
        other @ DocumentSymbolResponse::Nested(_) => panic!("Expected flat document symbols, got {other:?}"),
    }

    let workspace_symbols = client.workspace_symbol("example".to_string()).await.expect("Workspace symbol failed");
    assert!(workspace_symbols.iter().any(|symbol| symbol.name == "example_fn"));

    let call_items = client.prepare_call_hierarchy(uri.clone(), 4, 16).await.expect("Prepare call hierarchy failed");
    assert_eq!(call_items.len(), 1);
    assert_eq!(call_items[0].name, "example_fn");

    let incoming = client.incoming_calls(call_items[0].clone()).await.expect("Incoming calls failed");
    assert_eq!(incoming.len(), 1);
    assert_eq!(incoming[0].from.name, "caller_fn");

    let outgoing = client.outgoing_calls(call_items[0].clone()).await.expect("Outgoing calls failed");
    assert_eq!(outgoing.len(), 1);
    assert_eq!(outgoing[0].to.name, "callee_fn");

    let rename = client
        .rename(uri.clone(), 0, 3, "renamed_fn".to_string())
        .await
        .expect("Rename failed")
        .expect("Rename should return a workspace edit");
    #[allow(clippy::mutable_key_type)]
    let changes = rename.changes.expect("Rename should contain changes");
    let edits = changes.get(&uri).expect("Rename changes should include the file");
    assert_eq!(edits.len(), 1);
    assert_eq!(edits[0].new_text, "renamed_fn");

    harness.kill().await.expect("Failed to kill daemon");
}

#[tokio::test]
async fn diagnostic_helpers_round_trip_through_fake_server() {
    use_fake_rust_server();

    let project = CargoProject::new("request_diagnostics").expect("Failed to create project");
    project.add_file("src/main.rs", "fn main() { let error = 1; }\n").expect("Failed to add file");

    let harness = DaemonHarness::spawn(project.root(), LanguageId::Rust).await.expect("Failed to spawn daemon");
    let client = harness.connect().await.expect("Failed to connect");
    let uri = project.file_uri("src/main.rs");

    client.queue_diagnostic_refresh(uri.clone()).await.expect("Queue diagnostic refresh failed");

    let diagnostics = client.get_diagnostics(None).await.expect("Get diagnostics failed");
    assert_eq!(diagnostics.len(), 1);
    assert_eq!(diagnostics[0].uri, uri);
    assert_eq!(diagnostics[0].diagnostics.len(), 1);
    assert_eq!(diagnostics[0].diagnostics[0].message, "error token");

    harness.kill().await.expect("Failed to kill daemon");
}
