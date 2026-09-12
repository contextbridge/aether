use acp_utils::notifications::{McpNotification, McpServerStatus, McpServerStatusEntry};
use acp_utils::testing::{
    TestPeer, duplex_pair, idle_notification, initialize_request, initialize_response, plan_notification,
    running_notification, test_connection,
};
use agent_client_protocol::schema::ProtocolVersion;
use agent_client_protocol::schema::v2::{
    ContentBlock, ContentChunk, InitializeRequest, PlanEntry, PlanEntryPriority, PlanEntryStatus, SessionUpdate,
    StopReason, TextContent, UpdateSessionNotification,
};
use agent_client_protocol::{self as acp, Agent, ByteStreams, Client};
use serde_json::json;
use tokio::io::AsyncWriteExt;
use tokio::task::{LocalSet, spawn_local};
use tokio_util::compat::{TokioAsyncReadCompatExt, TokioAsyncWriteCompatExt};

#[tokio::test]
async fn duplex_initialization_uses_v2_info() {
    LocalSet::new()
        .run_until(async {
            let (agent_transport, client_transport) = duplex_pair();
            let server = spawn_local(
                Agent
                    .builder()
                    .on_receive_request(
                        async |request: InitializeRequest, responder, _cx| {
                            assert_eq!(request.protocol_version, ProtocolVersion::V2);
                            assert_eq!(request.info.name, "test-client");
                            responder.respond(initialize_response())
                        },
                        acp::on_receive_request!(),
                    )
                    .connect_to(agent_transport),
            );
            Client
                .builder()
                .connect_with(client_transport, async |cx| {
                    let response = cx.send_request(initialize_request()).block_task().await?;
                    assert_eq!(response.protocol_version, ProtocolVersion::V2);
                    assert_eq!(response.info.name, "test-agent");
                    Ok(())
                })
                .await
                .unwrap();
            let _ = server.await.unwrap();
        })
        .await;
}

#[tokio::test]
async fn test_peer_roundtrips_v2_updates_and_extensions() {
    LocalSet::new()
        .run_until(async {
            let (cx, mut peer) = test_connection().await;
            let updates = [
                running_notification("session"),
                UpdateSessionNotification::new(
                    "session",
                    SessionUpdate::AgentMessageChunk(ContentChunk::new(
                        ContentBlock::Text(TextContent::new("hello")),
                        "message-1",
                    )),
                ),
                plan_notification(
                    "session",
                    "plan-1",
                    vec![PlanEntry::new("Implement v2", PlanEntryPriority::High, PlanEntryStatus::InProgress)],
                ),
                idle_notification("session", Some(StopReason::EndTurn)),
                idle_notification("session", None),
            ];
            for update in updates {
                cx.send_notification(update.clone()).unwrap();
                assert_eq!(peer.next_session_notification().await, update);
            }
            let notification = McpNotification::ServerStatus {
                servers: vec![McpServerStatusEntry::new("tools", McpServerStatus::Connected { tool_count: 2 })],
            };
            cx.send_notification(notification.clone()).unwrap();
            assert_eq!(peer.next_mcp_notification().await, notification);
        })
        .await;
}

#[tokio::test]
async fn stdio_accepts_json_rpc_batch_notifications() {
    LocalSet::new().run_until(async {
        let (mut writer, reader) = tokio::io::duplex(4096);
        let (output, _output_reader) = tokio::io::duplex(4096);
        let (mut peer, builder) = TestPeer::new();
        let connection = spawn_local(builder.connect_to(ByteStreams::new(output.compat_write(), reader.compat())));
        let batch = json!([
            {"jsonrpc": "2.0", "method": "session/update", "params": {
                "sessionId": "session", "update": {"sessionUpdate": "state_update", "state": "running"}
            }},
            {"jsonrpc": "2.0", "method": "session/update", "params": {
                "sessionId": "session", "update": {"sessionUpdate": "state_update", "state": "idle", "stopReason": "end_turn"}
            }}
        ]);
        writer.write_all(format!("{batch}\n").as_bytes()).await.unwrap();
        writer.shutdown().await.unwrap();
        assert_eq!(peer.next_session_notification().await, running_notification("session"));
        assert_eq!(peer.next_session_notification().await, idle_notification("session", Some(StopReason::EndTurn)));
        let _ = connection.await.unwrap();
    }).await;
}
