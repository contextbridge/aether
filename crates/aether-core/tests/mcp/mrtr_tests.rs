//! End-to-end tests for the Phase 2.1 MRTR (multi round-trip request) client
//! executor. Every test drives the public API: the `mcp()` builder, the
//! `McpCommand::ExecuteTool` command, and the `ToolExecutionEvent` stream.
//! Fake in-memory servers script `InputRequiredResult` responses and record
//! exactly what the client sent on every round.

use aether_core::events::TraceContext;
use aether_core::mcp::mcp;
use aether_core::mcp::run_mcp_task::{McpCommand, ToolExecutionEvent};
use mcp_utils::client::{McpClientEvent, McpServer, McpTransport};
use mcp_utils::display_meta::ToolResultMeta;
use rmcp::{
    RoleServer, ServerHandler,
    model::{
        CallToolRequestParams, CallToolResponse, CallToolResult, CancelledNotification, CancelledNotificationParam,
        ElicitRequest, ElicitRequestParams, ElicitResult, ElicitationAction, ErrorData, Implementation, InputRequest,
        InputRequests, InputRequiredResult, ListToolsResult, PaginatedRequestParams, ProgressNotification,
        ProgressNotificationParam, RequestId, ServerCapabilities, ServerInfo, ServerNotification, Tool,
    },
    service::{DynService, MaybeSendFuture, NotificationContext, RequestContext},
};
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::{mpsc, oneshot};

/// The bounded MRTR round limit the executor must enforce.
const EXPECTED_ROUND_LIMIT: usize = 8;

// Fake servers

/// What kind of server-initiated input request the fake server includes in an
/// `InputRequiredResult`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum InputRequestKind {
    Form,
    Url,
    Sampling,
    None,
}

/// What the fake server observed on one `tools/call`.
#[derive(Debug, Clone, PartialEq, Eq)]
struct ReceivedCall {
    round: usize,
    input_responses: Option<Value>,
    request_state: Option<String>,
    traceparent: Option<String>,
    tracestate: Option<String>,
}

/// Scriptable MRTR server. Returns `InputRequiredResult` for the first
/// `rounds` calls (with `states[round]` as opaque request state), then
/// completes, echoing the final request's `requestState` and `inputResponses`.
#[derive(Clone)]
struct ScriptedMrtrServer {
    rounds: usize,
    states: Vec<String>,
    kind: InputRequestKind,
    received: Arc<Mutex<Vec<ReceivedCall>>>,
    progress_before: bool,
    progress_after: bool,
}

impl ScriptedMrtrServer {
    fn new(rounds: usize, states: Vec<String>) -> Self {
        Self {
            rounds,
            states,
            kind: InputRequestKind::Form,
            received: Arc::new(Mutex::new(Vec::new())),
            progress_before: false,
            progress_after: false,
        }
    }

    fn with_kind(mut self, kind: InputRequestKind) -> Self {
        self.kind = kind;
        self
    }

    fn with_progress(mut self, before: bool, after: bool) -> Self {
        self.progress_before = before;
        self.progress_after = after;
        self
    }

    fn into_dyn(self) -> Box<dyn DynService<RoleServer>> {
        Box::new(self)
    }
}

impl ServerHandler for ScriptedMrtrServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new("mrtr-form", "0.1.0"))
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, ErrorData> {
        let input_schema = serde_json::from_value(json!({ "type": "object", "properties": {} })).unwrap();
        Ok(ListToolsResult::with_all_items(vec![Tool::new("ask", "MRTR test tool", Arc::new(input_schema))]))
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, ErrorData> {
        let round = {
            let mut received = self.received.lock().unwrap();
            let round = received.len();
            received.push(ReceivedCall {
                round,
                input_responses: request
                    .input_responses
                    .as_ref()
                    .map(|responses| serde_json::to_value(responses).expect("input responses serialize")),
                request_state: request.request_state.clone(),
                traceparent: context.meta.0.get("traceparent").and_then(Value::as_str).map(str::to_string),
                tracestate: context.meta.0.get("tracestate").and_then(Value::as_str).map(str::to_string),
            });
            round
        };

        if self.progress_before && round < self.rounds {
            send_progress(&context, 0.25).await;
        }

        if round < self.rounds {
            let requests = self.input_requests(round);
            let state = self.states.get(round).cloned();
            return Ok(CallToolResponse::InputRequired(InputRequiredResult::new(requests, state)));
        }

        if self.progress_after {
            send_progress(&context, 1.0).await;
        }

        Ok(CallToolResponse::Complete(CallToolResult::structured(json!({
            "status": "done",
            "round": round,
            "request_state": request.request_state,
            "input_responses": request.input_responses,
        }))))
    }
}

