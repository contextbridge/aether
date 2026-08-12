use aether_core::core::{Prompt, agent};
use aether_core::events::{
    AgentEvent, Command, ContextEvent, MessageEvent, ModelEvent, ToolEvent, TurnEvent, TurnOutcome, UserCommand,
};
use llm::ContentBlock;
use llm::providers::openrouter::OpenRouterProvider;
use std::io::{self, Write};

#[allow(clippy::too_many_lines)]
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt::init();

    let llm = OpenRouterProvider::default("z-ai/glm-4.5-air")?;
    let (tx, mut rx, _handle) = agent(llm).system_prompt(Prompt::text("You are a helpful assistant.")).spawn().await?;

    tx.send(Command::UserCommand(UserCommand::Text {
        content: vec![ContentBlock::text("Write one paragraph about a unicorn")],
    }))
    .await?;

    loop {
        match rx.recv().await {
            Some(AgentEvent::Message(MessageEvent::Text { chunk, is_complete, .. })) => {
                if is_complete {
                    println!("\n\nMessage complete");
                } else {
                    print!("{chunk}");
                    io::stdout().flush().unwrap();
                }
            }
            Some(AgentEvent::Tool(ToolEvent::Call { request, .. })) => {
                println!("Tool '{}' in progress", request.name);
            }
            Some(AgentEvent::Tool(ToolEvent::Result { result, .. })) => {
                println!("Tool '{}' completed", result.name);
                println!("   Result: {}", result.result);
            }
            Some(AgentEvent::Tool(ToolEvent::Error { error, .. })) => {
                eprintln!("Tool '{}' failed: {}", error.name, error.error);
            }
            Some(AgentEvent::Tool(ToolEvent::Progress { request, progress, total, message })) => {
                let msg = message.as_ref().map(|m| format!("{m} ")).unwrap_or_default();
                let total_str = total.map(|t| format!("/{t}")).unwrap_or_default();
                println!("Tool '{}' progress: {}{}{}", request.name, msg, progress, total_str);
            }
            Some(AgentEvent::Tool(ToolEvent::TaskStatus { request, status, status_message, .. })) => {
                let detail = status_message.as_deref().unwrap_or(&status);
                println!("Tool '{}' background status: {detail}", request.name);
            }
            Some(AgentEvent::Tool(ToolEvent::TaskCompleted { request, task_id, result, .. })) => {
                println!("Tool '{}' background task {task_id} completed: {}", request.name, result.result);
            }
            Some(AgentEvent::Tool(ToolEvent::TaskFailed { request, task_id, error, .. })) => {
                eprintln!("Tool '{}' background task {task_id} failed: {}", request.name, error.error);
            }
            Some(AgentEvent::Tool(ToolEvent::TaskCancelled { request, task_id, .. })) => {
                println!("Tool '{}' background task {task_id} cancelled", request.name);
            }
            Some(AgentEvent::Turn(TurnEvent::Ended { outcome })) => {
                match outcome {
                    TurnOutcome::Completed => println!("Agent finished processing"),
                    TurnOutcome::Failed { error } => eprintln!("Error: {error}"),
                    TurnOutcome::Cancelled => println!("Processing cancelled"),
                }
                break;
            }
            Some(AgentEvent::Context(ContextEvent::CompactionStarted { message_count })) => {
                println!("Context compaction started: {message_count} messages");
            }
            Some(AgentEvent::Context(ContextEvent::CompactionEnded { outcome })) => {
                println!("Context compaction ended: {outcome:?}");
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
            Some(AgentEvent::Turn(turn @ TurnEvent::RetryScheduled { .. })) => {
                let retry = turn.retry_info().expect("retry event");
                println!("Retrying ({}/{}) in {}ms", retry.attempt, retry.max_attempts, retry.delay_ms);
            }
            Some(
                AgentEvent::Tool(
                    ToolEvent::CallUpdate { .. }
                    | ToolEvent::ExecutionStarted { .. }
                    | ToolEvent::DefinitionsUpdated { .. }
                    | ToolEvent::TaskCreated { .. },
                )
                | AgentEvent::Turn(
                    TurnEvent::Started { .. } | TurnEvent::LlmCallStarted { .. } | TurnEvent::LlmCallEnded { .. },
                ),
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
