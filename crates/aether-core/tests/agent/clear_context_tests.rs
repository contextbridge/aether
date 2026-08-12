use aether_core::core::Prompt;
use aether_core::events::{AgentEvent, Command, ContextEvent, ToolEvent, UserCommand};
use aether_core::testing::{FakeMcpServer, FakeTool, FakeToolResponse, TestScenario, test_agent};
use llm::testing::llm_response;
use llm::{ChatMessage, ContentBlock, LlmResponse};
use rmcp::model::{CreateTaskResult, DetailedTask, Task, TaskPayload, TaskStatus};
use std::sync::Arc;
use tokio::sync::Notify;

#[tokio::test]
async fn clear_context_suppresses_cancelled_background_task_notification() -> Result<(), Box<dyn std::error::Error>> {
    let now = chrono::Utc::now().to_rfc3339();
    let task = Task::new("clear-task", TaskStatus::Working, now.clone(), now).with_poll_interval_ms(10);
    let server = FakeMcpServer::new()
        .with_tool(FakeTool::new("deferred").responds(FakeToolResponse::task(CreateTaskResult::new(task.clone()))))
        .with_task("clear-task", [DetailedTask::new(task, TaskPayload::Working)]);
    let server_state = server.state();
    let arguments = serde_json::json!({}).to_string();
    let release = Arc::new(Notify::new());

    let result = test_agent()
        .fake_mcp_server("tasks", server)
        .llm_responses(&[
            llm_response("msg_1").tool_call("clear-call", "tasks__deferred", &[&arguments]).build(),
            vec![LlmResponse::start("cancelled-followup"), LlmResponse::text("must not finish"), LlmResponse::done()],
            vec![LlmResponse::start("msg_2"), LlmResponse::text("fresh context"), LlmResponse::done()],
        ])
        .pause_turn_after(1, 0, release)
        .scenario(
            TestScenario::new()
                .user_text("start background task")
                .wait_for(|event| matches!(event, AgentEvent::Tool(ToolEvent::TaskCreated { request, .. }) if request.id == "clear-call"))
                .send(Command::UserCommand(UserCommand::ClearContext))
                .wait_for(|event| matches!(event, AgentEvent::Context(ContextEvent::Cleared)))
                .wait_for_turn_end()
                .user_text("new question")
                .wait_for_turn_end(),
        )
        .run_with_context()
        .await?;

    assert_eq!(server_state.task_cancel_ids(), ["clear-task"]);
    assert!(
        !result.messages.iter().any(|event| matches!(event, AgentEvent::Tool(ToolEvent::TaskCancelled { .. }))),
        "cancellation acknowledgement must not repopulate cleared context: {:?}",
        result.messages,
    );
    let contexts = result.captured_contexts.lock().unwrap();
    let final_context = contexts.last().expect("fresh request should reach the model");
    assert!(
        !final_context.messages().iter().any(|message| matches!(message, ChatMessage::User { content, .. } if ContentBlock::join_text(content).contains("clear-task"))),
        "fresh context must not contain stale task cancellation"
    );

    Ok(())
}

#[tokio::test]
async fn test_clear_context_resets_history_and_preserves_system_prompt() -> Result<(), Box<dyn std::error::Error>> {
    let result = test_agent()
        .without_mcp()
        .system_prompt(Prompt::text("You are a test agent."))
        .llm_responses(&[
            vec![LlmResponse::start("msg_1"), LlmResponse::text("First response"), LlmResponse::done()],
            vec![LlmResponse::start("msg_2"), LlmResponse::text("Second response"), LlmResponse::done()],
        ])
        .scenario(
            TestScenario::new()
                .user_text("first question")
                .wait_for_turn_end()
                .send(Command::UserCommand(UserCommand::ClearContext))
                .wait_for(|event| matches!(event, AgentEvent::Context(ContextEvent::Cleared)))
                .user_text("second question")
                .wait_for_turn_end(),
        )
        .run_with_context()
        .await?;

    let contexts = result.captured_contexts.lock().unwrap();
    assert_eq!(contexts.len(), 2, "expected two LLM requests");

    let second = &contexts[1];
    let messages = second.messages();

    assert!(
        matches!(messages.first(), Some(ChatMessage::System { .. })),
        "system prompt should be preserved after clear"
    );

    let has_first_question = messages.iter().any(|m| {
        matches!(
            m,
            ChatMessage::User { content, .. } if *content == vec![ContentBlock::text("first question")]
        )
    });
    assert!(!has_first_question, "first turn user text should be removed from cleared context");

    let has_second_question = messages.iter().any(|m| {
        matches!(
            m,
            ChatMessage::User { content, .. } if *content == vec![ContentBlock::text("second question")]
        )
    });
    assert!(has_second_question, "new prompt should be present after clear");

    Ok(())
}