impl ScriptedMrtrServer {
    fn input_requests(&self, round: usize) -> Option<InputRequests> {
        let request = match self.kind {
            InputRequestKind::Form => form_input_request(&format!("Approve round {round}?")),
            InputRequestKind::Url => url_input_request("Authorize", "https://example.com/mrtr/auth"),
            InputRequestKind::Sampling => sampling_input_request(),
            InputRequestKind::None => return None,
        };
        let mut requests = InputRequests::new();
        requests.insert(format!("round-{round}"), request);
        Some(requests)
    }
}

/// Fake server that cancels the in-flight request via `notifications/cancelled`
/// and then returns a (dropped) result.
#[derive(Clone, Default)]
struct CancellingMrtrServer {
    reason: String,
}

impl CancellingMrtrServer {
    fn new(reason: impl Into<String>) -> Self {
        Self { reason: reason.into() }
    }

    fn into_dyn(self) -> Box<dyn DynService<RoleServer>> {
        Box::new(self)
    }
}

impl ServerHandler for CancellingMrtrServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new("mrtr-cancel", "0.1.0"))
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, ErrorData> {
        let input_schema = serde_json::from_value(json!({ "type": "object", "properties": {} })).unwrap();
        Ok(ListToolsResult::with_all_items(vec![Tool::new("ask", "MRTR cancel tool", Arc::new(input_schema))]))
    }

    async fn call_tool(
        &self,
        _request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, ErrorData> {
        let _ = context
            .peer
            .send_notification(ServerNotification::CancelledNotification(CancelledNotification::new(
                CancelledNotificationParam::new(Some(context.id.clone()), Some(self.reason.clone())),
            )))
            .await;
        Ok(CallToolResponse::Complete(CallToolResult::success(vec![])))
    }
}

/// Fake server that holds one `tools/call` open until the test releases it,
/// recording the in-flight request id and any client-side cancellation
/// notification for that request.
struct GatedMrtrServer {
    call_seen: mpsc::Sender<RequestId>,
    release: Arc<Mutex<Option<oneshot::Receiver<()>>>>,
    cancellations: mpsc::Sender<CancelledNotificationParam>,
}

impl GatedMrtrServer {
    fn new(
        call_seen: mpsc::Sender<RequestId>,
        release: oneshot::Receiver<()>,
        cancellations: mpsc::Sender<CancelledNotificationParam>,
    ) -> Self {
        Self { call_seen, release: Arc::new(Mutex::new(Some(release))), cancellations }
    }

    fn into_dyn(self) -> Box<dyn DynService<RoleServer>> {
        Box::new(self)
    }
}

impl ServerHandler for GatedMrtrServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new("mrtr-gated", "0.1.0"))
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, ErrorData> {
        let input_schema = serde_json::from_value(json!({ "type": "object", "properties": {} })).unwrap();
        Ok(ListToolsResult::with_all_items(vec![Tool::new("ask", "MRTR gated tool", Arc::new(input_schema))]))
    }

    async fn call_tool(
        &self,
        _request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, ErrorData> {
        let _ = self.call_seen.send(context.id.clone()).await;
        let release = self.release.lock().unwrap().take().expect("a single gated tool call");
        let _ = release.await;
        Ok(CallToolResponse::Complete(CallToolResult::success(vec![])))
    }

    fn on_cancelled(
        &self,
        notification: CancelledNotificationParam,
        _context: NotificationContext<RoleServer>,
    ) -> impl std::future::Future<Output = ()> + MaybeSendFuture + '_ {
        let cancellations = self.cancellations.clone();
        async move {
            let _ = cancellations.send(notification).await;
        }
    }
}

