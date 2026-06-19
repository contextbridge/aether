//! Stream a one-shot prompt through a Bedrock inference profile.
use futures::StreamExt;
use llm::providers::bedrock::BedrockProvider;
use llm::types::IsoString;
use llm::{ChatMessage, ContentBlock, Context, LlmResponse, StreamingModelProvider};
use std::env;
use std::io::Write;
use std::process::ExitCode;

#[tokio::main]
async fn main() -> ExitCode {
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "llm=debug".into()))
        .init();

    let mut args = env::args().skip(1);
    let Some(model) = args.next() else {
        eprintln!("usage: bedrock_inference_profile <model-id> <inference-profile-arn> [prompt]");
        return ExitCode::from(2);
    };
    let Some(arn) = args.next() else {
        eprintln!("usage: bedrock_inference_profile <model-id> <inference-profile-arn> [prompt]");
        return ExitCode::from(2);
    };
    let prompt = args.next().unwrap_or_else(|| "Say hello in one sentence.".to_string());

    let provider = BedrockProvider::new().await.with_model(&model).with_inference_profile_arn(&arn);
    println!("→ {}", provider.display_name());

    match provider.context_window() {
        Some(n) => println!("  context window: {n} (resolved from catalog)"),
        None => println!("  context window: unknown (model not in catalog)"),
    }
    println!("  prompt: {prompt}\n");

    let context = Context::new(
        vec![ChatMessage::User { content: vec![ContentBlock::text(prompt)], timestamp: IsoString::now() }],
        Vec::new(),
    );

    let mut stream = provider.stream_response(&context);
    let mut saw_error = false;
    while let Some(event) = stream.next().await {
        match event {
            Ok(LlmResponse::Text { chunk }) => {
                print!("{chunk}");
                std::io::stdout().flush().ok();
            }
            Ok(LlmResponse::Usage { tokens }) => {
                eprintln!("\n\n[usage] input={} output={}", tokens.input_tokens, tokens.output_tokens);
            }
            Ok(LlmResponse::Done { stop_reason }) => {
                eprintln!("[done] stop_reason={stop_reason:?}");
            }
            Ok(_) => {}
            Err(e) => {
                eprintln!("\n[error] {e}");
                saw_error = true;
                break;
            }
        }
    }

    if saw_error { ExitCode::from(1) } else { ExitCode::SUCCESS }
}
