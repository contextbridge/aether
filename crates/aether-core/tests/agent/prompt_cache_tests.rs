use std::error::Error;

use aether_core::core::Prompt;
use aether_core::events::{AgentCommand, Command};
use aether_core::testing::{FakeMcpServer, FakeTool, TestScenario, test_agent};
use llm::testing::llm_response;
use llm::{LlmResponse, ToolDefinition};

#[tokio::test]
async fn derives_same_cache_key_for_shared_prompt_prefix() -> Result<(), Box<dyn Error>> {
    let first = prompt_cache_key("system prompt", None, "first question").await?;
    let second = prompt_cache_key("system prompt", None, "different question").await?;

    assert_eq!(first, second);
    Ok(())
}

#[tokio::test]
async fn cache_key_changes_with_system_prompt() -> Result<(), Box<dyn Error>> {
    let first = prompt_cache_key("first system prompt", None, "question").await?;
    let second = prompt_cache_key("second system prompt", None, "question").await?;

    assert_ne!(first, second);
    Ok(())
}

#[tokio::test]
async fn cache_key_changes_with_tools() -> Result<(), Box<dyn Error>> {
    let first = prompt_cache_key("system prompt", Some(server_with_tool("read_file")), "question").await?;
    let second = prompt_cache_key("system prompt", Some(server_with_tool("write_file")), "question").await?;

    assert_ne!(first, second);
    Ok(())
}

#[tokio::test]
async fn cache_key_refreshes_after_tool_updates() -> Result<(), Box<dyn Error>> {
    let result = test_agent()
        .without_mcp()
        .system_prompt(Prompt::text("system prompt"))
        .llm_responses(&[response(), response()])
        .scenario(
            TestScenario::new()
                .user_text("first question")
                .wait_for_turn_end()
                .send(Command::AgentCommand(AgentCommand::UpdateTools(vec![tool("read_file")])))
                .user_text("second question")
                .wait_for_turn_end(),
        )
        .run_with_context()
        .await?;

    let contexts = result.captured_contexts.lock().unwrap();
    assert_eq!(contexts.len(), 2, "expected two LLM requests");
    assert_ne!(contexts[0].prompt_cache_key(), contexts[1].prompt_cache_key());
    Ok(())
}

#[tokio::test]
async fn cache_key_is_stable_across_turns() -> Result<(), Box<dyn Error>> {
    let result = test_agent()
        .without_mcp()
        .system_prompt(Prompt::text("system prompt"))
        .llm_responses(&[response(), response()])
        .scenario(
            TestScenario::new()
                .user_text("first question")
                .wait_for_turn_end()
                .user_text("second question")
                .wait_for_turn_end(),
        )
        .run_with_context()
        .await?;

    let contexts = result.captured_contexts.lock().unwrap();
    assert_eq!(contexts.len(), 2, "expected two LLM requests");
    assert_eq!(contexts[0].prompt_cache_key(), contexts[1].prompt_cache_key());
    Ok(())
}

#[tokio::test]
async fn cache_key_changes_with_model() -> Result<(), Box<dyn Error>> {
    let first = prompt_cache_key_for_model("codex:gpt-5.6-sol").await?;
    let second = prompt_cache_key_for_model("anthropic:claude-opus-4-5").await?;

    assert_ne!(first, second);
    Ok(())
}

#[tokio::test]
async fn separate_agents_receive_distinct_session_affinity_keys() -> Result<(), Box<dyn Error>> {
    let first = session_affinity_key().await?;
    let second = session_affinity_key().await?;

    assert_ne!(first, second);
    Ok(())
}

#[tokio::test]
async fn session_affinity_is_stable_across_turns_and_distinct_from_the_prefix_key() -> Result<(), Box<dyn Error>> {
    let result = test_agent()
        .without_mcp()
        .system_prompt(Prompt::text("system prompt"))
        .session_affinity_key("conversation-123")
        .llm_responses(&[response(), response()])
        .scenario(
            TestScenario::new()
                .user_text("first question")
                .wait_for_turn_end()
                .user_text("second question")
                .wait_for_turn_end(),
        )
        .run_with_context()
        .await?;

    let contexts = result.captured_contexts.lock().unwrap();
    assert_eq!(contexts.len(), 2, "expected two LLM requests");
    assert!(contexts.iter().all(|context| context.session_affinity_key() == Some("conversation-123")));
    assert!(contexts.iter().all(|context| context.prompt_cache_key() != context.session_affinity_key()));
    Ok(())
}

async fn prompt_cache_key(
    system_prompt: &str,
    tools: Option<FakeMcpServer>,
    user_prompt: &str,
) -> Result<String, Box<dyn Error>> {
    let builder = match tools {
        Some(server) => test_agent().fake_mcp_server("test", server),
        None => test_agent().without_mcp(),
    };
    let result = builder
        .system_prompt(Prompt::text(system_prompt))
        .llm_responses(&[response()])
        .user_text(user_prompt)
        .run_with_context()
        .await?;

    let contexts = result.captured_contexts.lock().unwrap();
    Ok(contexts[0].prompt_cache_key().expect("agent should derive a prompt cache key").to_string())
}

async fn prompt_cache_key_for_model(model: &str) -> Result<String, Box<dyn Error>> {
    let result = test_agent()
        .without_mcp()
        .model(model.parse()?)
        .system_prompt(Prompt::text("system prompt"))
        .llm_responses(&[response()])
        .user_text("question")
        .run_with_context()
        .await?;

    let contexts = result.captured_contexts.lock().unwrap();
    Ok(contexts[0].prompt_cache_key().expect("agent should derive a prompt cache key").to_string())
}

async fn session_affinity_key() -> Result<String, Box<dyn Error>> {
    let result =
        test_agent().without_mcp().llm_responses(&[response()]).user_text("question").run_with_context().await?;

    let contexts = result.captured_contexts.lock().unwrap();
    Ok(contexts[0].session_affinity_key().expect("agent should set a session affinity key").to_string())
}

fn response() -> Vec<LlmResponse> {
    llm_response("message").text(&["done"]).build()
}

fn server_with_tool(name: &str) -> FakeMcpServer {
    FakeMcpServer::new().with_tool(FakeTool::new(name).description(format!("{name} description")))
}

fn tool(name: &str) -> ToolDefinition {
    ToolDefinition::new(name, format!("{name} description"), serde_json::json!({ "type": "object" }))
}
