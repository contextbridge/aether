mod common;

use aether_lspd::LanguageId;
use common::{cargo_project, file_uri, hover_text, start_daemon};
use std::time::Duration;

#[tokio::test]
#[ignore = "requires rust-analyzer on PATH"]
async fn rust_analyzer_smoke_hover() {
    let project = cargo_project("smoke_ra").file("src/main.rs", "fn main() { let value = 1; }\n");
    let (_daemon, client) = start_daemon(project.root(), LanguageId::Rust).await;
    let uri = file_uri(&project.path("src/main.rs"));

    let hover = loop {
        if let Some(hover) = client.hover(uri.clone(), 0, 16).await.expect("Hover failed") {
            break hover;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    };

    let text = hover_text(Some(hover));
    assert!(text.contains("i32"), "expected the inferred type in hover: {text}");
}
