use aether_core::events::{AgentEvent, MessageEvent, ToolEvent, TurnEvent};
use aether_core::testing::{FakeMcpServer, FakeTool, FakeToolResponse, TestScenario, test_agent};
use llm::LlmResponse;
use llm::testing::llm_response;
use rmcp::model::{CallToolResult, CreateTaskResult, DetailedTask, Task, TaskPayload, TaskStatus};
use std::sync::Arc;
use tokio::sync::Notify;

#[tokio::test]
async fn user_cancel_does_not_report_foreground_tool_as_background_task() {
    let server = FakeMcpServer::new().with_tool(
        FakeTool::new("slow").responds(FakeToolResponse::text("late result").delay(std::time::Duration::from_secs(1))),
    );
    let arguments = serde_json::json!({}).to_string();

    let events = test_agent()
        .fake_mcp_server("tasks", server)
        .llm_responses(&[
            llm_response("msg_1").tool_call("foreground-call", "tasks__slow", &[&arguments]).build(),
            llm_response("msg_2").text(&["still works"]).build(),
        ])
        .scenario(
            TestScenario::new()
                .user_text("start foreground tool")
                .wait_for(|event| matches!(event, AgentEvent::Tool(ToolEvent::ExecutionStarted { tool_id, .. }) if tool_id == "foreground-call"))
                .cancel()
                .wait_for_turn_end()
                .user_text("continue")
                .wait_for_turn_end(),
        )
        .run()
        .await
        .unwrap();

    assert!(
        !events.iter().any(|event| matches!(event, AgentEvent::Tool(ToolEvent::TaskCompleted { request, .. } | ToolEvent::TaskFailed { request, .. } | ToolEvent::TaskCancelled { request, .. }) if request.id == "foreground-call")),
        "foreground cancellation must not be described as a background task: {events:?}"
    );
}

#[tokio::test]
async fn immediate_background_outcome_follows_deferred_event() {
    let now = chrono::Utc::now().to_rfc3339();
    let seed = Task::new("terminal-task", TaskStatus::Completed, now.clone(), now.clone());
    let completed = Task::new("terminal-task", TaskStatus::Completed, now.clone(), now);
    let result = CallToolResult::success(vec![rmcp::model::ContentBlock::text("finished")]);
    let server = FakeMcpServer::new()
        .with_tool(FakeTool::new("terminal").responds(FakeToolResponse::task(CreateTaskResult::new(seed))))
        .with_task(
            "terminal-task",
            [DetailedTask::new(
                completed,
                TaskPayload::Completed {
                    result: serde_json::from_value(serde_json::to_value(result).unwrap()).unwrap(),
                },
            )],
        );
    let arguments = serde_json::json!({}).to_string();

    let events = test_agent()
        .fake_mcp_server("tasks", server)
        .llm_responses(&[
            llm_response("msg_1").tool_call("terminal-call", "tasks__terminal", &[&arguments]).build(),
            llm_response("msg_2").text(&["background task started"]).build(),
            llm_response("msg_3").text(&["handled result"]).build(),
        ])
        .scenario(
            TestScenario::new()
                .user_text("start background task")
                .wait_for(|event| matches!(event, AgentEvent::Tool(ToolEvent::TaskCreated { request, .. }) if request.id == "terminal-call"))
                .wait_for(|event| matches!(event, AgentEvent::Tool(ToolEvent::TaskCompleted { request, .. } | ToolEvent::TaskFailed { request, .. }) if request.id == "terminal-call"))
                .wait_for_turn_end(),
        )
        .run()
        .await
        .unwrap();

    let deferred = events
        .iter()
        .position(|event| matches!(event, AgentEvent::Tool(ToolEvent::TaskCreated { request, .. }) if request.id == "terminal-call"))
        .unwrap();
    let outcome = events
        .iter()
        .position(|event| matches!(event, AgentEvent::Tool(ToolEvent::TaskCompleted { request, .. } | ToolEvent::TaskFailed { request, .. }) if request.id == "terminal-call"))
        .unwrap();
    assert!(deferred < outcome);
}

#[tokio::test]
async fn user_cancel_surfaces_background_task_cancellation() {
    let now = chrono::Utc::now().to_rfc3339();
    let task = Task::new("cancel-task", TaskStatus::Working, now.clone(), now).with_poll_interval_ms(10);
    let server = FakeMcpServer::new()
        .with_tool(FakeTool::new("deferred").responds(FakeToolResponse::task(CreateTaskResult::new(task.clone()))))
        .with_task("cancel-task", [DetailedTask::new(task, TaskPayload::Working)]);
    let server_state = server.state();
    let arguments = serde_json::json!({}).to_string();
    let release = Arc::new(Notify::new());

    let events = test_agent()
        .fake_mcp_server("tasks", server)
        .llm_responses(&[
            llm_response("msg_1").tool_call("cancel-call", "tasks__deferred", &[&arguments]).build(),
            vec![LlmResponse::start("paused-followup"), LlmResponse::done()],
        ])
        .pause_turn_after(1, 0, release)
        .scenario(
            TestScenario::new()
                .user_text("start background task")
                .wait_for(|event| matches!(event, AgentEvent::Tool(ToolEvent::TaskCreated { request, .. }) if request.id == "cancel-call"))
                .cancel()
                .wait_for_turn_end()
                .wait_for(|event| matches!(event, AgentEvent::Tool(ToolEvent::TaskCancelled { request, .. }) if request.id == "cancel-call")),
        )
        .run()
        .await
        .unwrap();

    assert_eq!(server_state.task_cancel_ids(), ["cancel-task"]);
    assert!(events.iter().any(
        |event| matches!(event, AgentEvent::Tool(ToolEvent::TaskCancelled { task_id, .. }) if task_id == "cancel-task")
    ));
}

