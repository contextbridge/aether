use futures::StreamExt;
use futures::future::Either;
use mcp_utils::client::{
    CancellationToken, ClientError, ClientOptions, Elicitation, ElicitationSink, TaskErrorReason, ToolCallError,
    ToolCallEvent, ToolCallOptions,
};
use mcp_utils::model::{
    CallToolRequestParams, CallToolResponse, CallToolResult, CustomNotification, DEFAULT_MRTR_MAX_ROUNDS, DetailedTask,
    ElicitRequestParams, ElicitResult, ElicitationAction, ElicitationCapability, ErrorData, FormElicitationCapability,
    InputRequests, InputRequiredResult, ProtocolVersion, ServerCapabilities, ServerConfig, ServerNotification, Task,
    TaskPayload, TaskStatus, Tool, UrlElicitationCapability,
};
use mcp_utils::testing::{
    ElicitationScript, FakeMcpServer, FakeMcpState, FakeTask, FakeTool, FakeToolResponse, accept_with_name, call,
    cancelled_task, completed_task, connect, elicitation_host, failed_task, input_requests, input_required_task,
    name_input, task, url_request, working_task,
};
use rmcp::{RoleServer, ServerHandler, service::RequestContext};
use serde_json::{Map, json};
use std::time::Duration;
use tokio::sync::mpsc;
use tokio::time::Instant;

#[tokio::test]
async fn connect_prefers_stateless_discovery_for_modern_servers() {
    let server = FakeMcpServer::new().with_protocol_versions([ProtocolVersion::V_2026_07_28]);

    assert_eq!(negotiated_version(server).await, ProtocolVersion::V_2026_07_28);
}

#[tokio::test]
async fn connect_selects_an_older_mutually_supported_revision() {
    let server = FakeMcpServer::new().with_protocol_versions([ProtocolVersion::V_2025_06_18]);

    assert_eq!(negotiated_version(server).await, ProtocolVersion::V_2025_06_18);
}

#[tokio::test]
async fn connect_falls_back_to_legacy_initialization() {
    let server = FakeMcpServer::new().with_legacy_protocol();

    assert_eq!(negotiated_version(server).await, ProtocolVersion::V_2025_11_25);
}

#[tokio::test]
async fn exposes_the_server_name_description_and_instructions() {
    let server = FakeMcpServer::math().with_instructions("Use the math tools");
    let client = connect("fake", server, &ClientOptions::default()).await;

    assert_eq!(client.name(), "fake");
    assert_eq!(client.description().as_deref(), Some("A fake MCP server for testing"));
    assert_eq!(client.instructions().as_deref(), Some("Use the math tools"));
    assert!(client.list_tools().await.unwrap().iter().any(|tool| tool.name == "add_numbers"));
}

#[tokio::test]
async fn servers_without_prompts_list_none() {
    let client = connect("fake", FakeMcpServer::new(), &ClientOptions::default()).await;

    assert_eq!(client.list_prompts().await.unwrap().len(), 0);
}

#[tokio::test]
async fn tool_calls_resolve_to_their_result() {
    let client = connect("fake", FakeMcpServer::math(), &ClientOptions::default()).await;

    let result = call(&client, "add_numbers", json!({"a": 2, "b": 3})).await;

    assert_eq!(result.unwrap().structured_content, Some(json!({"sum": 5})));
}

#[tokio::test]
async fn tool_lists_start_with_the_current_tools_then_follow_each_change() {
    let server = FakeMcpServer::math();
    let state = server.state();
    let client = connect("fake", server, &ClientOptions::default()).await;
    let mut tool_lists = Box::pin(client.tool_lists());

    let current = tool_names(tool_lists.next().await);
    state.add_tool_and_notify(FakeTool::new("added_later")).await;
    let changed = tool_names(tool_lists.next().await);

    assert_eq!(current, ["add_numbers", "divide_numbers"]);
    assert_eq!(changed, ["add_numbers", "added_later", "divide_numbers"]);
}

