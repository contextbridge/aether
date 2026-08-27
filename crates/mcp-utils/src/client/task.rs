use crate::client::McpClient;
use crate::client::call_tool::{CallToolError, ToolCallEvent};
use crate::client::elicitation::{ElicitInputsError, elicit_inputs};
use async_stream::stream;
use futures::future::{Either, select};
use futures::{Stream, StreamExt, pin_mut};
use rmcp::RoleClient;
use rmcp::model::{
    CancelTaskParams, CreateTaskResult, GetTaskParams, InputRequests, ProgressNotificationParam, Task, TaskPayload,
    TaskStatus, UpdateTaskParams,
};
use rmcp::service::{RunningService, ServiceError};
use std::collections::HashSet;
use std::future::Future;
use std::pin::pin;
use std::time::Duration;
use thiserror::Error;
use tokio::time::error::Elapsed;
use tokio::time::{Instant, sleep, timeout, timeout_at};
use tokio_util::sync::CancellationToken;

#[derive(Debug, Error)]
pub enum TaskErrorReason {
    #[error("failed to get task: {0}")]
    Get(#[source] ServiceError),
    #[error("failed to update task: {0}")]
    Update(#[source] ServiceError),
    #[error("expired before completion")]
    Expired,
    #[error("exceeded the {timeout:?} execution deadline")]
    TimedOut { timeout: Duration },
    #[error("repeated input requests that were already answered")]
    RepeatedInput,
    #[error("failed: {error}")]
    Failed { error: serde_json::Value },
    #[error("was cancelled")]
    Cancelled,
    #[error("returned a malformed result: {0}")]
    MalformedResult(#[source] serde_json::Error),
    #[error("requested an input kind this client does not support")]
    UnsupportedInput,
    #[error("produced an elicitation response that could not be serialized: {0}")]
    Serialize(#[source] serde_json::Error),
    #[error("returned a task payload this client does not support (status {status:?})")]
    UnsupportedPayload { status: TaskStatus },
}

pub(crate) struct TaskDriver<'a> {
    server_name: &'a str,
    client: &'a RunningService<RoleClient, McpClient>,
    timeout: Duration,
    cancellation_token: CancellationToken,
    default_poll_interval: Duration,
}

impl<'a> TaskDriver<'a> {
    pub(crate) fn new(
        server_name: &'a str,
        client: &'a RunningService<RoleClient, McpClient>,
        timeout: Duration,
        cancellation_token: CancellationToken,
    ) -> Self {
        Self { client, server_name, timeout, cancellation_token, default_poll_interval: Duration::from_secs(1) }
    }

    pub(crate) fn stream<T: Stream<Item = ProgressNotificationParam> + Send + 'a>(
        self,
        created: CreateTaskResult,
        progress_events: T,
    ) -> impl Stream<Item = ToolCallEvent> + 'a {
        stream! {
            yield ToolCallEvent::TaskCreated(created.clone());
            let task_events = self.stream_task_events(created.task);
            pin_mut!(task_events);
            pin_mut!(progress_events);

            loop {
                tokio::select! {
                    progress_event = progress_events.next() => {
                        let Some(progress_event) = progress_event else {
                            while let Some(event) = task_events.next().await {
                                yield event;
                            }
                            return;
                        };
                        yield ToolCallEvent::Progress(progress_event);
                    }
                    event = task_events.next() => {
                        let Some(event) = event else {
                            return;
                        };
                        yield event;
                    }
                }
            }
        }
    }

    fn stream_task_events(self, mut task: Task) -> impl Stream<Item = ToolCallEvent> + 'a {
        stream! {
            let bounds = TaskBounds::new(self.timeout, self.cancellation_token.clone());
            let mut answered_input_keys = HashSet::new();

            loop {
                if is_task_expired(&task) {
                    yield self.fail(task, TaskErrorReason::Expired);
                    return;
                }

                let detailed_task = match bounds
                    .run(self.client.get_task(GetTaskParams::new(task.task_id.clone())))
                    .await
                {
                    Ok(Ok(result)) => result.task,
                    Ok(Err(source)) => {
                        yield self.cancel(task, TaskErrorReason::Get(source)).await;
                        return;
                    }
                    Err(interrupt) => {
                        yield self.interrupted(task, interrupt).await;
                        return;
                    }
                };

                task = detailed_task.task;
                if !task.status.is_terminal() {
                    yield ToolCallEvent::TaskStatus(task.clone());
                }

                match detailed_task.payload {
                    TaskPayload::Working => {}
                    TaskPayload::InputRequired { input_requests } => {
                        match bounds.run(self.elicit_inputs(input_requests, &mut answered_input_keys, &task.task_id)).await {
                            Ok(Ok(())) => {}
                            Ok(Err(reason)) => {
                                yield self.cancel(task, reason).await;
                                return;
                            }
                            Err(interrupt) => {
                                yield self.interrupted(task, interrupt).await;
                                return;
                            }
                        }
                    }
                    TaskPayload::Completed { result } => {
                        let result = serde_json::from_value(serde_json::Value::Object(result))
                            .map_err(|source| self.error(&task.task_id, TaskErrorReason::MalformedResult(source)));
                        yield ToolCallEvent::TaskComplete { task, result };
                        return;
                    }
                    TaskPayload::Failed { error } => {
                        yield self.fail(task, TaskErrorReason::Failed { error: serde_json::Value::Object(error) });
                        return;
                    }
                    TaskPayload::Cancelled => {
                        yield self.fail(task, TaskErrorReason::Cancelled);
                        return;
                    }
                    _ => {
                        let status = task.status;
                        yield self.cancel(task, TaskErrorReason::UnsupportedPayload { status }).await;
                        return;
                    }
                }

                let duration = task.poll_interval_ms.map_or(self.default_poll_interval, Duration::from_millis);
                if let Err(interrupt) = bounds.run(sleep(duration)).await {
                    yield self.interrupted(task, interrupt).await;
                    return;
                }
            }
        }
    }