// Input request wire shapes

fn form_input_request(message: &str) -> InputRequest {
    serde_json::from_value(json!({
        "method": "elicitation/create",
        "params": {
            "mode": "form",
            "message": message,
            "requestedSchema": {
                "type": "object",
                "properties": { "reason": { "type": "string" } },
                "required": ["reason"]
            }
        }
    }))
    .expect("valid form input request")
}

fn url_input_request(message: &str, url: &str) -> InputRequest {
    // rmcp 3.0 still requires its removed legacy id field; the boundary helper
    // supplies a placeholder that Aether never reads or asserts on.
    InputRequest::Elicitation(ElicitRequest::new(acp_utils::elicitation::rmcp_url_elicitation(message, url)))
}

fn sampling_input_request() -> InputRequest {
    serde_json::from_value(json!({
        "method": "sampling/createMessage",
        "params": {
            "messages": [{ "role": "user", "content": { "type": "text", "text": "hi" } }],
            "maxTokens": 10
        }
    }))
    .expect("valid sampling input request")
}

async fn send_progress(context: &RequestContext<RoleServer>, progress: f64) {
    if let Some(token) = context.meta.get_progress_token() {
        let _ = context
            .peer
            .send_notification(ServerNotification::ProgressNotification(ProgressNotification::new(
                ProgressNotificationParam::new(token, progress),
            )))
            .await;
    }
}

// Test harness helpers

/// Everything observed while executing one tool call through the public API.
struct ToolRun {
    result: Result<llm::ToolCallResult, llm::ToolCallError>,
    result_meta: Option<ToolResultMeta>,
    progress: Vec<ProgressNotificationParam>,
}

fn tool_request(id: &str) -> llm::ToolCallRequest {
    llm::ToolCallRequest { id: id.to_string(), name: "mrtr__ask".to_string(), arguments: "{}".to_string() }
}

async fn run_tool(
    command_tx: &mpsc::Sender<McpCommand>,
    request: llm::ToolCallRequest,
    trace_context: Option<TraceContext>,
    timeout: Duration,
) -> ToolRun {
    let (event_tx, mut event_rx) = mpsc::channel(100);
    command_tx.send(McpCommand::ExecuteTool { request, trace_context, timeout, tx: event_tx }).await.unwrap();

    let mut progress = Vec::new();
    let (result, result_meta) = loop {
        match event_rx.recv().await {
            Some(ToolExecutionEvent::Progress { progress: p, .. }) => progress.push(p),
            Some(ToolExecutionEvent::Complete { result, result_meta, .. }) => break (result, result_meta),
            None => panic!("event stream ended without Complete"),
        }
    };
    ToolRun { result, result_meta, progress }
}

/// What the client handler/UI channel captured for one elicitation.
#[derive(Debug, Clone)]
struct CapturedElicitation {
    server_name: String,
    request: ElicitRequestParams,
}

/// Drains `event_rx`, captures every elicitation, and answers them from
/// `answers` in order. When `answers` runs out, the current request is held
/// open without a response so the executor's deadline can fire.
fn spawn_elicitation_script(
    mut event_rx: mpsc::Receiver<McpClientEvent>,
    answers: Vec<ElicitResult>,
) -> mpsc::Receiver<CapturedElicitation> {
    let (captured_tx, captured_rx) = mpsc::channel(16);
    tokio::spawn(async move {
        let mut answers = answers.into_iter();
        let mut held: Option<mcp_utils::client::ElicitationRequest> = None;
        while let Some(event) = event_rx.recv().await {
            if let McpClientEvent::Elicitation(req) = event {
                let _ = captured_tx
                    .send(CapturedElicitation { server_name: req.server_name.clone(), request: req.request.clone() })
                    .await;
                if let Some(result) = answers.next() {
                    let _ = req.response_sender.send(result);
                } else if held.is_none() {
                    held = Some(req);
                }
            }
        }
    });
    captured_rx
}

async fn spawn_server(server: Box<dyn DynService<RoleServer>>) -> aether_core::mcp::McpSpawnResult {
    let config = McpServer::new("mrtr", McpTransport::InMemory { server }, false);
    let mut spawn = mcp("/workspace").with_servers(vec![config]).spawn().await.unwrap();
    let _snapshot = spawn.block_until_ready().await.expect("bootstrap completes");
    spawn
}