#[tokio::test]
async fn closing_any_clone_ends_the_session_for_all() {
    let client = connect("fake", FakeMcpServer::new(), &ClientOptions::default()).await;

    client.clone().close().await;

    assert!(client.list_tools().await.is_err());
}

#[tokio::test]
async fn tasks_are_always_advertised_but_elicitation_needs_an_event_sink() {
    let without_sink = advertised(ClientOptions::default()).await;
    let (events, _host) = mpsc::channel(1);
    let with_sink =
        advertised(ClientOptions::default().elicitation(ElicitationSink::form_and_url(events.clone()))).await;
    let url = ElicitationCapability::new().with_url(UrlElicitationCapability::new());
    let url_only =
        advertised(ClientOptions::default().elicitation(ElicitationSink::new(events.clone(), url.clone()))).await;
    let neither =
        advertised(ClientOptions::default().elicitation(ElicitationSink::new(events, ElicitationCapability::new())))
            .await;

    assert!(without_sink.tasks && with_sink.tasks && url_only.tasks && neither.tasks);
    assert_eq!(without_sink.elicitation, None);
    assert_eq!(with_sink.elicitation, Some(url.with_form(FormElicitationCapability::new())));
    assert_eq!(url_only.elicitation, Some(ElicitationCapability::new().with_url(UrlElicitationCapability::new())));
    assert_eq!(neither.elicitation, None);
}

#[tokio::test]
async fn elicitations_resolve_as_cancel_without_an_event_sink() {
    let test = ElicitationTest::new();
    let client = connect("fake", test.server(), &ClientOptions::default()).await;

    call(&client, "ask", json!({})).await.unwrap();

    assert_eq!(test.answer(), json!({"action": "cancel"}));
}

#[tokio::test]
async fn dropping_an_elicitation_answers_cancel() {
    let test = ElicitationTest::new();
    let (options, mut host) = elicitation_host();
    let client = connect("fake", test.server(), &options).await;
    let host = tokio::spawn(async move {
        let Some(Elicitation::Request(elicitation)) = host.recv().await else { panic!("expected an elicitation") };
        drop(elicitation);
    });

    call(&client, "ask", json!({})).await.unwrap();
    host.await.unwrap();

    assert_eq!(test.answer(), json!({"action": "cancel"}));
}

#[tokio::test]
async fn elicitation_responses_reach_the_server_and_name_their_source() {
    let test = ElicitationTest::new();
    let (options, mut host) = elicitation_host();
    let client = connect("fake", test.server(), &options).await;
    let host = tokio::spawn(async move {
        let Some(Elicitation::Request(elicitation)) = host.recv().await else { panic!("expected an elicitation") };
        let server = elicitation.server.clone();
        elicitation.respond(ElicitResult::new(ElicitationAction::Accept).with_content(json!({"name": "Ferris"})));
        server
    });

    call(&client, "ask", json!({})).await.unwrap();

    assert_eq!(host.await.unwrap(), "fake");
    assert_eq!(test.answer(), json!({"action": "accept", "content": {"name": "Ferris"}}));
}

#[tokio::test]
async fn elicitations_answer_cancel_once_the_host_stops_listening() {
    let test = ElicitationTest::new();
    let (options, host) = elicitation_host();
    let client = connect("fake", test.server(), &options).await;
    drop(host);

    call(&client, "ask", json!({})).await.unwrap();

    assert_eq!(test.answer(), json!({"action": "cancel"}));
}

#[tokio::test]
async fn elicitation_completion_notifications_name_their_source() {
    let (options, mut host) = elicitation_host();
    let client = connect("linear", CompletionServer, &options).await;

    call(&client, "complete", json!({})).await.unwrap();

    let event = host.recv().await.expect("completion event");
    assert!(matches!(event, Elicitation::Complete { server, id } if server == "linear" && id == "oauth-1"));
}

