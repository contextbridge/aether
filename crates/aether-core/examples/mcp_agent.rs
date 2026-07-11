use aether_core::events::{AgentEvent, ContextEvent, MessageEvent, ModelEvent, ToolEvent, TurnEvent};
use aether_core::{
    core::{Prompt, agent},
    events::{Command, TurnOutcome, UserCommand},
    mcp::mcp,
};
use llm::{ContentBlock, providers::openrouter::OpenRouterProvider};

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

    tx.send(Command::UserCommand(UserCommand::Text {
        content: vec![ContentBlock::text("Visit https://contextbridge.ai and tell me what you see")],
    }))
    .await?;

    loop {
        match rx.recv().await {
            Some(AgentEvent::Message(MessageEvent::Text { chunk, is_complete, .. })) => {
                if is_complete {
                    println!();
                } else {
                    print!("{chunk}");
                    io::stdout().flush().unwrap();
                }
            }
            Some(AgentEvent::Tool(ToolEvent::Call { request, .. })) => {
                println!("\nTool '{}' in progress", request.name);
            }
            Some(AgentEvent::Tool(ToolEvent::Result { result, .. })) => {
                println!("\nTool '{}' completed successfully", result.name);
            }
            Some(AgentEvent::Tool(ToolEvent::Error { error, .. })) => {
                eprintln!("\nTool '{}' failed: {}", error.name, error.error);
            }
            Some(AgentEvent::Tool(ToolEvent::Progress { request, progress, total, message })) => {
                let msg = message.as_ref().map(|m| format!("{m} ")).unwrap_or_default();
                let total_str = total.map(|t| format!("/{t}")).unwrap_or_default();
                println!("\nTool '{}' progress: {}{}{}", request.name, msg, progress, total_str);
            }
            Some(AgentEvent::Turn(TurnEvent::Ended { outcome })) => {
                match outcome {
                    TurnOutcome::Completed => println!("\nAgent finished"),
                    TurnOutcome::Failed { error } => eprintln!("Error: {error}"),
                    TurnOutcome::Cancelled => println!("Cancelled"),
                }
                break;
            }
            Some(AgentEvent::Context(ContextEvent::CompactionStarted { message_count })) => {
                println!("Context compaction started: {message_count} messages");
            }
            Some(AgentEvent::Context(ContextEvent::CompactionResult { messages_removed, .. })) => {
                println!("Context compacted: {messages_removed} messages removed");
            }
            Some(AgentEvent::Context(ContextEvent::UsageUpdated { usage })) => {
                match (usage.usage_ratio, usage.context_limit) {
                    (Some(usage_ratio), Some(context_limit)) => {
                        println!(
                            "Context usage: {:.1}% ({}/{} tokens)",
                            usage_ratio * 100.0,
                            usage.input_tokens,
                            context_limit
                        );
                    }
                    _ => {
                        println!("Context usage: unknown limit ({} tokens used)", usage.input_tokens);
                    }
                }
            }
            Some(AgentEvent::Turn(TurnEvent::AutoContinue { attempt, max_attempts })) => {
                println!("Auto-continuing: attempt {attempt}/{max_attempts} (LLM stopped due to length)");
            }
            Some(AgentEvent::Turn(turn @ TurnEvent::LlmCallStarted { .. })) => {
                if let Some(retry) = turn.retry_info() {
                    println!("Retrying ({}/{}) in {}ms", retry.attempt, retry.max_attempts, retry.delay_ms);
                }
            }
            Some(
                AgentEvent::Tool(
                    ToolEvent::CallUpdate { .. }
                    | ToolEvent::ExecutionStarted { .. }
                    | ToolEvent::DefinitionsUpdated { .. },
                )
                | AgentEvent::Turn(TurnEvent::Started | TurnEvent::LlmCallEnded { .. }),
            ) => {}
            Some(AgentEvent::Model(ModelEvent::Switched { previous, new })) => {
                println!("Model switched: {previous} -> {new}");
            }
            Some(AgentEvent::Context(ContextEvent::Cleared)) => {
                println!("Context cleared");
            }
            Some(AgentEvent::Message(MessageEvent::Thought { chunk, .. })) => {
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