/// Yield until `condition` holds. Avoids wall-clock sleeps while still bound:
/// a correct implementation notices within a few polls, so the loop only spins
/// when a regression leaves the condition permanently false.
async fn wait_until(mut condition: impl FnMut() -> bool) {
    for _ in 0..10_000 {
        if condition() {
            return;
        }
        tokio::task::yield_now().await;
    }
    panic!("condition never became true");
}

// Single-round cases

#[tokio::test]
async fn single_round_form_elicitation_completes_and_round_trips_opaque_state() {
    let server = ScriptedMrtrServer::new(1, vec!["eyJyb3VuZCI6MX0.sig-α".to_string()]);
    let received = server.received.clone();
    let mut spawn = spawn_server(server.into_dyn()).await;
    let mut script_rx = spawn_elicitation_script(
        std::mem::replace(&mut spawn.event_rx, mpsc::channel(1).1),
        vec![ElicitResult::new(ElicitationAction::Accept).with_content(json!({ "reason": "approved" }))],
    );

    let run = run_tool(&spawn.command_tx, tool_request("single-form"), None, Duration::from_secs(10)).await;

    let result = run.result.expect("single round of input should complete");
    assert!(result.result.contains("status: done"), "unexpected result: {}", result.result);
    assert!(run.result_meta.is_none(), "no display metadata was returned");

    {
        let calls = received.lock().unwrap();
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0].request_state, None, "initial call carries no request state");
        assert_eq!(calls[0].input_responses, None);
        assert_eq!(
            calls[1].request_state.as_deref(),
            Some("eyJyb3VuZCI6MX0.sig-α"),
            "opaque state must round-trip byte-exact"
        );
        assert_eq!(
            calls[1].input_responses,
            Some(json!({ "round-0": { "action": "accept", "content": { "reason": "approved" } } })),
            "collected input response must be forwarded verbatim"
        );
    }

    let captured = script_rx.recv().await.expect("an elicitation was dispatched");
    assert_eq!(captured.server_name, "mrtr");
    match captured.request {
        ElicitRequestParams::FormElicitationParams { message, .. } => {
            assert_eq!(message, "Approve round 0?");
        }
        other => panic!("expected form elicitation, got {other:?}"),
    }
}

#[tokio::test]
async fn single_round_url_elicitation_resolves_through_ui_channel() {
    let server = ScriptedMrtrServer::new(1, vec!["url-state".to_string()]).with_kind(InputRequestKind::Url);
    let mut spawn = spawn_server(server.into_dyn()).await;
    let mut script_rx = spawn_elicitation_script(
        std::mem::replace(&mut spawn.event_rx, mpsc::channel(1).1),
        vec![ElicitResult::new(ElicitationAction::Accept)],
    );

    let run = run_tool(&spawn.command_tx, tool_request("single-url"), None, Duration::from_secs(10)).await;

    let result = run.result.expect("url elicitation round should complete");
    assert!(result.result.contains("status: done"), "unexpected result: {}", result.result);

    let captured = script_rx.recv().await.expect("a URL elicitation was dispatched");
    match captured.request {
        ElicitRequestParams::UrlElicitationParams { url, .. } => {
            assert_eq!(url, "https://example.com/mrtr/auth");
        }
        other => panic!("expected url elicitation, got {other:?}"),
    }
}