#[tokio::test]
async fn input_required_without_requests_or_state_aborts() {
    let server = asking(CallToolResponse::from(InputRequiredResult::from_input_requests(InputRequests::new())));
    let client = connect("fake", server, &ClientOptions::default()).await;

    let result = call(&client, "ask", json!({})).await;

    assert!(matches!(result, Err(ToolCallError::EmptyInputRequired)), "{result:?}");
}

#[tokio::test]
async fn state_only_rounds_poll_until_the_server_completes() {
    let server = FakeMcpServer::new().with_tool(
        FakeTool::new("ask")
            .responds(poll("first"))
            .when_state("first", poll("second"))
            .when_state("second", FakeToolResponse::text("done")),
    );
    let state = server.state();
    let client = connect("fake", server, &ClientOptions::default()).await;

    let result = call(&client, "ask", json!({})).await.unwrap();

    assert_eq!(text(&result), "done");
    assert_eq!(request_states(&state), [None, Some("first".to_string()), Some("second".to_string())]);
}

#[tokio::test(start_paused = true)]
async fn state_only_rounds_back_off_exponentially_up_to_a_cap_and_reset_after_input() {
    let mut tool = FakeTool::new("ask").responds(poll("1"));
    for round in 1..7 {
        tool = tool.when_state(round.to_string(), poll(&(round + 1).to_string()));
    }
    let tool = tool
        .when_state("7", name_input("asked"))
        .when_state("asked", poll("after"))
        .when_state("after", FakeToolResponse::text("done"));
    let server = FakeMcpServer::new().with_tool(tool);
    let state = server.state();
    let (options, _user) = ElicitationScript::answering([accept_with_name("Ferris")]);
    let client = connect("fake", server, &options).await;

    call(&client, "ask", json!({})).await.unwrap();

    let arrivals = state.calls_for("ask").into_iter().map(|call| call.at).collect::<Vec<Instant>>();
    let gaps = arrivals.windows(2).map(|pair| (pair[1] - pair[0]).as_millis()).collect::<Vec<_>>();
    assert_eq!(gaps, [50, 100, 200, 400, 800, 1600, 1600, 0, 50]);
}

#[tokio::test]
async fn the_server_may_not_ask_again_once_the_user_cancels() {
    let server = FakeMcpServer::new()
        .with_tool(FakeTool::new("ask").responds(name_input("asked")).when_state("asked", name_input("asked")));
    let client = connect("fake", server, &ClientOptions::default()).await;

    let result = call(&client, "ask", json!({})).await;

    assert!(matches!(result, Err(ToolCallError::RePromptAfterCancel)), "{result:?}");
}

#[tokio::test]
async fn the_server_may_still_finish_by_polling_once_the_user_cancels() {
    let server = FakeMcpServer::new().with_tool(
        FakeTool::new("ask")
            .responds(name_input("asked"))
            .when_state("asked", poll("finishing"))
            .when_state("finishing", FakeToolResponse::text("done")),
    );
    let client = connect("fake", server, &ClientOptions::default()).await;

    let result = call(&client, "ask", json!({})).await.unwrap();

    assert_eq!(text(&result), "done");
}

#[tokio::test]
async fn answered_input_rounds_continue_up_to_the_round_limit() {
    let server = FakeMcpServer::new()
        .with_tool(FakeTool::new("ask").responds(name_input("asked")).when_state("asked", name_input("asked")));
    let state = server.state();
    let (options, _user) =
        ElicitationScript::answering(vec![ElicitResult::new(ElicitationAction::Accept); DEFAULT_MRTR_MAX_ROUNDS]);
    let client = connect("fake", server, &options).await;

    let result = call(&client, "ask", json!({})).await;

    assert!(matches!(result, Err(ToolCallError::InputRoundsExceeded)), "{result:?}");
    assert_eq!(state.calls_for("ask").len(), DEFAULT_MRTR_MAX_ROUNDS + 1);
}

