use aether_cli::acp::testing::AcpTestHarness;
use agent_client_protocol::mcp_server::McpServer;
use agent_client_protocol::v2::schema::{
    CancelSessionNotification, SessionId, SessionUpdate, StateUpdate, StopReason, ToolCallStatus,
};
use agent_client_protocol::{Agent, V2, v2};
use agent_client_protocol_rmcp::rmcp_3::McpServerExt;
use foxfire_acp::mcp_status::McpServerStatus;
use llm::testing::{FakeLlmProvider, llm_response};
use mcp_utils::testing::{FakeMcpServer, FakeTool, FakeToolResponse, Gate};

#[tokio::test(flavor = "current_thread")]
async fn cancelling_the_turn_drops_the_remote_call() {
    AcpTestHarness::run(|mut harness| async move {
        let gate = Gate::new();
        let server = probe_server(FakeToolResponse::text("never returned").gated(gate.clone()));
        let id = open(&harness, server, 1).await;
        harness.prompt(&id, "try the tool").await.unwrap();
        gate.reached().await;
        harness.client_cx.send_notification(CancelSessionNotification::new(id.clone())).unwrap();
        harness.expect_idle(&id, StopReason::Cancelled).await;
        gate.drained().await;
        harness.shutdown().await;
    })
    .await;
}

#[tokio::test(flavor = "current_thread")]
async fn resume_rediscovers_and_subscribes_to_the_replacement_server() {
    AcpTestHarness::run(|mut harness| async move {
        let original = probe_server(FakeToolResponse::text("first"));
        let replacement = probe_server(FakeToolResponse::text("second")).with_tool(FakeTool::new("extra"));
        let id = open(&harness, original.clone(), 2).await;
        call_probe(&mut harness, &id).await;
        harness.disconnect().await;
        harness.reconnect().await;
        client(&harness)
            .resume_session(id.clone(), "/tmp")
            .with_mcp_server(host(replacement.clone()))
            .unwrap()
            .start_session()
            .block_task()
            .await
            .unwrap();
        harness.updates_until(&id, is_idle).await;
        assert_eq!(next_connected_count(&mut harness).await, 2, "resume advertised the previous client's tools");
        replacement.state().add_tool_and_notify(FakeTool::new("later")).await;
        while next_connected_count(&mut harness).await != 3 {}
        call_probe(&mut harness, &id).await;
        assert_eq!(original.state().calls_for("probe").len(), 1);
        assert_eq!(replacement.state().calls_for("probe").len(), 1);
        harness.shutdown().await;
    })
    .await;
}

async fn next_connected_count(harness: &mut AcpTestHarness) -> usize {
    loop {
        let statuses = harness.next_mcp_server_statuses().await;
        match &statuses.iter().find(|s| s.name == "client").unwrap().status {
            McpServerStatus::Connecting => {}
            McpServerStatus::Connected { tool_count } => return *tool_count,
            status => panic!("expected a connected client MCP server, got {status:?}"),
        }
    }
}

async fn call_probe(harness: &mut AcpTestHarness, id: &SessionId) {
    harness.prompt(id, "try the tool").await.unwrap();
    assert!(harness.updates_until(id, is_idle).await.iter().any(
        |u| matches!(u, SessionUpdate::ToolCallUpdate(t) if t.status.value() == Some(&ToolCallStatus::Completed))
    ));
}

async fn open(harness: &AcpTestHarness, server: FakeMcpServer, turns: usize) -> SessionId {
    let turn = [
        llm_response().tool_call("probe-call", "client__probe", &["{}"]).build(),
        llm_response().text(&["done"]).build(),
    ];
    harness.script_llm(FakeLlmProvider::new(turn.iter().cycle().take(turns * turn.len()).cloned().collect()));
    client(harness)
        .build_session("/tmp")
        .with_mcp_server(host(server))
        .unwrap()
        .start_session()
        .block_task()
        .await
        .unwrap()
        .session
        .session_id()
        .clone()
}

fn probe_server(response: FakeToolResponse) -> FakeMcpServer {
    FakeMcpServer::new().with_tool(FakeTool::new("probe").responds(response))
}
fn host(server: FakeMcpServer) -> McpServer<Agent> {
    McpServer::from_rmcp("client", move || server.clone())
}
fn client(harness: &AcpTestHarness) -> v2::ConnectionTo<Agent> {
    harness.client_cx.clone().into_protocol::<V2>().unwrap()
}
fn is_idle(update: &SessionUpdate) -> bool {
    matches!(update, SessionUpdate::StateUpdate(StateUpdate::Idle(_)))
}
