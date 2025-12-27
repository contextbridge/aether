use aether::{
    agent::{AgentMessage, UserMessage, agent},
    llm::openrouter::OpenRouterProvider,
};
use std::io::{self, Write};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt::init();

    let llm = OpenRouterProvider::default("z-ai/glm-4.5-air")?;
    let (tx, mut rx, _handle) = agent(llm)
        .system("You are a helpful assistant.")
        .spawn()
        .await?;

    tx.send(UserMessage::text("Write one paragraph about a unicorn"))
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
                    println!("\n\n✓ Message complete");
                }
            }
            Some(ToolCall { request, .. }) => {
                println!("🔧 Tool '{}' in progress", request.name);
            }
            Some(ToolResult { result, .. }) => {
                println!("🔧 Tool '{}' completed", result.name);
                println!("   Result: {}", result.result);
            }
            Some(ToolError { error, .. }) => {
                eprintln!("🔧 Tool '{}' failed: {}", error.name, error.error);
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
                    "🔧 Tool '{}' progress: {}{}{}",
                    request.name, msg, progress, total_str
                );
            }
            Some(Done) => {
                println!("✓ Agent finished processing");
                break;
            }
            Some(Error { message }) => {
                eprintln!("❌ Error: {message}");
                break;
            }
            Some(Cancelled { .. }) => {
                println!("⚠️  Processing cancelled");
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
