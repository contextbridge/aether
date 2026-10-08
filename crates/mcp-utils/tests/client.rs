use futures::StreamExt;
use futures::future::Either;
use mcp_utils::client::{
    CancellationToken, ClientOptions, Elicitation, McpClient, TaskErrorReason, ToolCallError, ToolCallEvent,
    ToolCallOptions, Transport,
};
use mcp_utils::model::{
    CallToolRequestParams, CallToolResponse, CallToolResult, ContentBlock, CreateTaskResult, CustomNotification,
    DEFAULT_MRTR_MAX_ROUNDS, DetailedTask, DiscoverResult, ElicitRequest, ElicitRequestParams, ElicitResult,
    ElicitationAction, ElicitationCapability, ErrorCode, ErrorData, FormElicitationCapability, Implementation,
    InitializeRequestParams, InitializeResult, InputRequest, InputRequests, InputRequiredResult, ListToolsResult,
    PaginatedRequestParams, ProtocolVersion, ServerCapabilities, ServerConfig, ServerNotification, Task, TaskPayload,
    TaskStatus, UrlElicitationCapability,
};
use mcp_utils::server::McpServer;
use mcp_utils::testing::{
    ElicitationScript, FakeMcpServer, FakeMcpState, FakeTool, FakeToolResponse, args, completed_task_payload, connect,
};
use rmcp::{
    RoleServer, ServerHandler,
    service::{NotificationContext, RequestContext},
};
use serde_json::{Map, json};
use std::borrow::Cow;
use std::future::{Ready, ready};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{mpsc, watch};

#[tokio::test]
async fn connect_prefers_stateless_discovery_for_modern_servers() {
    let negotiated = Negotiated::default();
    let client =
        connect("versioned", McpServer::new(ModernOnlyServer(negotiated.clone())), &ClientOptions::default()).await;

    client.list_tools().await.expect("list tools");

    assert_eq!(negotiated.observed().await, ProtocolVersion::V_2026_07_28);
}

#[tokio::test]
async fn connect_selects_an_older_mutually_supported_revision() {
    let negotiated = Negotiated::default();
    let client =
        connect("versioned", McpServer::new(June2025Server(negotiated.clone())), &ClientOptions::default()).await;

    client.list_tools().await.expect("list tools");

    assert_eq!(negotiated.observed().await, ProtocolVersion::V_2025_06_18);
}

#[tokio::test]
async fn connect_falls_back_to_legacy_initialization() {
    let negotiated = Negotiated::default();

    let _client =
        connect("versioned", McpServer::new(LegacyOnlyServer(negotiated.clone())), &ClientOptions::default()).await;

    assert_eq!(negotiated.observed().await, ProtocolVersion::V_2025_11_25);
}

#[tokio::test]
async fn exposes_the_server_name_description_and_instructions() {
    let client = connect("fake", FakeMcpServer::new(), &ClientOptions::default()).await;

    assert_eq!(client.name(), "fake");
    assert_eq!(client.description().as_deref(), Some("A fake MCP server for testing"));
    assert_eq!(client.instructions().as_deref(), Some("A fake MCP server for testing"));
    assert!(client.list_tools().await.unwrap().iter().any(|tool| tool.name == "add_numbers"));
}

#[tokio::test]
async fn servers_without_prompts_list_none() {
    let client = connect("fake", FakeMcpServer::new(), &ClientOptions::default()).await;

    assert!(client.list_prompts().await.unwrap().is_empty());
}

#[tokio::test]
async fn tool_calls_resolve_to_their_result() {
    let client = connect("fake", FakeMcpServer::new(), &ClientOptions::default()).await;

    let result =
        client.call_tool("add_numbers", args(json!({"a": 2, "b": 3})), ToolCallOptions::default()).result().await;

    assert_eq!(result.unwrap().structured_content, Some(json!({"sum": 5})));
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
    let with_sink = advertised(ClientOptions::default().elicitation(events.clone())).await;
    let url = ElicitationCapability::new().with_url(UrlElicitationCapability::new());
    let url_only =
        advertised(ClientOptions::default().elicitation(events.clone()).elicitation_capability(url.clone())).await;
    let neither =
        advertised(ClientOptions::default().elicitation(events).elicitation_capability(ElicitationCapability::new()))
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

    client.call_tool("ask", Map::new(), ToolCallOptions::default()).result().await.unwrap();

    assert_eq!(test.answer(), json!({"action": "cancel"}));
}

#[tokio::test]
async fn dropping_an_elicitation_answers_cancel() {
    let test = ElicitationTest::new();
    let (events, mut host) = mpsc::channel(1);
    let client = connect("fake", test.server(), &ClientOptions::default().elicitation(events)).await;
    let host = tokio::spawn(async move {
        let Some(Elicitation::Request(elicitation)) = host.recv().await else { panic!("expected an elicitation") };
        drop(elicitation);
    });

    client.call_tool("ask", Map::new(), ToolCallOptions::default()).result().await.unwrap();
    host.await.unwrap();

    assert_eq!(test.answer(), json!({"action": "cancel"}));
}