#[tokio::test]
async fn form_answers_and_request_state_are_echoed_back_verbatim() {
    let server = FakeMcpServer::new().with_tool(
        FakeTool::new("ask")
            .responds(name_input(TRICKY_STATE))
            .when_state(TRICKY_STATE, FakeToolResponse::text("done")),
    );
    let state = server.state();
    let (options, _user) = ElicitationScript::answering([accept_with_name("Ferris")]);
    let client = connect("fake", server, &options).await;

    let result = call(&client, "ask", json!({})).await.unwrap();

    assert_eq!(text(&result), "done");
    let calls = state.calls_for("ask");
    assert_eq!(calls[1].request.request_state.as_deref(), Some(TRICKY_STATE));
    assert_eq!(
        calls[1].request.input_responses.as_ref().unwrap()["name"],
        json!({"action": "accept", "content": {"name": "Ferris"}})
    );
}

#[tokio::test]
async fn url_elicitations_are_answered_once_then_polled_to_completion() {
    let consent = input_requests("consent", url_request("Authorize", "https://example.com/auth", "el-1"));
    let server = FakeMcpServer::new().with_tool(
        FakeTool::new("ask")
            .responds(InputRequiredResult::new(Some(consent), Some("url-opened".to_string())))
            .when_state("url-opened", poll("polling"))
            .when_state("polling", FakeToolResponse::text("done")),
    );
    let state = server.state();
    let (options, user) = ElicitationScript::answering([ElicitResult::new(ElicitationAction::Accept)]);
    let client = connect("fake", server, &options).await;

    let result = call(&client, "ask", json!({})).await.unwrap();

    assert_eq!(text(&result), "done");
    assert_eq!(request_states(&state), [None, Some("url-opened".to_string()), Some("polling".to_string())]);
    let asked = user.captured();
    assert_eq!(asked.len(), 1, "polling must not prompt again");
    assert!(matches!(
        &asked[0].request,
        ElicitRequestParams::UrlElicitationParams { url, elicitation_id, .. }
            if url == "https://example.com/auth" && elicitation_id == "el-1"
    ));
}

#[tokio::test]
async fn progress_is_reported_across_input_rounds() {
    let server = FakeMcpServer::new().with_tool(
        FakeTool::new("ask")
            .responds(FakeToolResponse::new(name_input("asked")).progress(1.0, Some(2.0)))
            .when_state("asked", FakeToolResponse::text("done").progress(2.0, Some(2.0))),
    );
    let (options, _user) = ElicitationScript::answering([accept_with_name("Ferris")]);
    let client = connect("fake", server, &options).await;

    let events = client.call_tool("ask", Map::new(), ToolCallOptions::default()).collect::<Vec<_>>().await;

    assert_eq!(progress(&events), [1.0, 2.0]);
    assert!(matches!(events.last(), Some(ToolCallEvent::Done(Ok(result))) if text(result) == "done"));
}

#[tokio::test]
async fn created_tasks_are_polled_until_they_complete() {
    let result = task_test([working_task(TASK), completed_task(TASK, "finished")]).run().await;

    assert!(matches!(result.events.first(), Some(ToolCallEvent::TaskCreated(created)) if created.task.task_id == TASK));
    assert!(matches!(result.events.last(), Some(ToolCallEvent::Done(Ok(result))) if text(result) == "finished"));
    assert_eq!(result.state.task_get_ids(), [TASK, TASK]);
}

#[tokio::test]
async fn intermediate_task_statuses_are_reported_before_the_result() {
    let halfway =
        DetailedTask::new(task(TASK, TaskStatus::Working).with_status_message("halfway"), TaskPayload::Working);

    let result = task_test([halfway, completed_task(TASK, "finished")]).run().await;

    let status = result.events.iter().position(
        |event| matches!(event, ToolCallEvent::TaskStatus(task) if task.status_message.as_deref() == Some("halfway")),
    );
    assert_eq!(status, Some(result.events.len() - 2));
}

