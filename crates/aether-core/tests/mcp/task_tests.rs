use aether_core::testing::{FakeMcpServer, FakeTool, FakeToolResponse, McpTestBuilder};
use mcp_utils::client::ToolCallEvent;
use rmcp::model::{
    CallToolResult, CreateTaskResult, DetailedTask, ElicitRequest, ElicitRequestParams, ElicitResult,
    ElicitationAction, InputRequest, InputRequests, Task, TaskPayload, TaskStatus,
};
use serde_json::json;
use std::time::Duration;

fn task(task_id: &str, status: TaskStatus) -> Task {
    let now = chrono::Utc::now().to_rfc3339();
    Task::new(task_id, status, now.clone(), now).with_poll_interval_ms(10)
}

fn task_server(task_id: &str, states: impl IntoIterator<Item = DetailedTask>) -> FakeMcpServer {
    let seed = task(task_id, TaskStatus::Working);
    FakeMcpServer::new()
        .with_tool(FakeTool::new("deferred").responds(FakeToolResponse::task(CreateTaskResult::new(seed))))
        .with_task(task_id, states)
}

fn completed_task(task_id: &str, text: &str) -> DetailedTask {
    let result = CallToolResult::success(vec![rmcp::model::ContentBlock::text(text)]);
    DetailedTask::new(
        task(task_id, TaskStatus::Completed),
        TaskPayload::Completed { result: serde_json::from_value(serde_json::to_value(result).unwrap()).unwrap() },
    )
}

fn input_required_task(task_id: &str, key: &str) -> DetailedTask {
    let request = InputRequest::Elicitation(ElicitRequest::new(ElicitRequestParams::FormElicitationParams {
        meta: None,
        message: "Provide a name".to_string(),
        requested_schema: serde_json::from_value(json!({
            "type": "object",
            "properties": { "name": { "type": "string" } },
            "required": ["name"]
        }))
        .unwrap(),
    }));
    DetailedTask::new(
        task(task_id, TaskStatus::InputRequired),
        TaskPayload::InputRequired { input_requests: InputRequests::from([(key.to_string(), request)]) },
    )
}

#[tokio::test]
async fn task_input_is_elicited_and_sent_to_tasks_update() {
    let server = task_server(
        "input-task",
        [input_required_task("input-task", "answer"), completed_task("input-task", "accepted")],
    );
    let state = server.state();
    let response = ElicitResult::new(ElicitationAction::Accept).with_content(json!({ "name": "Ada" }));
    let expected_response = serde_json::to_value(&response).unwrap();
    let test = McpTestBuilder::new().server("tasks", server).elicitation_response(response).build().await;

    test.call("tasks", "deferred", json!({})).await;
    let notification = test.next_task_outcome().await.expect("task notification");

    assert_eq!(notification.status, "completed");
    assert!(notification.body.contains("accepted"));
    assert_eq!(test.elicitations().len(), 1);
    let updates = state.task_updates();
    assert_eq!(updates.len(), 1);
    assert_eq!(updates[0].task_id, "input-task");
    assert_eq!(updates[0].input_responses.get("answer"), Some(&expected_response));
}

#[tokio::test]
async fn repeated_task_input_keys_terminate_with_an_error() {
    let server = task_server(
        "repeated-input",
        [
            input_required_task("repeated-input", "answer"),
            input_required_task("repeated-input", "answer"),
            completed_task("repeated-input", "must not complete"),
        ],
    );
    let state = server.state();
    let test = McpTestBuilder::new()
        .server("tasks", server)
        .elicitation_response(ElicitResult::new(ElicitationAction::Accept).with_content(json!({ "name": "Ada" })))
        .build()
        .await;

    test.call("tasks", "deferred", json!({})).await;
    let notification = test.next_task_outcome().await.expect("task error notification");

    assert_eq!(notification.status, "failed");
    assert!(notification.body.contains("repeated input requests"), "{}", notification.body);
    assert_eq!(test.elicitations().len(), 1);
    assert_eq!(state.task_updates().len(), 1);
    assert_eq!(state.task_cancel_ids(), ["repeated-input"]);
}

