//! Stream a one-shot prompt through a Bedrock model served by the Responses API.
//!
//! ```bash
//! export AWS_REGION=us-west-2
//! export AWS_BEARER_TOKEN_BEDROCK=...   # or rely on the SigV4 credential chain
//! cargo run -p aether-llm --features bedrock --example bedrock_mantle -- \
//!     openai.gpt-5.6-luna "What is 12 * 17? Use the calculator tool."
//! ```
use futures::StreamExt;
use llm::catalog::ModelTransport;
use llm::providers::bedrock::BedrockProvider;
use llm::{
    ChatMessage, Context, LlmModel, LlmResponse, ProviderConnectionConfig, ReasoningEffort, StreamingModelProvider,
    ToolDefinition,
};
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
    let Some(model_id) = args.next() else {
        eprintln!("usage: bedrock_mantle <model-id> [prompt]");
        eprintln!("  e.g. bedrock_mantle openai.gpt-5.6-luna \"What is 12 * 17? Use the calculator tool.\"");
        return ExitCode::from(2);
    };
    let prompt = args.next().unwrap_or_else(|| "What is 12 * 17? Use the calculator tool.".to_string());

    let Ok(model) = format!("bedrock:{model_id}").parse::<LlmModel>() else {
        eprintln!("'{model_id}' is not a known Bedrock model");
        return ExitCode::from(2);
    };
    let Some(transport) = model.transport() else {
        eprintln!(
            "'{model_id}' is served by the Converse API, not the Responses API.\n\
             Try one of: openai.gpt-5.6-luna, openai.gpt-5.6-terra, openai.gpt-5.6-sol,\n\
             openai.gpt-5.5, openai.gpt-5.4, openai.gpt-oss-120b, openai.gpt-oss-20b, xai.grok-4.3"
        );
        return ExitCode::from(2);
    };

    let provider = BedrockProvider::new(ProviderConnectionConfig::default()).await.with_model(&model_id);
    println!("→ {}", provider.display_name());
    let ModelTransport::OpenAiResponses { base_url_template } = transport;
    println!("  endpoint template: {base_url_template}");
    println!("  wire shape: OpenAI Responses");
    match provider.context_window() {
        Some(n) => println!("  context window: {n}"),
        None => println!("  context window: unknown"),
    }
    println!("  reasoning levels: {:?}", model.reasoning_levels());
    println!("  prompt: {prompt}\n");

    let mut context = Context::new(vec![ChatMessage::user(prompt)], vec![calculator_tool()]);
    context.set_reasoning_effort(Some(ReasoningEffort::Low));

    let mut stream = provider.stream_response(&context);
    let mut saw_error = false;
    while let Some(event) = stream.next().await {
        match event {
            Ok(LlmResponse::Text { chunk }) => {
                print!("{chunk}");
                std::io::stdout().flush().ok();
            }
            Ok(LlmResponse::Reasoning { chunk }) => {
                eprint!("\x1b[2m{chunk}\x1b[0m");
                std::io::stderr().flush().ok();
            }
            Ok(LlmResponse::ToolRequestComplete { tool_call }) => {
                eprintln!("\n[tool] {} {}", tool_call.name, tool_call.arguments);
            }
            Ok(LlmResponse::Usage { tokens }) => {
                eprintln!("\n[usage] input={} output={}", tokens.input_tokens, tokens.output_tokens);
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

fn calculator_tool() -> ToolDefinition {
    ToolDefinition {
        name: "calculator".to_string(),
        description: "Evaluate an arithmetic expression".to_string(),
        parameters: serde_json::json!({
            "type": "object",
            "properties": { "expression": { "type": "string" } },
            "required": ["expression"]
        }),
        server: None,
        annotations: None,
    }
}