#[tokio::test]
async fn a_terminal_seed_is_still_fetched_for_its_result() {
    let result =
        task_test([completed_task(TASK, "already done")]).with_task(task(TASK, TaskStatus::Completed)).run().await;

    assert!(matches!(result.events.last(), Some(ToolCallEvent::Done(Ok(result))) if text(result) == "already done"));
    assert_eq!(result.state.task_get_ids(), [TASK]);
}

#[tokio::test]
async fn call_tool_forwards_progress_after_task_creation() {
    let server = FakeTask::new(TASK)
        .progress(1.0, Some(2.0))
        .states([working_task(TASK), completed_task(TASK, "finished")])
        .server("deferred");
    let client = connect("fake", server, &ClientOptions::default()).await;

    let events = client.call_tool("deferred", Map::new(), ToolCallOptions::default()).collect::<Vec<_>>().await;

    assert!(matches!(events.first(), Some(ToolCallEvent::TaskCreated(_))));
    assert_eq!(progress(&events), [1.0]);
    assert!(matches!(events.last(), Some(ToolCallEvent::Done(Ok(_)))));
}

#[tokio::test]
async fn call_tool_handles_huge_task_ttl() {
    let result = task_test([completed_task(TASK, "finished")])
        .with_task(task(TASK, TaskStatus::Working).with_ttl_ms(u64::MAX))
        .run()
        .await;

    assert!(matches!(result.events.last(), Some(ToolCallEvent::Done(Ok(_)))));
}

#[tokio::test]
async fn call_tool_handles_huge_execution_timeout() {
    let result = task_test([completed_task(TASK, "finished")]).with_timeout(Duration::MAX).run().await;

    assert!(matches!(result.events.last(), Some(ToolCallEvent::Done(Ok(_)))));
}

#[tokio::test]
async fn task_input_is_answered_through_tasks_update() {
    let answer = accept_with_name("Ada");

    let result = task_test([input_required_task(TASK, &["name"]), completed_task(TASK, "accepted")])
        .answering([answer.clone()])
        .run()
        .await;

    assert!(matches!(result.events.last(), Some(ToolCallEvent::Done(Ok(result))) if text(result) == "accepted"));
    let updates = result.state.task_updates();
    assert_eq!(updates.len(), 1);
    assert_eq!(updates[0].task_id, TASK);
    assert_eq!(updates[0].input_responses.get("name"), Some(&serde_json::to_value(&answer).unwrap()));
}

#[tokio::test]
async fn terminal_task_failures_name_their_reason() {
    let malformed = DetailedTask::new(
        task(TASK, TaskStatus::Completed),
        TaskPayload::Completed { result: serde_json::from_value(json!({ "unexpected": true })).unwrap() },
    );

    let failed = task_test([failed_task(TASK, "task exploded")]).run().await;
    let cancelled = task_test([cancelled_task(TASK)]).run().await;
    let malformed = task_test([malformed]).run().await;

    assert!(matches!(failed.failure(), TaskErrorReason::Failed { error } if error["message"] == "task exploded"));
    assert!(matches!(cancelled.failure(), TaskErrorReason::Cancelled));
    assert!(matches!(malformed.failure(), TaskErrorReason::MalformedResult(_)));
}

#[tokio::test]
async fn expired_tasks_fail_without_polling() {
    let mut seed = task(TASK, TaskStatus::Working);
    seed.created_at = "2020-01-01T00:00:00Z".to_string();
    seed.ttl_ms = Some(1);

    let result = task_test([completed_task(TASK, "too late")]).with_task(seed).run().await;

    assert!(matches!(result.failure(), TaskErrorReason::Expired));
    assert_eq!(result.state.task_get_ids(), Vec::<String>::new());
}

#[tokio::test]
async fn a_failed_poll_abandons_the_task() {
    let result = task_test([completed_task(TASK, "must not finish")]).failing_gets(1).run().await;

    assert!(matches!(result.failure(), TaskErrorReason::Get(_)));
    assert_eq!(result.state.task_get_ids(), [TASK]);
    assert_eq!(result.state.task_cancel_ids(), [TASK]);
}