#[tokio::test]
async fn user_cancel_preserves_queued_background_task_outcome() {
    let now = chrono::Utc::now().to_rfc3339();
    let seed = Task::new("queued-task", TaskStatus::Working, now.clone(), now.clone()).with_poll_interval_ms(10);
    let completed = Task::new("queued-task", TaskStatus::Completed, now.clone(), now);
    let result = CallToolResult::success(vec![rmcp::model::ContentBlock::text("finished")]);
    let server = FakeMcpServer::new()
        .with_tool(FakeTool::new("deferred").responds(FakeToolResponse::task(CreateTaskResult::new(seed))))
        .with_task(
            "queued-task",
            [DetailedTask::new(
                completed,
                TaskPayload::Completed {
                    result: serde_json::from_value(serde_json::to_value(result).unwrap()).unwrap(),
                },
            )],
        );
    let arguments = serde_json::json!({}).to_string();
    let release = Arc::new(Notify::new());

    let events = test_agent()
        .fake_mcp_server("tasks", server)
        .llm_responses(&[
            llm_response("msg_1").tool_call("queued-call", "tasks__deferred", &[&arguments]).build(),
            vec![LlmResponse::start("paused-followup"), LlmResponse::done()],
            llm_response("msg_3").text(&["ack"]).build(),
        ])
        .pause_turn_after(1, 0, release)
        .scenario(
            TestScenario::new()
                .user_text("start background task")
                .wait_for(|event| matches!(event, AgentEvent::Tool(ToolEvent::TaskCreated { request, .. }) if request.id == "queued-call"))
                .cancel()
                .wait_for({
                    let seen = std::cell::Cell::new((false, false));
                    move |event| {
                        let (mut outcome, mut ended) = seen.get();
                        outcome |= matches!(event, AgentEvent::Tool(ToolEvent::TaskCompleted { request, .. } | ToolEvent::TaskFailed { request, .. } | ToolEvent::TaskCancelled { request, .. }) if request.id == "queued-call");
                        ended |= matches!(event, AgentEvent::Turn(TurnEvent::Ended { .. }));
                        seen.set((outcome, ended));
                        outcome && ended
                    }
                }),
        )
        .run()
        .await
        .unwrap();

    assert!(
        events.iter().any(|event| matches!(
            event,
            AgentEvent::Turn(TurnEvent::Ended { outcome: aether_core::events::TurnOutcome::Cancelled })
        )),
        "expected the paused turn to end as cancelled: {events:?}"
    );
    if let Some(AgentEvent::Tool(ToolEvent::TaskCompleted { result, .. })) = events.iter().find(|event| {
        matches!(event, AgentEvent::Tool(ToolEvent::TaskCompleted { request, .. }) if request.id == "queued-call")
    }) {
        assert!(result.result.contains("finished"), "completed outcome should carry the task result: {result:?}");
    }
}

/// After cancelling, a new prompt should produce a normal response.
/// Regression test: the agent's `cancelled` flag was never reset, so all
/// LLM events after the first cancel were silently dropped.
#[tokio::test]
async fn test_prompt_after_cancel_produces_response() {
    let events = test_agent()
        .llm_responses(&[
            llm_response("msg_1").text(&["Hello", " world", " this", " is", " a", " long", " response"]).build(),
            llm_response("msg_2").text(&["Second response"]).build(),
        ])
        .scenario(
            TestScenario::new()
                .user_text("first question")
                .cancel()
                .wait_for_turn_end()
                .user_text("second question")
                .wait_for_turn_end(),
        )
        .run()
        .await
        .unwrap();

    assert!(
        events.iter().any(|event| matches!(
            event,
            AgentEvent::Turn(TurnEvent::Ended { outcome: aether_core::events::TurnOutcome::Cancelled })
        )),
        "expected the first turn to be cancelled"
    );

    let first_ended = events.iter().position(|e| matches!(e, AgentEvent::Turn(TurnEvent::Ended { .. }))).unwrap();
    let second_turn = &events[first_ended + 1..];

    assert!(
        second_turn.iter().any(|e| matches!(e, AgentEvent::Message(MessageEvent::Text { chunk, is_complete: false, .. }) if chunk == "Second response")),
        "expected streamed text from the second prompt"
    );
    assert!(
        second_turn.iter().any(|e| matches!(e, AgentEvent::Turn(TurnEvent::Ended { .. }))),
        "expected TurnEnded from the second prompt"
    );
}