#[tokio::test]
async fn elicitation_responses_reach_the_server_and_name_their_source() {
    let test = ElicitationTest::new();
    let (events, mut host) = mpsc::channel(1);
    let client = connect("fake", test.server(), &ClientOptions::default().elicitation(events)).await;
    let host = tokio::spawn(async move {
        let Some(Elicitation::Request(elicitation)) = host.recv().await else { panic!("expected an elicitation") };
        let server = elicitation.server.clone();
        elicitation.respond(ElicitResult::new(ElicitationAction::Accept).with_content(json!({"name": "Ferris"})));
        server
    });

    client.call_tool("ask", Map::new(), ToolCallOptions::default()).result().await.unwrap();

    assert_eq!(host.await.unwrap(), "fake");
    assert_eq!(test.answer(), json!({"action": "accept", "content": {"name": "Ferris"}}));
}

#[tokio::test]
async fn elicitation_completion_notifications_name_their_source() {
    let (events, mut host) = mpsc::channel(1);
    let client =
        connect("linear", McpServer::new(CompletionServer), &ClientOptions::default().elicitation(events)).await;

    client.call_tool("complete", Map::new(), ToolCallOptions::default()).result().await.unwrap();

    let event = host.recv().await.expect("completion event");
    assert!(matches!(event, Elicitation::Complete { server, id } if server == "linear" && id == "oauth-1"));
}

#[tokio::test]
async fn input_required_without_requests_or_state_aborts() {
    let server = asking(CallToolResponse::from(InputRequiredResult::new(None, None)));
    let client = connect("fake", server, &ClientOptions::default()).await;

    let result = client.call_tool("ask", Map::new(), ToolCallOptions::default()).result().await;

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

    let result = client.call_tool("ask", Map::new(), ToolCallOptions::default()).result().await.unwrap();

    assert_eq!(text(&result), "done");
    assert_eq!(request_states(&state), [None, Some("first".to_string()), Some("second".to_string())]);
}

#[tokio::test]
async fn the_server_may_not_ask_again_once_the_user_cancels() {
    let server = FakeMcpServer::new()
        .with_tool(FakeTool::new("ask").responds(name_input("asked")).when_state("asked", name_input("asked")));
    let client = connect("fake", server, &ClientOptions::default()).await;

    let result = client.call_tool("ask", Map::new(), ToolCallOptions::default()).result().await;

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

    let result = client.call_tool("ask", Map::new(), ToolCallOptions::default()).result().await.unwrap();

    assert_eq!(text(&result), "done");
}

#[tokio::test]
async fn answered_input_rounds_continue_up_to_the_round_limit() {
    let server = FakeMcpServer::new()
        .with_tool(FakeTool::new("ask").responds(name_input("asked")).when_state("asked", name_input("asked")));
    let state = server.state();
    let (events, host) = mpsc::channel(1);
    let _user =
        ElicitationScript::spawn(host, vec![ElicitResult::new(ElicitationAction::Accept); DEFAULT_MRTR_MAX_ROUNDS]);
    let client = connect("fake", server, &ClientOptions::default().elicitation(events)).await;

    let result = client.call_tool("ask", Map::new(), ToolCallOptions::default()).result().await;

    assert!(matches!(result, Err(ToolCallError::InputRoundsExceeded)), "{result:?}");
    assert_eq!(state.calls_for("ask").len(), DEFAULT_MRTR_MAX_ROUNDS + 1);
}

