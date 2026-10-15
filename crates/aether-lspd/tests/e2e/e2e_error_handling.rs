use crate::common::{cargo_project, connect, fake_daemon, file_uri};
use aether_lspd::LanguageId;

#[tokio::test]
async fn daemon_survives_client_drop() {
    let project = cargo_project("client_drop");
    let (_daemon, client) = fake_daemon(project.root(), LanguageId::Rust).await;
    drop(client);

    let client = connect(project.root(), LanguageId::Rust).await;
    let hover = client.hover(file_uri(&project.path("src/main.rs")), 0, 0).await.expect("Hover failed");
    assert!(hover.is_some());
}

#[tokio::test]
async fn invalid_server_message_does_not_break_the_session() {
    let project = cargo_project("garbled_message");
    let (_daemon, client) = fake_daemon(project.root(), LanguageId::Python).await;

    let hover = client.hover(file_uri(&project.path("src/main.rs")), 0, 0).await.expect("Hover failed");
    assert!(hover.is_some(), "the response after an invalid server message should still arrive");
}
