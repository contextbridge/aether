use aether::{
    agent::{AgentMessage, UserMessage, agent},
    llm::openrouter::OpenRouterProvider,
    mcp::mcp,
};
use std::io::{self, Write};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt::init();

    let llm = OpenRouterProvider::default("z-ai/glm-4.5-air")?;
    let (tools, mcp_tx, _mcp_handle) = mcp()
        .from_json_file("examples/mcp.json")
        .await?
        .spawn()
        .await?;
    let (tx, mut rx, _handle) = agent(llm)
        .system("You are a helpful assistant with access to web browsing tools via Playwright.")
        .tools(mcp_tx, tools)
        .spawn()
        .await?;

    tx.send(UserMessage::text(
        "Visit https://contextbridge.ai and tell me what you see",
    ))
    .await?;

    loop {
        use AgentMessage::*;
        match rx.recv().await {
            Some(Text {
                chunk, is_complete, ..
            }) => {
                if !is_complete {
                    print!("{chunk}");
                    io::stdout().flush().unwrap();
                } else {
                    println!();
                }
            }
            Some(ToolCall { request, .. }) => {
                println!("\n🔧 Tool '{}' in progress", request.name);
            }
            Some(ToolResult { result, .. }) => {
                println!("\n🔧 Tool '{}' completed successfully", result.name);
            }
            Some(ToolError { error, .. }) => {
                eprintln!("\n🔧 Tool '{}' failed: {}", error.name, error.error);
            }
            Some(ToolProgress {
                request,
                progress,
                total,
                message,
            }) => {
                let msg = message
                    .as_ref()
                    .map(|m| format!("{m} "))
                    .unwrap_or_default();
                let total_str = total.map(|t| format!("/{t}")).unwrap_or_default();
                println!(
                    "\n🔧 Tool '{}' progress: {}{}{}",
                    request.name, msg, progress, total_str
                );
            }
            Some(Done) => {
                println!("\n✓ Agent finished");
                break;
            }
            Some(Error { message }) => {
                eprintln!("❌ Error: {message}");
                break;
            }
            Some(Cancelled { .. }) => {
                println!("⚠️  Cancelled");
                break;
            }
            Some(ContextCompacted {
                messages_removed, ..
            }) => {
                println!("📦 Context compacted: {} messages removed", messages_removed);
            }
            None => {
                println!("Channel closed");
                break;
            }
        }
    }

    Ok(())
}