#[tokio::test]
async fn any_repeated_task_input_key_terminates_without_partial_update() {
    let TaskPayload::InputRequired { input_requests: mut second_requests } =
        input_required_task("mixed-input", "answer").payload
    else {
        unreachable!()
    };
    let TaskPayload::InputRequired { mut input_requests } = input_required_task("mixed-input", "new-answer").payload
    else {
        unreachable!()
    };
    let new_request = input_requests.remove("new-answer").unwrap();
    second_requests.insert("new-answer".to_string(), new_request);
    let second_round = DetailedTask::new(
        task("mixed-input", TaskStatus::InputRequired),
        TaskPayload::InputRequired { input_requests: second_requests },
    );
    let server = task_server(
        "mixed-input",
        [
            input_required_task("mixed-input", "answer"),
            second_round,
            completed_task("mixed-input", "must not complete"),
        ],
    );
    let state = server.state();
    let test = McpTestBuilder::new()
        .server("tasks", server)
        .elicitation_response(ElicitResult::new(ElicitationAction::Accept))
        .build()
        .await;

    test.call("tasks", "deferred", json!({})).await;
    let notification = test.next_task_outcome().await.expect("task error notification");

    assert_eq!(notification.status, "failed");
    assert!(notification.body.contains("repeated input requests"), "{}", notification.body);
    assert_eq!(test.elicitations().len(), 1);
    assert_eq!(state.task_updates().len(), 1, "must not submit a partial second input round");
    assert_eq!(state.task_cancel_ids(), ["mixed-input"]);
}

#[tokio::test]
async fn failed_task_payload_produces_error_notification() {
    let failed = DetailedTask::new(
        task("failed-task", TaskStatus::Failed),
        TaskPayload::Failed {
            error: serde_json::from_value(json!({ "code": "boom", "message": "task exploded" })).unwrap(),
        },
    );
    let test = McpTestBuilder::new().server("tasks", task_server("failed-task", [failed])).build().await;

    test.call("tasks", "deferred", json!({})).await;
    let notification = test.next_task_outcome().await.expect("task error notification");

    assert_eq!(notification.status, "failed");
    assert!(notification.body.contains("task exploded"));
    assert_eq!(notification.status, "failed");
}

#[tokio::test]
async fn cancelled_task_payload_produces_error_notification() {
    let cancelled = DetailedTask::new(task("cancelled-task", TaskStatus::Cancelled), TaskPayload::Cancelled);
    let test = McpTestBuilder::new().server("tasks", task_server("cancelled-task", [cancelled])).build().await;

    test.call("tasks", "deferred", json!({})).await;
    let notification = test.next_task_outcome().await.expect("task cancellation notification");

    assert_eq!(notification.status, "failed");
    assert!(notification.body.contains("was cancelled"));
    assert_eq!(notification.status, "failed");
}

#[tokio::test]
async fn malformed_task_result_produces_error_notification() {
    let malformed = DetailedTask::new(
        task("malformed-task", TaskStatus::Completed),
        TaskPayload::Completed { result: serde_json::from_value(json!({ "unexpected": true })).unwrap() },
    );
    let test = McpTestBuilder::new().server("tasks", task_server("malformed-task", [malformed])).build().await;

    test.call("tasks", "deferred", json!({})).await;
    let notification = test.next_task_outcome().await.expect("malformed task notification");

    assert_eq!(notification.status, "failed");
    assert!(notification.body.contains("malformed result"), "{}", notification.body);
}