#[tokio::test]
async fn multi_round_elicitation_accumulates_responses_and_uses_latest_state() {
    let server = ScriptedMrtrServer::new(2, vec!["state-1".to_string(), "state-2".to_string()]);
    let received = server.received.clone();
    let mut spawn = spawn_server(server.into_dyn()).await;
    let mut script_rx = spawn_elicitation_script(
        std::mem::replace(&mut spawn.event_rx, mpsc::channel(1).1),
        vec![
            ElicitResult::new(ElicitationAction::Accept).with_content(json!({ "reason": "first" })),
            ElicitResult::new(ElicitationAction::Accept).with_content(json!({ "reason": "second" })),
        ],
    );

    let run = run_tool(&spawn.command_tx, tool_request("multi-round"), None, Duration::from_secs(10)).await;

    let result = run.result.expect("two rounds of input should complete");
    assert!(result.result.contains("status: done"), "unexpected result: {}", result.result);

    let calls = received.lock().unwrap();
    assert_eq!(calls.len(), 3);
    assert_eq!(calls[1].request_state.as_deref(), Some("state-1"));
    assert_eq!(calls[2].request_state.as_deref(), Some("state-2"), "retry must carry the latest opaque state");
    assert_eq!(
        calls[2].input_responses,
        Some(json!({
            "round-0": { "action": "accept", "content": { "reason": "first" } },
            "round-1": { "action": "accept", "content": { "reason": "second" } }
        })),
        "responses from every round must be collected and forwarded"
    );

    let mut captured = Vec::new();
    while let Ok(c) = script_rx.try_recv() {
        captured.push(c);
    }
    assert_eq!(captured.len(), 2, "both rounds must reach the UI channel");
}

#[tokio::test]
async fn missing_request_state_is_tolerated_and_retried_without_one() {
    let server = ScriptedMrtrServer::new(1, vec![]);
    let received = server.received.clone();
    let mut spawn = spawn_server(server.into_dyn()).await;
    let _script_rx = spawn_elicitation_script(
        std::mem::replace(&mut spawn.event_rx, mpsc::channel(1).1),
        vec![ElicitResult::new(ElicitationAction::Accept).with_content(json!({ "reason": "ok" }))],
    );

    let run = run_tool(&spawn.command_tx, tool_request("no-state"), None, Duration::from_secs(10)).await;

    run.result.expect("a state-less input round should still complete");
    let calls = received.lock().unwrap();
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[1].request_state, None, "retry must not invent a request state");
    assert!(calls[1].input_responses.is_some());
}

// Decline / cancel forwarding

#[tokio::test]
async fn decline_is_forwarded_to_the_server_not_converted() {
    let server = ScriptedMrtrServer::new(1, vec!["decline-state".to_string()]);
    let received = server.received.clone();
    let mut spawn = spawn_server(server.into_dyn()).await;
    let _script_rx = spawn_elicitation_script(
        std::mem::replace(&mut spawn.event_rx, mpsc::channel(1).1),
        vec![ElicitResult::new(ElicitationAction::Decline)],
    );

    let run = run_tool(&spawn.command_tx, tool_request("decline"), None, Duration::from_secs(10)).await;

    let result = run.result.expect("a decline response is forwarded and the server decides the outcome");
    assert!(result.result.contains("status: done"), "unexpected result: {}", result.result);
    let calls = received.lock().unwrap();
    assert_eq!(
        calls[1].input_responses,
        Some(json!({ "round-0": { "action": "decline" } })),
        "decline must reach the server as an input response"
    );
}

#[tokio::test]
async fn cancel_is_forwarded_to_the_server_not_converted() {
    let server = ScriptedMrtrServer::new(1, vec!["cancel-state".to_string()]);
    let received = server.received.clone();
    let mut spawn = spawn_server(server.into_dyn()).await;
    let _script_rx = spawn_elicitation_script(
        std::mem::replace(&mut spawn.event_rx, mpsc::channel(1).1),
        vec![ElicitResult::new(ElicitationAction::Cancel)],
    );

    let run = run_tool(&spawn.command_tx, tool_request("cancel"), None, Duration::from_secs(10)).await;

    let result = run.result.expect("a cancel response is forwarded and the server decides the outcome");
    assert!(result.result.contains("status: done"), "unexpected result: {}", result.result);
    let calls = received.lock().unwrap();
    assert_eq!(
        calls[1].input_responses,
        Some(json!({ "round-0": { "action": "cancel" } })),
        "cancel must reach the server as an input response"
    );
}

// Progress and trace metadata across rounds

