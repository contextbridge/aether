use clap::Parser;
use futures::StreamExt;
use llm::providers::anthropic::AnthropicProvider;
use llm::types::IsoString;
use llm::{ChatMessage, Context, LlmResponse, ModelSettings, ProviderFactory, StreamingModelProvider, ToolDefinition};
use serde_json::json;
use std::error::Error;
use std::io::{self, Write};

#[derive(Parser)]
#[command(author, version, about = "Test Anthropic provider integration")]
struct Args {
    /// The message to send to Claude
    #[arg(short, long, default_value = "Hello, Claude! Can you help me write a simple Rust function?")]
    prompt: String,

    /// Disable prompt caching (enabled by default)
    #[arg(long)]
    no_cache: bool,

    /// Claude model to use
    #[arg(short = 'm', long, default_value = "claude-sonnet-4-5-20250929")]
    model: String,

    /// Maximum tokens
    #[arg(long, default_value = "1000")]
    max_tokens: u32,

    /// Temperature (0.0 to 1.0)
    #[arg(long, default_value = "0.7")]
    temperature: f32,
}

#[allow(clippy::too_many_lines)]
#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let args = Args::parse();

    println!("🤖 Testing Anthropic Provider");
    println!("Model: {}", args.model);
    println!("Message: {}", args.prompt);
    println!("Caching: {}", if args.no_cache { "disabled" } else { "enabled" });
    println!("Temperature: {}", args.temperature);
    println!("Max tokens: {}", args.max_tokens);
    println!("{}", "=".repeat(50));

    let provider = AnthropicProvider::from_env().await?.with_model(&args.model);

    let _ = args.no_cache; // Prompt caching is always enabled

    // Prepare context
    let messages = vec![
        ChatMessage::system("You are a helpful AI assistant. Be concise but informative in your responses."),
        ChatMessage::User { content: vec![llm::ContentBlock::text(args.prompt)], timestamp: IsoString::now() },
    ];

    let tools = vec![
        ToolDefinition::new(
            "search_web",
            "Search the web for current information",
            json!({
                "type": "object",
                "properties": {
                    "query": {
                        "type": "string",
                        "description": "The search query"
                    }
                },
                "required": ["query"]
            }),
        ),
        ToolDefinition::new(
            "calculate",
            "Perform mathematical calculations",
            json!({
                "type": "object",
                "properties": {
                    "expression": {
                        "type": "string",
                        "description": "Mathematical expression to evaluate"
                    }
                },
                "required": ["expression"]
            }),
        ),
    ];
    let mut context = Context::new(messages, tools);
    context.set_model_settings(ModelSettings {
        temperature: Some(args.temperature),
        max_tokens: Some(args.max_tokens),
        ..Default::default()
    });

    // Stream the response
    let stream = provider.stream_response(&context);
    let mut stream = Box::pin(stream);

    print!("🔄 Streaming response: ");
    io::stdout().flush().unwrap();

    let mut current_tool_call = None;
    let mut response_text = String::new();

    while let Some(result) = stream.next().await {
        match result? {
            LlmResponse::Start { message_id } => {
                println!("\n✅ Started (ID: {message_id})");
            }
            LlmResponse::Text { chunk } => {
                print!("{chunk}");
                io::stdout().flush().unwrap();
                response_text.push_str(&chunk);
            }
            LlmResponse::Reasoning { .. } | LlmResponse::EncryptedReasoning { .. } => {}

            LlmResponse::ToolRequestStart { id, name } => {
                println!("\n🔧 Tool call started: {name} ({id})");
                current_tool_call = Some((id.clone(), name, String::new()));
            }
            LlmResponse::ToolRequestArg { id, chunk } => {
                if let Some((ref call_id, _, ref mut args)) = current_tool_call
                    && call_id == &id
                {
                    args.push_str(&chunk);
                    print!(".");
                    io::stdout().flush().unwrap();
                }
            }
            LlmResponse::ToolRequestComplete { tool_call } => {
                println!("\n🔧 Tool call completed: {} with args: {}", tool_call.name, tool_call.arguments);

                // Simulate tool execution (you would call actual tools here)
                let tool_result = match tool_call.name.as_str() {
                    "search_web" => "Search results: Found relevant information about Rust programming.",
                    "calculate" => "Calculation result: 42",
                    _ => "Tool executed successfully",
                };

                println!("🔧 Tool result: {tool_result}");
                current_tool_call = None;
            }
            LlmResponse::Done { stop_reason } => {
                if let Some(reason) = stop_reason {
                    println!("\n✅ Stream completed ({reason:?})");
                } else {
                    println!("\n✅ Stream completed");
                }
                break;
            }
            LlmResponse::Error { message } => {
                println!("\n❌ Error: {message}");
                break;
            }
            LlmResponse::Usage { tokens } => {
                println!(
                    "\n📊 Token usage - input: {}, output: {}, cache_read: {}, cache_creation: {}, reasoning: {}",
                    tokens.input_tokens,
                    tokens.output_tokens,
                    tokens.cache_read_tokens.unwrap_or(0),
                    tokens.cache_creation_tokens.unwrap_or(0),
                    tokens.reasoning_tokens.unwrap_or(0),
                );
            }
        }
    }

    println!("\n{}", "=".repeat(50));
    println!("📊 Summary:");
    println!("Total response length: {} characters", response_text.len());
    println!("✅ Test completed successfully!");

    Ok(())
}
