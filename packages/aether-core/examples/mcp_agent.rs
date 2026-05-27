use aether_core::{
    core::{Prompt, agent},
    events::{AgentMessage, UserMessage},
    mcp::mcp,
};
use llm::providers::openrouter::OpenRouterProvider;

use std::io::{self, Write};

#[allow(clippy::too_many_lines)]
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt::init();

    let llm = OpenRouterProvider::default("z-ai/glm-4.5-air")?;
    let mut spawn = mcp(std::env::current_dir()?).from_json_files(&["examples/mcp.json"]).await?.spawn().await?;
    let connection_details = spawn.block_until_ready().await.ok_or("MCP bootstrap aborted before completion")?;
    let _mcp_handle = spawn.handle;

    let (tx, mut rx, _handle) = agent(llm)
        .system_prompt(Prompt::text("You are a helpful assistant with access to web browsing tools via Playwright."))
        .tools(spawn.command_tx, connection_details.tool_definitions)
        .spawn()
        .await?;

    tx.send(UserMessage::text("Visit https://contextbridge.ai and tell me what you see")).await?;

    loop {
        use AgentMessage::{
            AutoContinue, Cancelled, ContextCleared, ContextCompactionResult, ContextCompactionStarted,
            ContextUsageUpdate, Done, Error, ModelSwitched, Retrying, Text, Thought, ToolCall, ToolCallUpdate,
            ToolError, ToolProgress, ToolResult,
        };
        match rx.recv().await {
            Some(Text { chunk, is_complete, .. }) => {
                if is_complete {
                    println!();
                } else {
                    print!("{chunk}");
                    io::stdout().flush().unwrap();
                }
            }
            Some(ToolCall { request, .. }) => {
                println!("\nTool '{}' in progress", request.name);
            }
            Some(ToolCallUpdate { .. }) => {}
            Some(ToolResult { result, .. }) => {
                println!("\nTool '{}' completed successfully", result.name);
            }
            Some(ToolError { error, .. }) => {
                eprintln!("\nTool '{}' failed: {}", error.name, error.error);
            }
            Some(ToolProgress { request, progress, total, message }) => {
                let msg = message.as_ref().map(|m| format!("{m} ")).unwrap_or_default();
                let total_str = total.map(|t| format!("/{t}")).unwrap_or_default();
                println!("\nTool '{}' progress: {}{}{}", request.name, msg, progress, total_str);
            }
            Some(Done) => {
                println!("\nAgent finished");
                break;
            }
            Some(Error { message }) => {
                eprintln!("Error: {message}");
                break;
            }
            Some(Cancelled { .. }) => {
                println!("Cancelled");
                break;
            }
            Some(ContextCompactionStarted { message_count }) => {
                println!("Context compaction started: {message_count} messages");
            }
            Some(ContextCompactionResult { messages_removed, .. }) => {
                println!("Context compacted: {messages_removed} messages removed");
            }
            Some(ContextUsageUpdate { usage_ratio, input_tokens, context_limit, .. }) => {
                match (usage_ratio, context_limit) {
                    (Some(usage_ratio), Some(context_limit)) => {
                        println!(
                            "Context usage: {:.1}% ({}/{} tokens)",
                            usage_ratio * 100.0,
                            input_tokens,
                            context_limit
                        );
                    }
                    _ => {
                        println!("Context usage: unknown limit ({input_tokens} tokens used)");
                    }
                }
            }
            Some(AutoContinue { attempt, max_attempts }) => {
                println!("Auto-continuing: attempt {attempt}/{max_attempts} (LLM stopped due to length)");
            }
            Some(Retrying { attempt, max_attempts, delay_ms, error }) => {
                println!("Retrying ({attempt}/{max_attempts}) in {delay_ms}ms: {error}");
            }
            Some(ModelSwitched { previous, new }) => {
                println!("Model switched: {previous} -> {new}");
            }
            Some(ContextCleared) => {
                println!("Context cleared");
            }
            Some(Thought { chunk, .. }) => {
                print!("{chunk}");
                io::stdout().flush().unwrap();
            }
            None => {
                println!("Channel closed");
                break;
            }
        }
    }

    Ok(())
}