#[tokio::test]
async fn a_failed_input_update_abandons_the_task() {
    let result = task_test([input_required_task(TASK, &["name"])])
        .answering([ElicitResult::new(ElicitationAction::Accept)])
        .failing_updates(1)
        .run()
        .await;

    assert!(matches!(result.failure(), TaskErrorReason::Update(_)));
    assert_eq!(result.state.task_updates().len(), 1);
    assert_eq!(result.state.task_cancel_ids(), [TASK]);
}

#[tokio::test]
async fn call_tool_cancellation_cancels_server_task_and_ends_stream() {
    let server = FakeTask::new(TASK).server("deferred");
    let state = server.state();
    let client = connect("fake", server, &ClientOptions::default()).await;
    let cancel = CancellationToken::new();
    let options = ToolCallOptions { cancel: cancel.clone(), ..ToolCallOptions::default() };
    let mut events = client.call_tool("deferred", Map::new(), options);

    assert!(matches!(events.next().await, Some(ToolCallEvent::TaskCreated(_))));
    cancel.cancel();
    let mut last = None;
    while let Some(event) = events.next().await {
        last = Some(event);
    }

    assert!(matches!(last, Some(ToolCallEvent::Done(Err(ToolCallError::Cancelled)))));
    assert_eq!(state.task_cancel_ids(), [TASK]);
}

#[tokio::test(start_paused = true)]
async fn call_tool_deadline_includes_task_elicitation() {
    let result = task_test([input_required_task(TASK, &["name"])]).with_timeout(Duration::from_millis(25)).run().await;

    assert!(matches!(result.events.last(), Some(ToolCallEvent::Done(Err(ToolCallError::TimedOut(_))))));
    assert_eq!(result.state.task_get_ids(), [TASK]);
    assert_eq!(result.state.task_cancel_ids(), [TASK]);
}

#[tokio::test]
async fn a_round_repeating_any_answered_key_is_abandoned_without_a_partial_update() {
    let result = task_test([input_required_task(TASK, &["name"]), input_required_task(TASK, &["name", "nickname"])])
        .answering([ElicitResult::new(ElicitationAction::Accept)])
        .run()
        .await;

    assert!(matches!(result.failure(), TaskErrorReason::RepeatedInput));
    assert_eq!(result.state.task_updates().len(), 1);
    assert_eq!(result.state.task_cancel_ids(), [TASK]);
}

const TASK: &str = "task-1";
const TRICKY_STATE: &str = "st.ate/with+special=chars and spaces \"quotes\"\n\ttab";

fn asking(response: impl Into<FakeToolResponse>) -> FakeMcpServer {
    FakeMcpServer::new().with_tool(FakeTool::new("ask").responds(response))
}

fn poll(state: &str) -> CallToolResponse {
    InputRequiredResult::new(None, Some(state.to_string())).into()
}

fn progress(events: &[ToolCallEvent]) -> Vec<f64> {
    events
        .iter()
        .filter_map(|event| match event {
            ToolCallEvent::Progress(progress) => Some(progress.progress),
            _ => None,
        })
        .collect()
}

fn request_states(state: &FakeMcpState) -> Vec<Option<String>> {
    state.calls_for("ask").into_iter().map(|call| call.request.request_state).collect()
}

fn text(result: &CallToolResult) -> &str {
    result.content.first().and_then(|content| content.as_text()).map_or("", |text| text.text.as_str())
}

fn tool_names(tools: Option<Result<Vec<Tool>, ClientError>>) -> Vec<String> {
    let tools = tools.expect("a tool list").expect("the server lists its tools");
    let mut names = tools.into_iter().map(|tool| tool.name.to_string()).collect::<Vec<_>>();
    names.sort();
    names
}

