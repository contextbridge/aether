use acp::schema::ProtocolVersion;
use acp::schema::v2::{
    CancelSessionNotification, CloseSessionRequest, CreateElicitationRequest, ElicitationFormMode, ElicitationSchema,
    ElicitationSessionScope, Implementation, InitializeRequest, InitializeResponse, PromptRequest,
};
use acp_utils::client::{AcpEvent, connect_acp_client};
use acp_utils::testing::duplex_pair;
use agent_client_protocol::{self as acp, Agent};
use tokio::task::{LocalSet, spawn_local};

#[tokio::test]
async fn disconnect_drops_connection_before_the_ui_releases_a_pending_approval() {
    LocalSet::new()
        .run_until(async {
            let (agent_transport, client_transport) = duplex_pair();
            let agent = Agent
                .v2()
                .on_receive_request(
                    async |_: InitializeRequest, responder, _cx| {
                        responder.respond(InitializeResponse::new(
                            ProtocolVersion::V2,
                            Implementation::new("test-agent", "0.0.0"),
                        ))
                    },
                    acp::on_receive_request!(),
                )
                .on_receive_request(
                    async |request: PromptRequest, responder, cx| {
                        let input = CreateElicitationRequest::new(
                            ElicitationFormMode::new(
                                ElicitationSessionScope::new(request.session_id),
                                ElicitationSchema::new(),
                            ),
                            "Continue?",
                        );
                        let connection = cx.clone();
                        cx.spawn(async move {
                            let result = connection.send_request(input).block_task().await;
                            assert!(result.is_err(), "detaching must not answer the approval");
                            drop(responder);
                            Ok(())
                        })?;
                        Ok(())
                    },
                    acp::on_receive_request!(),
                )
                .on_receive_notification(
                    async |_: CancelSessionNotification, _cx| -> Result<(), acp::Error> {
                        panic!("disconnect must not send session/cancel");
                    },
                    acp::on_receive_notification!(),
                )
                .on_receive_request(
                    async |_: CloseSessionRequest, _responder, _cx| -> Result<(), acp::Error> {
                        panic!("disconnect must not send session/close");
                    },
                    acp::on_receive_request!(),
                );
            let server = spawn_local(agent.connect_to(agent_transport));
            let mut client = connect_acp_client(
                client_transport,
                InitializeRequest::new(ProtocolVersion::V2, Implementation::new("test-client", "0.0.0")),
            )
            .await
            .unwrap();
            let handle = client.handle.clone();
            let prompt = spawn_local(async move { handle.prompt(PromptRequest::new("live", vec![])).await });
            let Some(AcpEvent::ElicitationRequest { responder, .. }) = client.event_rx.recv().await else {
                panic!("approval must be pending before detach");
            };
            client.handle.disconnect().await;
            assert!(matches!(client.event_rx.recv().await, Some(AcpEvent::ConnectionClosed)));
            assert!(prompt.await.unwrap().is_err());
            client.handle.disconnect().await;
            drop(responder);
            let _ = server.await.expect("agent connection task must not panic");
        })
        .await;
}