#[tokio::test]
async fn call_tool_drives_created_task_to_completion() {
    let result = task_test([completed_task()]).run().await;

    assert!(
        matches!(result.events.first(), Some(ToolCallEvent::TaskCreated(created)) if created.task.task_id == "task-1")
    );
    assert!(matches!(
        result.events.last(),
        Some(ToolCallEvent::Done { task: Some(task), result: Ok(result) })
            if task.task_id == "task-1" && text(result) == "finished"
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
        .with_task("task-1", [DetailedTask::new(task(TaskStatus::Working), TaskPayload::Working), completed_task()]);
    let client = connect("fake", server, &ClientOptions::default()).await;

    let events = client
        .call_tool("deferred", Map::new(), ToolCallOptions::with_timeout(Duration::from_secs(1)))
        .collect::<Vec<_>>()
        .await;

    assert!(matches!(events.first(), Some(ToolCallEvent::TaskCreated(_))));
    assert!(events.iter().any(|event| matches!(
        event,
        ToolCallEvent::Progress(progress)
            if (progress.progress - 1.0).abs() < f64::EPSILON
                && progress.total.is_some_and(|total| (total - 2.0).abs() < f64::EPSILON)
    )));
    assert!(matches!(events.last(), Some(ToolCallEvent::Done { task: Some(_), result: Ok(_) })));
}

#[tokio::test]
async fn call_tool_handles_huge_task_ttl() {
    let result = task_test([completed_task()]).with_task(task(TaskStatus::Working).with_ttl_ms(u64::MAX)).run().await;

    assert!(matches!(result.events.last(), Some(ToolCallEvent::Done { task: Some(_), result: Ok(_) })));
}

#[tokio::test]
async fn call_tool_handles_huge_execution_timeout() {
    let result = task_test([completed_task()]).with_timeout(Duration::MAX).run().await;

    assert!(matches!(result.events.last(), Some(ToolCallEvent::Done { task: Some(_), result: Ok(_) })));
}

#[tokio::test]
async fn call_tool_cancellation_cancels_server_task_and_ends_stream() {
    let seed = task(TaskStatus::Working);
    let server = FakeMcpServer::new()
        .with_tool(FakeTool::new("deferred").responds(FakeToolResponse::task(CreateTaskResult::new(seed.clone()))))
        .with_task("task-1", [DetailedTask::new(seed, TaskPayload::Working)]);
    let state = server.state();
    let client = connect("fake", server, &ClientOptions::default()).await;
    let cancel = CancellationToken::new();
    let options = ToolCallOptions { cancel: cancel.clone(), ..ToolCallOptions::with_timeout(Duration::from_secs(5)) };
    let mut events = client.call_tool("deferred", Map::new(), options);

    assert!(matches!(events.next().await, Some(ToolCallEvent::TaskCreated(_))));
    cancel.cancel();
    let mut last = None;
    while let Some(event) = events.next().await {
        last = Some(event);
    }

    assert!(matches!(last, Some(ToolCallEvent::Done { task: Some(task), result: Err(ToolCallError::Cancelled) })
        if task.task_id == "task-1"));
    assert_eq!(state.task_cancel_ids(), ["task-1"]);
}

#[tokio::test]
async fn call_tool_deadline_includes_task_elicitation() {
    let result = task_test([input_required_task()]).with_timeout(Duration::from_millis(25)).run().await;

    assert!(matches!(
        result.events.last(),
        Some(ToolCallEvent::Done { task: Some(_), result: Err(ToolCallError::TimedOut(_)) })
    ));
    assert_eq!(result.state.task_cancel_ids(), ["task-1"]);
}

#[tokio::test]
async fn call_tool_abandons_tasks_that_repeat_answered_input_requests() {
    let result = task_test([input_required_task(), input_required_task()])
        .answering([ElicitResult::new(ElicitationAction::Accept)])
        .run()
        .await;

    assert!(matches!(
        result.events.last(),
        Some(ToolCallEvent::Done { task: Some(_), result: Err(ToolCallError::Task { reason, .. }) })
            if matches!(**reason, TaskErrorReason::RepeatedInput)
    ));
    assert_eq!(result.state.task_updates().len(), 1);
    assert_eq!(result.state.task_cancel_ids(), ["task-1"]);
}

#[tokio::test]
async fn unix_transport_connects_to_a_hosted_socket() {
    let handle = McpServer::new(FakeMcpServer::new()).serve_unix().unwrap();

    let client = McpClient::connect("fake", Transport::Unix(handle.path().to_path_buf()), &ClientOptions::default())
        .await
        .unwrap();

    assert!(client.list_tools().await.unwrap().iter().any(|tool| tool.name == "divide_numbers"));
}

fn asking(response: impl Into<FakeToolResponse>) -> FakeMcpServer {
    FakeMcpServer::new().with_tool(FakeTool::new("ask").responds(response))
}

fn name_input(state: &str) -> CallToolResponse {
    InputRequiredResult::new(Some(name_request()), Some(state.to_string())).into()
}

fn poll(state: &str) -> CallToolResponse {
    InputRequiredResult::new(None, Some(state.to_string())).into()
}

fn name_request() -> InputRequests {
    let request = ElicitRequest::new(ElicitRequestParams::FormElicitationParams {
        meta: None,
        message: "Name?".to_string(),
        requested_schema: serde_json::from_value(json!({"type": "object", "properties": {}})).unwrap(),
    });
    InputRequests::from([("name".to_string(), InputRequest::Elicitation(request))])
}

fn request_states(state: &FakeMcpState) -> Vec<Option<String>> {
    state.calls_for("ask").into_iter().map(|call| call.request.request_state).collect()
}

fn text(result: &CallToolResult) -> &str {
    result.content.first().and_then(|content| content.as_text()).map_or("", |text| text.text.as_str())
}

struct TaskTest {
    seed: Task,
    states: Vec<DetailedTask>,
    timeout: Duration,
    answers: Option<Vec<ElicitResult>>,
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
        answers: None,
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

    fn answering(mut self, answers: impl IntoIterator<Item = ElicitResult>) -> Self {
        self.answers = Some(answers.into_iter().collect());
        self
    }

    async fn run(self) -> TaskTestResult {
        let task_id = self.seed.task_id.clone();
        let server = FakeMcpServer::new()
            .with_tool(FakeTool::new("deferred").responds(FakeToolResponse::task(CreateTaskResult::new(self.seed))))
            .with_task(task_id, self.states);
        let state = server.state();
        let (host_events, host) = mpsc::channel(4);
        let _host = match self.answers {
            Some(answers) => Either::Left(ElicitationScript::spawn(host, answers)),
            None => Either::Right(host),
        };
        let client = connect("fake", server, &ClientOptions::default().elicitation(host_events)).await;

        let events =
            client.call_tool("deferred", Map::new(), ToolCallOptions::with_timeout(self.timeout)).collect().await;
        TaskTestResult { events, state }
    }
}

fn completed_task() -> DetailedTask {
    let result = CallToolResult::success(vec![ContentBlock::text("finished")]);
    DetailedTask::new(task(TaskStatus::Completed), completed_task_payload(result))
}

fn input_required_task() -> DetailedTask {
    DetailedTask::new(task(TaskStatus::InputRequired), TaskPayload::InputRequired { input_requests: name_request() })
}

fn task(status: TaskStatus) -> Task {
    let now = chrono::Utc::now().to_rfc3339();
    Task::new("task-1", status, now.clone(), now).with_poll_interval_ms(10)
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

#[derive(Clone)]
struct Negotiated(Arc<watch::Sender<Option<ProtocolVersion>>>);

impl Default for Negotiated {
    fn default() -> Self {
        Self(Arc::new(watch::channel(None).0))
    }
}

impl Negotiated {
    async fn observed(&self) -> ProtocolVersion {
        let mut observed = self.0.subscribe();
        observed.wait_for(Option::is_some).await.expect("recorder alive").clone().expect("checked by wait_for")
    }

    fn record(&self, version: Option<ProtocolVersion>) {
        self.0.send_replace(version);
    }

    fn list_tools(&self, context: &RequestContext<RoleServer>) -> Ready<Result<ListToolsResult, ErrorData>> {
        self.record(context.protocol_version());
        ready(Ok(ListToolsResult::with_all_items(Vec::new())))
    }
}

#[derive(Clone, Default)]
struct ModernOnlyServer(Negotiated);

impl ServerHandler for ModernOnlyServer {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new("modern-only", "1.0.0"))
            .with_protocol_version(ProtocolVersion::V_2026_07_28)
    }

    fn supported_protocol_versions(&self) -> Cow<'static, [ProtocolVersion]> {
        Cow::Owned(vec![ProtocolVersion::V_2026_07_28])
    }

    fn initialize(
        &self,
        _request: InitializeRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> impl Future<Output = Result<InitializeResult, ErrorData>> + Send + '_ {
        ready(Err(ErrorData::new(ErrorCode::METHOD_NOT_FOUND, "initialize is not supported", None)))
    }

    fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        context: RequestContext<RoleServer>,
    ) -> impl Future<Output = Result<ListToolsResult, ErrorData>> + Send + '_ {
        self.0.list_tools(&context)
    }
}