#[tokio::test]
async fn expired_task_terminates_before_polling() {
    let mut seed = task("expired-task", TaskStatus::Working);
    seed.created_at = "2020-01-01T00:00:00Z".to_string();
    seed.ttl_ms = Some(1);
    let server = FakeMcpServer::new()
        .with_tool(FakeTool::new("deferred").responds(FakeToolResponse::task(CreateTaskResult::new(seed))))
        .with_task("expired-task", [completed_task("expired-task", "too late")]);
    let state = server.state();
    let test = McpTestBuilder::new().server("tasks", server).build().await;

    test.call("tasks", "deferred", json!({})).await;
    let notification = test.next_task_outcome().await.expect("expired task notification");

    assert_eq!(notification.status, "failed");
    assert!(notification.body.contains("expired before completion"));
    assert!(state.task_get_ids().is_empty());
}

#[tokio::test]
async fn task_execution_deadline_terminates_polling() {
    let working = DetailedTask::new(task("deadline-task", TaskStatus::Working), TaskPayload::Working);
    let server = task_server("deadline-task", [working]);
    let state = server.state();
    let test = McpTestBuilder::new().server("tasks", server).tool_timeout(Duration::from_millis(25)).build().await;

    test.call("tasks", "deferred", json!({})).await;
    let notification = test.next_task_outcome().await.expect("deadline notification");

    assert_eq!(notification.status, "failed");
    assert!(notification.body.contains("execution deadline"), "{}", notification.body);
    assert_eq!(state.task_cancel_ids(), ["deadline-task"]);
}

#[tokio::test]
async fn tasks_get_failure_terminates_task() {
    let server =
        task_server("get-failure", [completed_task("get-failure", "must not finish")]).with_task_get_failures(1);
    let state = server.state();
    let test = McpTestBuilder::new().server("tasks", server).build().await;

    test.call("tasks", "deferred", json!({})).await;
    let notification = test.next_task_outcome().await.expect("task error notification");

    assert_eq!(notification.status, "failed");
    assert!(notification.body.contains("failed to get task"), "{}", notification.body);
    assert_eq!(state.task_get_ids(), ["get-failure"]);
    assert_eq!(state.task_cancel_ids(), ["get-failure"]);
}

#[tokio::test]
async fn tasks_update_failure_terminates_task() {
    let server =
        task_server("update-failure", [input_required_task("update-failure", "answer")]).with_task_update_failures(1);
    let state = server.state();
    let test = McpTestBuilder::new()
        .server("tasks", server)
        .elicitation_response(ElicitResult::new(ElicitationAction::Accept))
        .build()
        .await;

    test.call("tasks", "deferred", json!({})).await;
    let notification = test.next_task_outcome().await.expect("task error notification");

    assert_eq!(notification.status, "failed");
    assert!(notification.body.contains("failed to update task"), "{}", notification.body);
    assert_eq!(state.task_updates().len(), 1);
    assert_eq!(state.task_cancel_ids(), ["update-failure"]);
}

#[tokio::test]
async fn intermediate_task_status_is_emitted_before_outcome() {
    let server = task_server(
        "status-task",
        [
            DetailedTask::new(
                task("status-task", TaskStatus::Working).with_status_message("halfway"),
                TaskPayload::Working,
            ),
            completed_task("status-task", "done"),
        ],
    );
    let test = McpTestBuilder::new().server("tasks", server).build().await;

    test.call("tasks", "deferred", json!({})).await;
    let event = test.next_tool_event().await.expect("task status event");

    assert!(matches!(
        event,
        ToolCallEvent::TaskStatus(task)
            if task.task_id == "status-task"
                && task.status == TaskStatus::Working
                && task.status_message.as_deref() == Some("halfway")
    ));
}