#[tokio::test]
async fn progress_is_delivered_before_and_after_an_mrtr_round() {
    let server = ScriptedMrtrServer::new(1, vec!["prog-state".to_string()]).with_progress(true, true);
    let mut spawn = spawn_server(server.into_dyn()).await;
    let _script_rx = spawn_elicitation_script(
        std::mem::replace(&mut spawn.event_rx, mpsc::channel(1).1),
        vec![ElicitResult::new(ElicitationAction::Accept).with_content(json!({ "reason": "ok" }))],
    );

    let run = run_tool(&spawn.command_tx, tool_request("progress"), None, Duration::from_secs(10)).await;

    run.result.expect("tool completes after the input round");
    let values: Vec<f64> = run.progress.iter().map(|p| p.progress).collect();
    assert_eq!(values, vec![0.25, 1.0], "progress must flow on both sides of the MRTR round");
}

#[tokio::test]
async fn trace_metadata_is_attached_to_every_retry() {
    let trace = TraceContext {
        traceparent: "00-00112233445566778899aabbccddeeff-0123456789abcdef-01".to_string(),
        tracestate: Some("vendor=value".to_string()),
    };
    let server = ScriptedMrtrServer::new(2, vec!["t1".to_string(), "t2".to_string()]);
    let received = server.received.clone();
    let mut spawn = spawn_server(server.into_dyn()).await;
    let _script_rx = spawn_elicitation_script(
        std::mem::replace(&mut spawn.event_rx, mpsc::channel(1).1),
        vec![
            ElicitResult::new(ElicitationAction::Accept).with_content(json!({ "reason": "a" })),
            ElicitResult::new(ElicitationAction::Accept).with_content(json!({ "reason": "b" })),
        ],
    );

    let run = run_tool(&spawn.command_tx, tool_request("trace"), Some(trace), Duration::from_secs(10)).await;

    run.result.expect("tool completes");
    let calls = received.lock().unwrap();
    assert_eq!(calls.len(), 3);
    for call in calls.iter() {
        assert_eq!(
            call.traceparent.as_deref(),
            Some("00-00112233445566778899aabbccddeeff-0123456789abcdef-01"),
            "every round must carry trace metadata; saw: {calls:?}"
        );
        assert_eq!(call.tracestate.as_deref(), Some("vendor=value"));
    }
}

// Failure cases

#[tokio::test]
async fn excessive_rounds_stops_at_the_bounded_limit() {
    let server = ScriptedMrtrServer::new(usize::MAX, vec!["never".to_string()]);
    let received = server.received.clone();
    let mut spawn = spawn_server(server.into_dyn()).await;
    let mut script_rx = spawn_elicitation_script(
        std::mem::replace(&mut spawn.event_rx, mpsc::channel(1).1),
        vec![
            ElicitResult::new(ElicitationAction::Accept).with_content(json!({ "reason": "ok" }));
            EXPECTED_ROUND_LIMIT
        ],
    );

    let run = run_tool(&spawn.command_tx, tool_request("round-limit"), None, Duration::from_secs(10)).await;

    let err = run.result.expect_err("a never-completing server must hit the round limit");
    assert!(err.error.contains("rounds"), "expected round-limit error, got: {}", err.error);
    assert!(
        err.error.contains(&EXPECTED_ROUND_LIMIT.to_string()),
        "expected the round limit {} in the error, got: {}",
        EXPECTED_ROUND_LIMIT,
        err.error
    );

    let calls = received.lock().unwrap();
    assert_eq!(calls.len(), EXPECTED_ROUND_LIMIT + 1, "initial call plus one retry per allowed input round, then stop");
    drop(calls);

    let mut answered = 0;
    while let Ok(_c) = script_rx.try_recv() {
        answered += 1;
    }
    assert_eq!(answered, EXPECTED_ROUND_LIMIT, "each allowed input round must resolve through the UI channel");
}