#[derive(Clone, Default)]
struct June2025Server(Negotiated);

impl ServerHandler for June2025Server {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new("older-revision", "1.0.0"))
            .with_protocol_version(ProtocolVersion::V_2025_06_18)
    }

    fn supported_protocol_versions(&self) -> Cow<'static, [ProtocolVersion]> {
        Cow::Owned(vec![ProtocolVersion::V_2025_06_18])
    }

    fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        context: RequestContext<RoleServer>,
    ) -> impl Future<Output = Result<ListToolsResult, ErrorData>> + Send + '_ {
        self.0.list_tools(&context)
    }
}

#[derive(Clone, Default)]
struct LegacyOnlyServer(Negotiated);

impl ServerHandler for LegacyOnlyServer {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new("legacy", "1.0.0"))
            .with_protocol_version(ProtocolVersion::V_2025_11_25)
    }

    fn discover(
        &self,
        _context: RequestContext<RoleServer>,
    ) -> impl Future<Output = Result<DiscoverResult, ErrorData>> + Send + '_ {
        ready(Err(ErrorData::new(ErrorCode::METHOD_NOT_FOUND, "server/discover is not supported", None)))
    }

    fn on_initialized(&self, context: NotificationContext<RoleServer>) -> impl Future<Output = ()> + Send + '_ {
        self.0.record(context.peer.peer_info().map(|info| info.protocol_version.clone()));
        ready(())
    }
}
