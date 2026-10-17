mod common;

use aether_lspd::LanguageId;
use common::{cargo_project, file_uri, hover_text, start_daemon};

#[tokio::test]
#[ignore = "requires rust-analyzer on PATH"]
async fn rust_analyzer_smoke_hover() {
    let project = cargo_project("smoke_ra").file("src/main.rs", "fn main() { let value = 1; }\n");
    let (_daemon, client) = start_daemon(project.root(), LanguageId::Rust).await;
    let uri = file_uri(&project.path("src/main.rs"));

    let hover = client.hover(uri, 0, 16).await.expect("Hover failed");

    let text = hover_text(hover);
    assert!(text.contains("i32"), "expected the inferred type in hover: {text}");
}