#[tokio::test]
async fn eight_input_rounds_then_success_allows_the_final_retry() {
    let server = ScriptedMrtrServer::new(EXPECTED_ROUND_LIMIT, vec!["boundary".to_string(); EXPECTED_ROUND_LIMIT]);
    let received = server.received.clone();
    let mut spawn = spawn_server(server.into_dyn()).await;
    let mut script_rx = spawn_elicitation_script(
        std::mem::replace(&mut spawn.event_rx, mpsc::channel(1).1),
        vec![
            ElicitResult::new(ElicitationAction::Accept).with_content(json!({ "reason": "ok" }));
            EXPECTED_ROUND_LIMIT
        ],
    );

    let run = run_tool(&spawn.command_tx, tool_request("eight-rounds-success"), None, Duration::from_secs(10)).await;

    let result = run.result.expect("the retry after exactly 8 input rounds must be allowed and may complete");
    assert!(result.result.contains("status: done"), "unexpected result: {}", result.result);

    let calls = received.lock().unwrap();
    assert_eq!(calls.len(), EXPECTED_ROUND_LIMIT + 1, "initial call plus one retry per input round");
    assert_eq!(calls.last().unwrap().round, EXPECTED_ROUND_LIMIT);
    drop(calls);

    let mut answered = 0;
    while let Ok(_c) = script_rx.try_recv() {
        answered += 1;
    }
    assert_eq!(answered, EXPECTED_ROUND_LIMIT, "exactly 8 input rounds are resolved");
}

#[tokio::test]
async fn ninth_input_required_fails_before_resolving_the_ninth_input() {
    let server = ScriptedMrtrServer::new(EXPECTED_ROUND_LIMIT + 1, vec!["never".to_string(); EXPECTED_ROUND_LIMIT + 1]);
    let received = server.received.clone();
    let mut spawn = spawn_server(server.into_dyn()).await;
    let mut script_rx = spawn_elicitation_script(
        std::mem::replace(&mut spawn.event_rx, mpsc::channel(1).1),
        vec![
            ElicitResult::new(ElicitationAction::Accept).with_content(json!({ "reason": "ok" }));
            EXPECTED_ROUND_LIMIT
        ],
    );

    let run = run_tool(&spawn.command_tx, tool_request("ninth-round-fails"), None, Duration::from_secs(10)).await;

    let err = run.result.expect_err("a ninth input request must fail the operation");
    assert!(err.error.contains("rounds"), "expected round-limit error, got: {}", err.error);

    let calls = received.lock().unwrap();
    assert_eq!(calls.len(), EXPECTED_ROUND_LIMIT + 1, "initial call plus 8 retries; the ninth request is the last");
    drop(calls);

    let mut answered = 0;
    while let Ok(_c) = script_rx.try_recv() {
        answered += 1;
    }
    assert_eq!(answered, EXPECTED_ROUND_LIMIT, "the ninth input request must not be resolved");
}

#[tokio::test]
async fn protocol_mismatch_when_input_required_carries_neither_requests_nor_state() {
    let server = ScriptedMrtrServer::new(1, vec![]).with_kind(InputRequestKind::None);
    let spawn = spawn_server(server.into_dyn()).await;

    let run = run_tool(&spawn.command_tx, tool_request("bad-input-required"), None, Duration::from_secs(10)).await;

    let err = run.result.expect_err("InputRequiredResult with no requests and no state is a protocol mismatch");
    assert!(
        err.error.contains("protocol") || err.error.contains("neither"),
        "expected a protocol error, got: {}",
        err.error
    );
}

#[tokio::test]
async fn unsupported_input_request_is_reported_as_invalid_response() {
    let server = ScriptedMrtrServer::new(1, vec!["s".to_string()]).with_kind(InputRequestKind::Sampling);
    let spawn = spawn_server(server.into_dyn()).await;

    let run = run_tool(&spawn.command_tx, tool_request("sampling"), None, Duration::from_secs(10)).await;

    let err = run.result.expect_err("Aether cannot fulfill sampling input requests");
    assert!(
        err.error.contains("only elicitation/create is supported"),
        "expected an invalid-input-response error, got: {}",
        err.error
    );
}

#[tokio::test]
async fn timeout_while_the_form_is_open_fails_the_whole_operation() {
    let server = ScriptedMrtrServer::new(1, vec!["slow-user".to_string()]);
    let mut spawn = spawn_server(server.into_dyn()).await;
    let _script_rx = spawn_elicitation_script(
        std::mem::replace(&mut spawn.event_rx, mpsc::channel(1).1),
        vec![], // never answer: the executor's deadline must fire while awaiting input
    );

    let run = run_tool(&spawn.command_tx, tool_request("timeout"), None, Duration::from_millis(300)).await;

    let err = run.result.expect_err("the single deadline must cover the input wait");
    assert!(err.error.contains("timed out"), "expected a timeout error, got: {}", err.error);
}