#[tokio::test]
async fn deferred_task_is_polled_and_injected_as_a_success_notification() {
    let seed = task("task-1", TaskStatus::Working);
    let completed = DetailedTask::new(
        task("task-1", TaskStatus::Completed),
        TaskPayload::Completed {
            result: serde_json::from_value(json!({
                "content": [{"type": "text", "text": "finished"}],
                "isError": false
            }))
            .unwrap(),
        },
    );
    let server = FakeMcpServer::new()
        .with_tool(FakeTool::new("deferred").responds(FakeToolResponse::task(CreateTaskResult::new(seed.clone()))))
        .with_task("task-1", [DetailedTask::new(seed, TaskPayload::Working), completed]);
    let state = server.state();
    let test = McpTestBuilder::new().server("tasks", server).build().await;

    let outcome = test.call("tasks", "deferred", json!({})).await;
    assert_eq!(outcome.deferred_task.as_ref().map(|task| task.task.task_id.as_str()), Some("task-1"));
    assert!(outcome.result.unwrap().result.contains("task-1"));

    let notification = test.next_task_outcome().await.expect("task notification");
    assert_eq!(notification.task_id, "task-1");
    assert_eq!(notification.status, "completed");
    assert!(notification.body.contains("finished"));
    assert_eq!(state.task_get_ids(), ["task-1", "task-1"]);
}

#[tokio::test]
async fn cancelling_deferred_task_stops_polling_and_notifies_server() {
    let working = task("task-cancel", TaskStatus::Working);
    let server = FakeMcpServer::new()
        .with_tool(FakeTool::new("deferred").responds(FakeToolResponse::task(CreateTaskResult::new(working.clone()))))
        .with_task("task-cancel", [DetailedTask::new(working, TaskPayload::Working)]);
    let state = server.state();
    let test = McpTestBuilder::new().server("tasks", server).build().await;

    let outcome = test.call("tasks", "deferred", json!({})).await;
    let tool_id = outcome.result.expect("deferred acknowledgement").id;
    test.cancel_tool(&tool_id);

    loop {
        let event = test.next_tool_event().await.expect("tool event before cancellation");
        if let ToolCallEvent::Cancelled { task_id } = event {
            assert_eq!(task_id.as_deref(), Some("task-cancel"));
            break;
        }
    }
    assert_eq!(state.task_cancel_ids(), ["task-cancel"]);
}

#[tokio::test]
async fn deferred_task_keeps_its_ordered_stream_until_terminal_outcome() {
    let seed = task("ordered-terminal-task", TaskStatus::Completed);
    let result = CallToolResult::success(vec![rmcp::model::ContentBlock::text("already done")]);
    let server = FakeMcpServer::new()
        .with_tool(FakeTool::new("terminal").responds(FakeToolResponse::task(CreateTaskResult::new(seed.clone()))))
        .with_task(
            "ordered-terminal-task",
            [DetailedTask::new(
                seed,
                TaskPayload::Completed {
                    result: serde_json::from_value(serde_json::to_value(result).unwrap()).unwrap(),
                },
            )],
        );
    let test = McpTestBuilder::new().server("tasks", server).build().await;

    let outcome = test.call("tasks", "terminal", json!({})).await;
    assert!(outcome.deferred_task.is_some());

    assert!(test.next_tool_event().await.is_some(), "deferred tool stream ended before its terminal outcome");
}

#[tokio::test]
async fn terminal_seed_is_refreshed_from_tasks_get_before_notification() {
    let seed = task("terminal-task", TaskStatus::Completed);
    let result = CallToolResult::success(vec![rmcp::model::ContentBlock::text("already done")]);
    let server = FakeMcpServer::new()
        .with_tool(FakeTool::new("terminal").responds(FakeToolResponse::task(CreateTaskResult::new(seed.clone()))))
        .with_task(
            "terminal-task",
            [DetailedTask::new(
                seed,
                TaskPayload::Completed {
                    result: serde_json::from_value(serde_json::to_value(result).unwrap()).unwrap(),
                },
            )],
        );
    let state = server.state();
    let test = McpTestBuilder::new().server("tasks", server).build().await;

    let outcome = test.call("tasks", "terminal", json!({})).await;
    assert!(outcome.deferred_task.is_some());
    let notification = test.next_task_outcome().await.expect("terminal task notification");
    assert_eq!(notification.status, "completed");
    assert!(notification.body.contains("already done"));
    assert_eq!(state.task_get_ids(), ["terminal-task"]);
}
