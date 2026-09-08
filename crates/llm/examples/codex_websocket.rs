//! Live Codex WebSocket smoke test using Aether's existing OS-keychain credentials.
//!
//! ```bash
//! cargo run -p aether-llm --features codex,aether-auth/keyring --example codex_websocket
//! ```
//!
//! Optionally pass a model ID as the first argument (default: `gpt-5.6-luna`).
//! Requires an existing Codex login; expired credentials may be refreshed and saved.
//! Inference uses WebSocket only: the provider has no HTTP/SSE fallback.
use std::process::ExitCode;
use std::sync::Arc;

use aether_auth::OsKeyringStore;
use futures::StreamExt;
use llm::providers::codex::CodexProvider;
use llm::{
    ChatMessage, Context, LlmResponse, ProviderConnectionConfig, ReasoningEffort, StopReason, StreamingModelProvider,
};

#[tokio::main]
async fn main() -> ExitCode {
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_ansi(false)
        .with_env_filter("llm::providers::codex=debug")
        .init();

    match run().await {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("FAIL: {error}");
            ExitCode::FAILURE
        }
    }
}

async fn run() -> llm::Result<()> {
    let model = std::env::args().nth(1).unwrap_or_else(|| "gpt-5.6-luna".into());
    let store = Arc::new(OsKeyringStore::with_platform_store());
    let provider = CodexProvider::new(store, ProviderConnectionConfig::default(), &model)?;
    let mut context = Context::new(
        vec![
            ChatMessage::system("Follow the user's instructions exactly."),
            ChatMessage::user("Reply with exactly: Hello, world!"),
        ],
        vec![],
    );
    context.set_reasoning_effort(Some(ReasoningEffort::Low));
    context.set_session_affinity_key(Some(uuid::Uuid::new_v4().to_string()));
    context.set_turn_id(Some(uuid::Uuid::new_v4().to_string()));

    println!("Testing {} via WebSocket with OS-keychain authentication", provider.display_name());
    let mut stream = provider.stream_response(&context);
    let mut text = String::new();
    let mut completed = false;
    while let Some(event) = stream.next().await {
        match event? {
            LlmResponse::Text { chunk } => text.push_str(&chunk),
            LlmResponse::Usage { tokens } => {
                println!("Usage: input={} output={}", tokens.input_tokens, tokens.output_tokens);
            }
            LlmResponse::Done { stop_reason } => {
                println!("Done: {stop_reason:?}");
                completed = stop_reason == Some(StopReason::EndTurn);
            }
            LlmResponse::Error { message } => return Err(llm::LlmError::ProviderRequest(message)),
            _ => {}
        }
    }
    println!("Response: {text:?}");
    if !completed || text.trim() != "Hello, world!" {
        return Err(llm::LlmError::ProviderRequest(
            "Expected a completed EndTurn response containing exactly 'Hello, world!'".into(),
        ));
    }
    println!("PASS: received the expected completed response over Codex WebSocket (no SSE fallback)");
    Ok(())
}