struct TaskTest {
    task: FakeTask,
    timeout: Option<Duration>,
    answers: Option<Vec<ElicitResult>>,
}

struct TaskTestResult {
    events: Vec<ToolCallEvent>,
    state: FakeMcpState,
}

fn task_test(states: impl IntoIterator<Item = DetailedTask>) -> TaskTest {
    TaskTest { task: FakeTask::new(TASK).states(states), timeout: None, answers: None }
}

impl TaskTest {
    fn with_task(mut self, seed: Task) -> Self {
        self.task = self.task.seed(seed);
        self
    }

    fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = Some(timeout);
        self
    }

    fn answering(mut self, answers: impl IntoIterator<Item = ElicitResult>) -> Self {
        self.answers = Some(answers.into_iter().collect());
        self
    }

    fn failing_gets(mut self, failures: usize) -> Self {
        self.task = self.task.failing_gets(failures);
        self
    }

    fn failing_updates(mut self, failures: usize) -> Self {
        self.task = self.task.failing_updates(failures);
        self
    }

    async fn run(self) -> TaskTestResult {
        let server = self.task.server("deferred");
        let state = server.state();
        let (options, _host) = if let Some(answers) = self.answers {
            let (options, script) = ElicitationScript::answering(answers);
            (options, Either::Left(script))
        } else {
            let (options, host) = elicitation_host();
            (options, Either::Right(host))
        };
        let client = connect("fake", server, &options).await;

        let options = ToolCallOptions { timeout: self.timeout, ..ToolCallOptions::default() };
        let events = client.call_tool("deferred", Map::new(), options).collect().await;
        TaskTestResult { events, state }
    }
}

impl TaskTestResult {
    fn failure(&self) -> &TaskErrorReason {
        match self.events.last() {
            Some(ToolCallEvent::Done(Err(ToolCallError::Task { task_id, reason }))) if task_id == TASK => reason,
            last => panic!("expected the task to fail, got {last:?}"),
        }
    }
}

struct Advertised {
    tasks: bool,
    elicitation: Option<ElicitationCapability>,
}

async fn advertised(options: ClientOptions) -> Advertised {
    let server = FakeMcpServer::new();
    let state = server.state();
    let _client = connect("fake", server, &options).await;
    let capabilities = state.client_capabilities().expect("capabilities are sent during discovery");
    Advertised {
        tasks: capabilities
            .extensions
            .as_ref()
            .is_some_and(|extensions| extensions.contains_key("io.modelcontextprotocol/tasks")),
        elicitation: capabilities.elicitation,
    }
}

async fn negotiated_version(server: FakeMcpServer) -> ProtocolVersion {
    let state = server.state();
    let client = connect("versioned", server, &ClientOptions::default()).await;
    client.list_tools().await.expect("list tools");
    state.protocol_version().expect("tools/list carries a protocol version")
}

struct ElicitationTest {
    server: FakeMcpServer,
}

impl ElicitationTest {
    fn new() -> Self {
        let server = FakeMcpServer::new().with_tool(
            FakeTool::new("ask").responds(name_input("asked")).when_state("asked", FakeToolResponse::text("done")),
        );
        Self { server }
    }

    fn server(&self) -> FakeMcpServer {
        self.server.clone()
    }

    fn answer(&self) -> serde_json::Value {
        let calls = self.server.state().calls_for("ask");
        calls.last().and_then(|call| call.request.input_responses.as_ref()).expect("the tool was answered")["name"]
            .clone()
    }
}

#[derive(Clone)]
struct CompletionServer;

impl ServerHandler for CompletionServer {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
    }

    async fn call_tool(
        &self,
        _request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, ErrorData> {
        let completion =
            CustomNotification::new("notifications/elicitation/complete", Some(json!({ "elicitationId": "oauth-1" })));
        context.peer.send_notification(ServerNotification::CustomNotification(completion)).await.ok();
        Ok(CallToolResult::success(Vec::new()).into())
    }
}