#[tokio::test]
async fn server_cancellation_is_reported_as_cancelled() {
    let server = CancellingMrtrServer::new("user pressed stop");
    let spawn = spawn_server(server.into_dyn()).await;

    let run = run_tool(&spawn.command_tx, tool_request("cancelled"), None, Duration::from_secs(10)).await;

    let err = run.result.expect_err("a cancelled request must surface as a cancellation error");
    assert!(err.error.contains("cancelled"), "expected a cancellation error, got: {}", err.error);
    assert!(err.error.contains("user pressed stop"), "reason must be preserved, got: {}", err.error);
}

#[tokio::test]
async fn accepted_form_response_must_carry_object_shaped_content() {
    let server = ScriptedMrtrServer::new(1, vec!["bad-form".to_string()]);
    let received = server.received.clone();
    let mut spawn = spawn_server(server.into_dyn()).await;
    let _script_rx = spawn_elicitation_script(
        std::mem::replace(&mut spawn.event_rx, mpsc::channel(1).1),
        vec![ElicitResult::new(ElicitationAction::Accept).with_content(json!("not-an-object"))],
    );

    let run = run_tool(&spawn.command_tx, tool_request("malformed-accept"), None, Duration::from_secs(10)).await;

    let err = run.result.expect_err("a malformed accepted response must fail the operation");
    assert!(err.error.contains("invalid MRTR input response"), "got: {}", err.error);
    assert!(err.error.contains("object"), "got: {}", err.error);
    assert_eq!(received.lock().unwrap().len(), 1, "no retry may carry a malformed response");
}

#[tokio::test]
async fn dropping_the_event_receiver_cancels_while_awaiting_input() {
    let server = ScriptedMrtrServer::new(usize::MAX, vec!["hold-state".to_string()]);
    let received = server.received.clone();
    let mut spawn = spawn_server(server.into_dyn()).await;

    let (event_tx, event_rx) = mpsc::channel(8);
    let mut client_events = std::mem::replace(&mut spawn.event_rx, mpsc::channel(1).1);
    let elicitation_task = tokio::spawn(async move {
        while let Some(event) = client_events.recv().await {
            if let McpClientEvent::Elicitation(req) = event {
                return req;
            }
        }
        panic!("server must dispatch an elicitation");
    });

    spawn
        .command_tx
        .send(McpCommand::ExecuteTool {
            request: tool_request("consumer-cancel-input"),
            trace_context: None,
            timeout: Duration::from_mins(1),
            tx: event_tx,
        })
        .await
        .unwrap();

    let elicitation = elicitation_task.await.expect("elicitation task completed");
    let response_sender = elicitation.response_sender;

    drop(event_rx);

    wait_until(|| response_sender.is_closed()).await;
    assert_eq!(received.lock().unwrap().len(), 1, "no retry may be attempted after the consumer cancels");
}

#[tokio::test]
async fn dropping_the_event_receiver_cancels_an_in_flight_request() {
    let (call_seen_tx, mut call_seen_rx) = mpsc::channel(1);
    let (release_tx, release_rx) = oneshot::channel();
    let (cancel_tx, mut cancel_rx) = mpsc::channel(1);
    let server = GatedMrtrServer::new(call_seen_tx, release_rx, cancel_tx);
    let spawn = spawn_server(server.into_dyn()).await;

    let (event_tx, event_rx) = mpsc::channel(8);
    spawn
        .command_tx
        .send(McpCommand::ExecuteTool {
            request: tool_request("consumer-cancel-inflight"),
            trace_context: None,
            timeout: Duration::from_mins(1),
            tx: event_tx,
        })
        .await
        .unwrap();

    let request_id = call_seen_rx.recv().await.expect("the request reaches the server");

    drop(event_rx);

    let cancelled = cancel_rx.recv().await.expect("the server observes the client cancellation");
    assert_eq!(cancelled.request_id.as_ref(), Some(&request_id), "cancellation targets the in-flight request");
    assert!(cancelled.reason.is_some(), "cancellation must carry a reason");

    let _ = release_tx.send(());
}
