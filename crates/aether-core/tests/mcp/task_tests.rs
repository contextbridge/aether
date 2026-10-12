use super::support::{TaskOutcome, call_tool};
use aether_core::events::ToolEvent;
use aether_core::testing::test_agent;
use mcp_utils::testing::{FakeMcpServer, FakeTask, completed_task, failed_task, task, working_task};
use rmcp::model::{DetailedTask, TaskPayload, TaskStatus};
use serde_json::json;
use std::time::Duration;

#[tokio::test]
async fn a_created_task_runs_in_the_background_until_its_outcome_arrives() {
    let server = task_server([working_task(TASK), completed_task(TASK, "finished")]);

    let run = call_tool(test_agent().mcp_server("tasks", server), DEFERRED, json!({})).await;

    assert_eq!(run.created_task(), Some(TASK));
    assert!(matches!(run.task_outcome(), TaskOutcome::Completed(body) if body.contains("finished")));
}

#[tokio::test]
async fn a_failed_task_becomes_a_failed_outcome() {
    let server = task_server([failed_task(TASK, "task exploded")]);

    let run = call_tool(test_agent().mcp_server("tasks", server), DEFERRED, json!({})).await;

    assert!(matches!(run.task_outcome(), TaskOutcome::Failed(body) if body.contains("task exploded")));
}

#[tokio::test]
async fn task_statuses_are_reported_before_the_outcome() {
    let halfway =
        DetailedTask::new(task(TASK, TaskStatus::Working).with_status_message("halfway"), TaskPayload::Working);
    let server = task_server([halfway, completed_task(TASK, "done")]);

    let run = call_tool(test_agent().mcp_server("tasks", server), DEFERRED, json!({})).await;

    let status = run.tool_events().position(|event| {
        matches!(
            event,
            ToolEvent::TaskStatus { task_id, status, status_message, .. }
                if task_id == TASK && status == "working" && status_message.as_deref() == Some("halfway")
        )
    });
    let outcome = run.tool_events().position(|event| matches!(event, ToolEvent::TaskCompleted { .. }));
    assert!(status.expect("task status event") < outcome.expect("task outcome"));
}

#[tokio::test(start_paused = true)]
async fn the_tool_timeout_bounds_background_tasks() {
    let server = task_server([working_task(TASK)]);
    let state = server.state();
    let agent = test_agent().mcp_server("tasks", server).tool_timeout(Duration::from_millis(25));

    let run = call_tool(agent, DEFERRED, json!({})).await;

    assert!(matches!(run.task_outcome(), TaskOutcome::Failed(body) if body.contains("timeout")));
    assert_eq!(state.task_cancel_ids(), [TASK]);
}

fn task_server(states: impl IntoIterator<Item = DetailedTask>) -> FakeMcpServer {
    FakeTask::new(TASK).states(states).server("deferred")
}

const TASK: &str = "task-1";
const DEFERRED: &str = "tasks__deferred";