    async fn elicit_inputs(
        &self,
        input_requests: InputRequests,
        answered_input_keys: &mut HashSet<String>,
        task_id: &str,
    ) -> Result<(), TaskErrorReason> {
        if input_requests.keys().any(|key| answered_input_keys.contains(key)) {
            return Err(TaskErrorReason::RepeatedInput);
        }

        let (responses, _) = elicit_inputs(self.client.service(), input_requests).await?;
        answered_input_keys.extend(responses.keys().cloned());

        self.client.update_task(UpdateTaskParams::new(task_id, responses)).await.map_err(TaskErrorReason::Update)
    }

    async fn interrupted(&self, task: Task, interrupt: InterruptedReason) -> ToolCallEvent {
        match interrupt {
            InterruptedReason::TimedOut => self.cancel(task, TaskErrorReason::TimedOut { timeout: self.timeout }).await,
            InterruptedReason::Cancelled => {
                cancel_server_task(self.client, self.server_name, &task.task_id).await;
                ToolCallEvent::Cancelled { task_id: Some(task.task_id) }
            }
        }
    }

    async fn cancel(&self, task: Task, reason: TaskErrorReason) -> ToolCallEvent {
        cancel_server_task(self.client, self.server_name, &task.task_id).await;
        self.fail(task, reason)
    }

    fn fail(&self, task: Task, reason: TaskErrorReason) -> ToolCallEvent {
        let error = self.error(&task.task_id, reason);
        ToolCallEvent::TaskComplete { task, result: Err(error) }
    }

    fn error(&self, task_id: &str, reason: TaskErrorReason) -> CallToolError {
        CallToolError::Task {
            server: self.server_name.to_string(),
            task_id: task_id.to_string(),
            reason: Box::new(reason),
        }
    }
}

pub(crate) async fn cancel_server_task(
    client: &RunningService<RoleClient, McpClient>,
    server_name: &str,
    task_id: &str,
) {
    match timeout(Duration::from_secs(1), client.cancel_task(CancelTaskParams::new(task_id))).await {
        Ok(Ok(())) => {}
        Ok(Err(error)) => {
            tracing::warn!(server = %server_name, %task_id, "Failed to cancel abandoned MCP task: {error}");
        }
        Err(_) => tracing::warn!(server = %server_name, %task_id, "Timed out cancelling abandoned MCP task"),
    }
}

impl From<ElicitInputsError> for TaskErrorReason {
    fn from(error: ElicitInputsError) -> Self {
        match error {
            ElicitInputsError::UnsupportedInput => Self::UnsupportedInput,
            ElicitInputsError::Serialize(source) => Self::Serialize(source),
        }
    }
}

struct TaskBounds {
    deadline: TaskDeadline,
    cancel: CancellationToken,
}

enum InterruptedReason {
    TimedOut,
    Cancelled,
}

impl TaskBounds {
    fn new(timeout: Duration, cancel: CancellationToken) -> Self {
        Self { deadline: TaskDeadline::after(timeout), cancel }
    }

    async fn run<T>(&self, future: impl Future<Output = T>) -> Result<T, InterruptedReason> {
        let timedout = pin!(self.deadline.timeout(future));
        let cancelled = pin!(self.cancel.cancelled());
        match select(timedout, cancelled).await {
            Either::Left((Ok(value), _)) => Ok(value),
            Either::Left((Err(_), _)) => Err(InterruptedReason::TimedOut),
            Either::Right(((), _)) => Err(InterruptedReason::Cancelled),
        }
    }
}

enum TaskDeadline {
    At(Instant),
    FarFuture,
}

