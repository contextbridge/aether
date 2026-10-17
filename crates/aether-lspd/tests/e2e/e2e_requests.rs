use crate::common::{cargo_project, fake_daemon, fake_request, file_uri, hover_text, workspace_error_count};
use aether_lspd::LanguageId;
use futures::future::join_all;
use lsp_types::{DocumentSymbolResponse, GotoDefinitionResponse};
use std::path::PathBuf;

#[tokio::test]
async fn request_helpers_round_trip_through_fake_server() {
    let project = cargo_project("request_contracts");
    let content = r#"fn example_fn() -> i32 {
    7
}

fn main() {
    let value = example_fn();
    println!("{}", value);
}
"#;
    project.write("src/main.rs", content);

    let (_daemon, client) = fake_daemon(project.root(), LanguageId::Rust).await;
    let uri = file_uri(&project.path("src/main.rs"));

    let hover = hover_text(client.hover(uri.clone(), 5, 16).await.expect("Hover request failed"));
    assert!(hover.contains("open_count=1; line=5; character=16"), "{hover}");
    assert!(hover.contains("example_fn"));

    let definition = client.goto_definition(uri.clone(), 5, 16).await.expect("Goto definition failed");
    match definition {
        GotoDefinitionResponse::Array(locations) => {
            assert_eq!(locations.len(), 1);
            assert_eq!(locations[0].uri, uri);
            assert_eq!(locations[0].range.start.line, 0);
        }
        other => panic!("Expected array goto definition result, got {other:?}"),
    }

    let references = client.find_references(uri.clone(), 0, 3, true).await.expect("Find references failed");
    let lines: Vec<u32> = references.iter().map(|location| location.range.start.line).collect();
    assert_eq!(lines, [0, 5]);
    assert!(references.iter().all(|location| location.uri == uri));

    let document_symbols = client.document_symbol(uri.clone()).await.expect("Document symbol failed");
    match document_symbols {
        DocumentSymbolResponse::Flat(symbols) => {
            assert!(symbols.iter().any(|symbol| symbol.name == "ExampleStruct"));
            assert!(symbols.iter().any(|symbol| symbol.name == "example_fn"));
        }
        other @ DocumentSymbolResponse::Nested(_) => panic!("Expected flat document symbols, got {other:?}"),
    }

    let rename = client
        .rename(uri.clone(), 0, 3, "renamed_fn".to_string())
        .await
        .expect("Rename failed")
        .expect("Rename should return a workspace edit");
    #[allow(clippy::mutable_key_type)]
    let changes = rename.changes.expect("Rename should contain changes");
    let edits = changes.get(&uri).expect("Rename changes should include the file");
    assert_eq!(edits.len(), 2);
    assert!(edits.iter().all(|edit| edit.new_text == "renamed_fn"));
}

#[tokio::test]
async fn documents_in_paths_with_spaces_round_trip_through_the_daemon() {
    let project = cargo_project("request_spaces").file("src/my mod.rs", "fn main() { let error = 1; }\n");
    let (_daemon, client) = fake_daemon(project.root(), LanguageId::Rust).await;
    let uri = file_uri(&project.path("src/my mod.rs"));

    assert!(uri.as_str().contains("my%20mod.rs"), "URI must be percent-encoded: {}", uri.as_str());

    client.queue_diagnostic_refresh(uri.clone()).await.expect("Queue diagnostic refresh failed");

    let diagnostics = client.get_diagnostics(None).await.expect("Get diagnostics failed");
    let ours = diagnostics
        .iter()
        .find(|entry| entry.uri == uri)
        .unwrap_or_else(|| panic!("no diagnostics for {} in {diagnostics:?}", uri.as_str()));
    assert_eq!(ours.diagnostics.len(), 1);
    assert_eq!(ours.diagnostics[0].message, "error token");

    let on_disk = std::fs::canonicalize(project.root().join("src/my mod.rs")).expect("File should exist");
    assert_eq!(PathBuf::from(aether_lspd::uri_to_path(&uri)), on_disk);
}

#[tokio::test]
async fn diagnostic_helpers_round_trip_through_fake_server() {
    let project = cargo_project("request_diagnostics").file("src/main.rs", "fn main() { let error = 1; }\n");
    let (_daemon, client) = fake_daemon(project.root(), LanguageId::Rust).await;
    let uri = file_uri(&project.path("src/main.rs"));

    client.queue_diagnostic_refresh(uri.clone()).await.expect("Queue diagnostic refresh failed");

    let diagnostics = client.get_diagnostics(None).await.expect("Get diagnostics failed");
    assert_eq!(diagnostics.len(), 1);
    assert_eq!(diagnostics[0].uri, uri);
    assert_eq!(diagnostics[0].diagnostics.len(), 1);
    assert_eq!(diagnostics[0].diagnostics[0].message, "error token");
}

#[tokio::test]
async fn a_pending_diagnostics_call_does_not_block_other_requests_on_the_connection() {
    let project =
        cargo_project("held_diagnostics").file("src/main.rs", "// hold-diagnostics\nfn main() { let error = 1; }\n");
    let (_daemon, client) = fake_daemon(project.root(), LanguageId::Rust).await;
    let uri = file_uri(&project.path("src/main.rs"));

    let (diagnostics, hover) = tokio::join!(client.get_diagnostics(Some(uri.clone())), async {
        let hover = client.hover(uri.clone(), 0, 0).await;
        fake_request(&client, "fake/releaseDiagnostics").await;
        hover
    });

    assert!(hover.expect("Hover failed").is_some());
    assert_eq!(workspace_error_count(&diagnostics.expect("Failed to get diagnostics")), 1);
}

#[tokio::test]
async fn position_requests_wait_for_the_server_to_finish_loading() {
    let project = cargo_project("loading_server").file(".fake-loading", "");
    let (_daemon, client) = fake_daemon(project.root(), LanguageId::Rust).await;
    let uri = file_uri(&project.path("src/main.rs"));

    let (hover, ()) = tokio::join!(client.hover(uri, 0, 0), async {
        // The round trip gives the hover time to reach the daemon before loading finishes.
        fake_request(&client, "fake/noop").await;
        fake_request(&client, "fake/finishLoading").await;
    });

    assert!(hover_text(hover.expect("Hover failed")).contains("open_count=1"));
}

#[tokio::test]
async fn concurrent_requests_for_one_document_reach_the_server_after_it_opens() {
    let project = cargo_project("concurrent_open");
    let (_daemon, client) = fake_daemon(project.root(), LanguageId::Rust).await;
    let uri = file_uri(&project.path("src/main.rs"));

    let hovers = join_all((0..32).map(|_| client.hover(uri.clone(), 0, 0))).await;

    for hover in hovers {
        let hover = hover_text(hover.expect("Hover failed"));
        assert!(!hover.contains("open_count=0"), "a hover reached the server before the document was open: {hover}");
    }
}
