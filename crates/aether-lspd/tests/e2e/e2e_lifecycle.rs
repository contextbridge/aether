use crate::common::{
    TestWorkspace, cargo_project, connect, fake_daemon, file_uri, hover_text, wait_for_workspace_diagnostics,
    workspace_error_count,
};
use aether_lspd::LanguageId;

#[tokio::test]
async fn daemon_keeps_serving_after_a_client_disconnects() {
    let project = cargo_project("disconnect_persists");
    let (_daemon, client) = fake_daemon(project.root(), LanguageId::Rust).await;
    client.disconnect().await.expect("Failed to disconnect client");

    let client = connect(project.root(), LanguageId::Rust).await;
    let hover = client.hover(file_uri(&project.path("src/main.rs")), 0, 0).await.expect("Hover failed");
    assert!(hover.is_some());
}

#[tokio::test]
async fn multiple_clients_share_fake_server_session() {
    let project = cargo_project("shared_session");
    let (_daemon, client1) = fake_daemon(project.root(), LanguageId::Rust).await;
    let client2 = connect(project.root(), LanguageId::Rust).await;
    let uri = file_uri(&project.path("src/main.rs"));

    let hover1 = hover_text(client1.hover(uri.clone(), 0, 0).await.expect("Hover failed"));
    let hover2 = hover_text(client2.hover(uri, 0, 0).await.expect("Hover failed"));

    assert_ne!(hover1, "");
    assert_eq!(hover1, hover2);
}

#[tokio::test]
async fn diagnostics_are_available_across_clients_without_explicit_open() {
    let project = cargo_project("shared_diagnostics").file("src/main.rs", "fn main() { let error = 1; }\n");
    let (_daemon, client1) = fake_daemon(project.root(), LanguageId::Rust).await;
    let client2 = connect(project.root(), LanguageId::Rust).await;
    let uri = file_uri(&project.path("src/main.rs"));

    for client in [&client1, &client2] {
        let diagnostics = client.get_diagnostics(Some(uri.clone())).await.expect("Failed to get diagnostics");
        assert_eq!(diagnostics.len(), 1);
        assert_eq!(diagnostics[0].diagnostics.len(), 1);
    }
}

#[tokio::test]
async fn workspace_bootstrap_diagnostics_are_available_without_explicit_open() {
    let project = TestWorkspace::new()
        .file("build/Cargo.toml", "[package]\nname = \"build_root\"\nversion = \"0.1.0\"\nedition = \"2024\"\n")
        .file("build/src/main.rs", "fn main() { let error = 1; }\n");
    let (_daemon, client) = fake_daemon(&project.path("build"), LanguageId::Rust).await;

    let diagnostics =
        wait_for_workspace_diagnostics(&client, |diagnostics| workspace_error_count(diagnostics) > 0).await;

    assert_eq!(diagnostics.len(), 1);
    assert_eq!(workspace_error_count(&diagnostics), 1);
}

#[tokio::test]
async fn workspace_diagnostics_refresh_after_external_edit_without_explicit_open() {
    let project = cargo_project("workspace_external_refresh").file("src/main.rs", "fn main() { let ok = 1; }\n");
    let (_daemon, client) = fake_daemon(project.root(), LanguageId::Rust).await;

    let initial = wait_for_workspace_diagnostics(&client, |diagnostics| diagnostics.len() == 1).await;
    assert_eq!(workspace_error_count(&initial), 0);

    project.write("src/main.rs", "fn main() { let error = 1; }\n");

    let refreshed = wait_for_workspace_diagnostics(&client, |diagnostics| workspace_error_count(diagnostics) > 0).await;
    assert_eq!(refreshed.len(), 1);
    assert_eq!(workspace_error_count(&refreshed), 1);
}

#[tokio::test]
async fn bootstrap_and_requests_do_not_save_documents() {
    let project = cargo_project("no_implicit_saves").file("src/lib.rs", "pub fn lib() {}\n");
    let (_daemon, client) = fake_daemon(project.root(), LanguageId::Rust).await;
    client.get_diagnostics(None).await.expect("Failed to get workspace diagnostics");

    let hover = hover_text(client.hover(file_uri(&project.path("src/main.rs")), 0, 0).await.expect("Hover failed"));

    assert!(hover.contains("save_count=0"), "{hover}");
}

#[tokio::test]
async fn queued_refresh_after_an_edit_saves_the_document_once() {
    let project = cargo_project("edit_saves_once");
    let (_daemon, client) = fake_daemon(project.root(), LanguageId::Rust).await;
    client.get_diagnostics(None).await.expect("Failed to get workspace diagnostics");
    let uri = file_uri(&project.path("src/main.rs"));

    project.write("src/main.rs", "fn main() { let edited = 1; }\n");
    client.queue_diagnostic_refresh(uri.clone()).await.expect("Queue diagnostic refresh failed");
    client.get_diagnostics(None).await.expect("Failed to get workspace diagnostics");

    let hover = hover_text(client.hover(uri, 0, 0).await.expect("Hover failed"));
    assert!(hover.contains("save_count=1"), "{hover}");
}
