use std::error::Error;

use aether_core::core::{Prompt, agent};
use aether_core::events::{AgentCommand, AgentEvent, Command, TurnEvent, UserCommand};
use llm::testing::{FakeLlmProvider, llm_response};
use llm::{ContentBlock, LlmModel, ToolDefinition};
use tokio::sync::mpsc;

#[tokio::test]
async fn derives_same_cache_key_for_shared_prompt_prefix() -> Result<(), Box<dyn Error>> {
    let first = capture_cache_key("system prompt", Vec::new(), "first question").await?;
    let second = capture_cache_key("system prompt", Vec::new(), "different question").await?;

    assert_eq!(first, second);
    Ok(())
}

#[tokio::test]
async fn cache_key_changes_with_system_prompt() -> Result<(), Box<dyn Error>> {
    let first = capture_cache_key("first system prompt", Vec::new(), "question").await?;
    let second = capture_cache_key("second system prompt", Vec::new(), "question").await?;

    assert_ne!(first, second);
    Ok(())
}

#[tokio::test]
async fn cache_key_changes_with_tools() -> Result<(), Box<dyn Error>> {
    let first = capture_cache_key("system prompt", vec![tool("read_file")], "question").await?;
    let second = capture_cache_key("system prompt", vec![tool("write_file")], "question").await?;

    assert_ne!(first, second);
    Ok(())
}

#[tokio::test]
async fn cache_key_refreshes_after_tool_updates() -> Result<(), Box<dyn Error>> {
    let llm = fake_llm("codex:gpt-5.6-sol", 2)?;
    let captured = llm.captured_contexts();
    let (tx, mut rx, _handle) = agent(llm).system_prompt(Prompt::text("system prompt")).spawn().await?;

    send_prompt(&tx, &mut rx, "first question").await?;
    tx.send(Command::AgentCommand(AgentCommand::UpdateTools(vec![tool("read_file")]))).await?;
    send_prompt(&tx, &mut rx, "second question").await?;

    let contexts = captured.lock().unwrap();
    assert_ne!(contexts[0].prompt_cache_key(), contexts[1].prompt_cache_key());
    Ok(())
}

#[tokio::test]
async fn cache_key_is_stable_across_turns() -> Result<(), Box<dyn Error>> {
    let llm = fake_llm("codex:gpt-5.6-sol", 2)?;
    let captured = llm.captured_contexts();
    let (tx, mut rx, _handle) = agent(llm).system_prompt(Prompt::text("system prompt")).spawn().await?;

    send_prompt(&tx, &mut rx, "first question").await?;
    send_prompt(&tx, &mut rx, "second question").await?;

    let contexts = captured.lock().unwrap();
    assert_eq!(contexts[0].prompt_cache_key(), contexts[1].prompt_cache_key());
    Ok(())
}

#[tokio::test]
async fn cache_key_changes_with_model() -> Result<(), Box<dyn Error>> {
    let first = capture_cache_key_for_model("codex:gpt-5.6-sol").await?;
    let second = capture_cache_key_for_model("anthropic:claude-opus-4-5").await?;

    assert_ne!(first, second);
    Ok(())
}

async fn capture_cache_key(
    system_prompt: &str,
    tools: Vec<ToolDefinition>,
    user_prompt: &str,
) -> Result<String, Box<dyn Error>> {
    capture_cache_key_with("codex:gpt-5.6-sol", system_prompt, tools, user_prompt).await
}

async fn capture_cache_key_for_model(model: &str) -> Result<String, Box<dyn Error>> {
    capture_cache_key_with(model, "system prompt", Vec::new(), "question").await
}

async fn capture_cache_key_with(
    model: &str,
    system_prompt: &str,
    tools: Vec<ToolDefinition>,
    user_prompt: &str,
) -> Result<String, Box<dyn Error>> {
    let mcp = aether_core::mcp::mcp("/workspace").spawn().await?;
    let llm = fake_llm(model, 1)?;
    let captured = llm.captured_contexts();
    let (tx, mut rx, _handle) =
        agent(llm).system_prompt(Prompt::text(system_prompt)).tools(mcp.handle().clone(), tools).spawn().await?;

    send_prompt(&tx, &mut rx, user_prompt).await?;

    let contexts = captured.lock().unwrap();
    Ok(contexts[0].prompt_cache_key().expect("agent should derive a prompt cache key").to_string())
}

fn fake_llm(model: &str, turns: usize) -> Result<FakeLlmProvider, Box<dyn Error>> {
    let model: LlmModel = model.parse()?;
    let responses = (0..turns).map(|_| llm_response("message").text(&["done"]).build()).collect();
    Ok(FakeLlmProvider::new(responses).with_model(model))
}

async fn send_prompt(
    tx: &mpsc::Sender<Command>,
    rx: &mut mpsc::Receiver<AgentEvent>,
    content: &str,
) -> Result<(), Box<dyn Error>> {
    tx.send(Command::UserCommand(UserCommand::Text { content: vec![ContentBlock::text(content)] })).await?;

    while let Some(event) = rx.recv().await {
        if matches!(event, AgentEvent::Turn(TurnEvent::Ended { .. })) {
            break;
        }
    }

    Ok(())
}

fn tool(name: &str) -> ToolDefinition {
    ToolDefinition::new(name, format!("{name} description"), serde_json::json!({ "type": "object" }))
}