impl TaskDeadline {
    fn after(timeout: Duration) -> Self {
        Instant::now().checked_add(timeout).map_or(Self::FarFuture, Self::At)
    }

    async fn timeout<T>(&self, future: impl Future<Output = T>) -> Result<T, Elapsed> {
        match self {
            Self::At(deadline) => timeout_at(*deadline, future).await,
            Self::FarFuture => Ok(future.await),
        }
    }
}

fn is_task_expired(task: &Task) -> bool {
    if task.status.is_terminal() {
        return false;
    }
    let Some(ttl_ms) = task.ttl_ms else {
        return false;
    };
    let Ok(created_at) = chrono::DateTime::parse_from_rfc3339(&task.created_at) else {
        tracing::warn!(task_id = %task.task_id, created_at = %task.created_at, "Ignoring malformed MCP task creation timestamp");
        return false;
    };
    let Ok(ttl_ms) = i64::try_from(ttl_ms) else {
        return false;
    };
    created_at
        .with_timezone(&chrono::Utc)
        .checked_add_signed(chrono::Duration::milliseconds(ttl_ms))
        .is_some_and(|expires_at| chrono::Utc::now() > expires_at)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client::call_tool::{CallToolOptions, call_tool};
    use crate::client::{McpClientEvent, client_capabilities};
    use crate::testing::{FakeMcpServer, FakeMcpState, FakeTool, FakeToolResponse, connect};
    use futures::StreamExt;
    use rmcp::model::{
        CallToolRequestParams, CallToolResult, ClientInfo, CreateTaskResult, DetailedTask, ElicitRequest,
        ElicitRequestParams, Implementation, InputRequest, ProtocolVersion,
    };
    use serde_json::json;
    use std::sync::Arc;
    use tokio::sync::mpsc;

    #[tokio::test]
    async fn call_tool_drives_created_task_to_completion() {
        let result = task_test([completed_task()]).run().await;

        assert!(
            matches!(result.events.first(), Some(ToolCallEvent::TaskCreated(created)) if created.task.task_id == "task-1")
        );
        assert!(matches!(
            result.events.last(),
            Some(ToolCallEvent::TaskComplete { task, result: Ok(result) })
                if task.task_id == "task-1"
                    && result.content.first().and_then(|content| content.as_text()).is_some_and(|text| text.text == "finished")
        ));
        assert_eq!(result.state.task_get_ids(), ["task-1"]);
    }

    #[tokio::test]
    async fn call_tool_forwards_progress_after_task_creation() {
        let seed = task(TaskStatus::Working);
        let server = FakeMcpServer::new()
            .with_tool(
                FakeTool::new("deferred")
                    .responds(FakeToolResponse::task(CreateTaskResult::new(seed)).task_progress(1.0, Some(2.0))),
            )
            .with_task(
                "task-1",
                [DetailedTask::new(task(TaskStatus::Working), TaskPayload::Working), completed_task()],
            );
        let (event_tx, _event_rx) = mpsc::channel::<McpClientEvent>(4);
        let client = McpClient::new(
            ClientInfo::new(client_capabilities(), Implementation::new("test-client", "0.1.0")),
            "task-server".into(),
            event_tx,
        );
        let (_server, client) = connect(server, client).await.expect("connect task server");

        let events = call_tool(
            Arc::new(client),
            CallToolRequestParams::new("deferred"),
            CallToolOptions { timeout: Duration::from_secs(1), ..CallToolOptions::default() },
        )
        .collect::<Vec<_>>()
        .await;

        assert!(matches!(events.first(), Some(ToolCallEvent::TaskCreated(_))));
        assert!(events.iter().any(|event| matches!(
            event,
            ToolCallEvent::Progress(progress)
                if (progress.progress - 1.0).abs() < f64::EPSILON
                    && progress.total.is_some_and(|total| (total - 2.0).abs() < f64::EPSILON)
        )));
        assert!(matches!(events.last(), Some(ToolCallEvent::TaskComplete { result: Ok(_), .. })));
    }

    #[tokio::test]
    async fn call_tool_handles_huge_task_ttl() {
        let result =
            task_test([completed_task()]).with_task(task(TaskStatus::Working).with_ttl_ms(u64::MAX)).run().await;
        assert!(matches!(result.events.last(), Some(ToolCallEvent::TaskComplete { result: Ok(_), .. })));
    }

    #[tokio::test]
    async fn call_tool_handles_huge_execution_timeout() {
        let result = task_test([completed_task()]).with_timeout(Duration::MAX).run().await;
        assert!(matches!(result.events.last(), Some(ToolCallEvent::TaskComplete { result: Ok(_), .. })));
    }

    #[tokio::test]
    async fn call_tool_cancellation_cancels_server_task_and_ends_stream() {
        let seed = task(TaskStatus::Working);
        let server = FakeMcpServer::new()
            .with_tool(FakeTool::new("deferred").responds(FakeToolResponse::task(CreateTaskResult::new(seed.clone()))))
            .with_task("task-1", [DetailedTask::new(seed, TaskPayload::Working)]);
        let state = server.state();
        let (event_tx, _event_rx) = mpsc::channel::<McpClientEvent>(4);
        let client = McpClient::new(
            ClientInfo::new(client_capabilities(), Implementation::new("test-client", "0.1.0")),
            "task-server".into(),
            event_tx,
        );
        let (_server, client) = connect(server, client).await.expect("connect task server");

        let cancel = CancellationToken::new();
        let options = CallToolOptions { timeout: Duration::from_secs(5), meta: None, cancel: cancel.clone() };
        let mut events = pin!(call_tool(Arc::new(client), CallToolRequestParams::new("deferred"), options));

        assert!(matches!(events.next().await, Some(ToolCallEvent::TaskCreated(_))));
        cancel.cancel();
        let mut last = None;
        while let Some(event) = events.next().await {
            last = Some(event);
        }

        assert!(matches!(last, Some(ToolCallEvent::Cancelled { task_id: Some(task_id) }) if task_id == "task-1"));
        assert_eq!(state.task_cancel_ids(), ["task-1"]);
    }

    #[tokio::test]
    async fn call_tool_deadline_includes_task_elicitation() {
        let result = task_test([input_required_task()]).with_timeout(Duration::from_millis(25)).run().await;

        assert!(matches!(
            result.events.last(),
            Some(ToolCallEvent::TaskComplete {
                result: Err(CallToolError::Task { reason, .. }),
                ..
            }) if matches!(reason.as_ref(), TaskErrorReason::TimedOut { .. })
        ));
        assert_eq!(result.state.task_cancel_ids(), ["task-1"]);
    }

    struct TaskTest {
        seed: Task,
        states: Vec<DetailedTask>,
        timeout: Duration,
    }

    struct TaskTestResult {
        events: Vec<ToolCallEvent>,
        state: FakeMcpState,
    }

    fn task_test(states: impl IntoIterator<Item = DetailedTask>) -> TaskTest {
        TaskTest {
            seed: task(TaskStatus::Working),
            states: states.into_iter().collect(),
            timeout: Duration::from_secs(1),
        }
    }

    impl TaskTest {
        fn with_task(mut self, seed: Task) -> Self {
            self.seed = seed;
            self
        }

        fn with_timeout(mut self, timeout: Duration) -> Self {
            self.timeout = timeout;
            self
        }

        async fn run(self) -> TaskTestResult {
            let task_id = self.seed.task_id.clone();
            let server = FakeMcpServer::new()
                .with_tool(FakeTool::new("deferred").responds(FakeToolResponse::task(CreateTaskResult::new(self.seed))))
                .with_task(task_id, self.states);
            let state = server.state();
            let (event_tx, _event_rx) = mpsc::channel::<McpClientEvent>(4);
            let client = McpClient::new(
                ClientInfo::new(client_capabilities(), Implementation::new("test-client", "0.1.0")),
                "task-server".into(),
                event_tx,
            );
            let (_server, client) = connect(server, client).await.expect("connect task server");
            assert_eq!(client.peer_info().expect("peer info").protocol_version, ProtocolVersion::V_2026_07_28);

            let events = call_tool(
                Arc::new(client),
                CallToolRequestParams::new("deferred"),
                CallToolOptions { timeout: self.timeout, ..CallToolOptions::default() },
            )
            .collect()
            .await;
            TaskTestResult { events, state }
        }
    }

    fn completed_task() -> DetailedTask {
        let result = CallToolResult::success(vec![rmcp::model::ContentBlock::text("finished")]);
        DetailedTask::new(
            task(TaskStatus::Completed),
            TaskPayload::Completed { result: serde_json::from_value(json!(result)).expect("serialize tool result") },
        )
    }

    fn input_required_task() -> DetailedTask {
        let request = ElicitRequest::new(ElicitRequestParams::FormElicitationParams {
            meta: None,
            message: "Provide input".to_string(),
            requested_schema: serde_json::from_value(json!({
                "type": "object",
                "properties": {}
            }))
            .expect("valid elicitation schema"),
        });
        DetailedTask::new(
            task(TaskStatus::InputRequired),
            TaskPayload::InputRequired {
                input_requests: InputRequests::from([("answer".to_string(), InputRequest::Elicitation(request))]),
            },
        )
    }

    fn task(status: TaskStatus) -> Task {
        let now = chrono::Utc::now().to_rfc3339();
        Task::new("task-1", status, now.clone(), now).with_poll_interval_ms(10)
    }
}
