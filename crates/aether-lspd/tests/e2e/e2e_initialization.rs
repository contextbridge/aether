use crate::common::use_fake_servers;
use aether_lspd::testing::TestDaemon;
use aether_lspd::{ClientError, LanguageId, LspClient};
use std::time::Duration;
use tempfile::TempDir;

#[tokio::test]
async fn server_exiting_during_initialize_fails_connect() {
    let message = connect_failure(LanguageId::Go).await;

    assert!(message.contains("failed to complete initialization"), "{message}");
}

#[tokio::test]
async fn server_rejecting_initialize_fails_connect() {
    let message = connect_failure(LanguageId::C).await;

    assert!(message.contains("failed to complete initialization"), "{message}");
}

async fn connect_failure(language: LanguageId) -> String {
    use_fake_servers();

    let root = TempDir::new().expect("Failed to create project");
    let daemon =
        TestDaemon::spawn(root.path(), language, Duration::from_secs(2)).await.expect("Failed to spawn daemon");

    let error = LspClient::connect(root.path(), language).await.err().expect("connect should fail");
    daemon.shutdown().expect("Failed to shut down daemon");

    let ClientError::InitializationFailed(message) = error else {
        panic!("Expected initialization failure, got {error}");
    };
    message
}
