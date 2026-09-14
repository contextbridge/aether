use acp::schema::v2::{
    CreateElicitationRequest, ElicitationFormMode, ElicitationSchema, ElicitationSessionScope, InitializeRequest,
    PromptRequest,
};
use acp_utils::client::{AcpClientError, AcpEvent, connect_acp_client};
use acp_utils::testing::{FakeAgent, duplex_pair, initialize_request};
use agent_client_protocol::{self as acp, Agent};
use tokio::sync::mpsc::unbounded_channel;
use tokio::task::{LocalSet, spawn_local};

#[tokio::test]
async fn dropping_the_last_handle_closes_the_event_stream_and_transport() {
    LocalSet::new()
        .run_until(async {
            let (agent_transport, client_transport) = duplex_pair();
            let agent = FakeAgent::default().agent();
            let server = spawn_local(agent.connect_to(agent_transport));
            let mut client = connect_acp_client(client_transport, initialize_request()).await.unwrap();
            let retained = client.handle.clone();
            drop(client.handle);
            drop(retained);
            assert!(matches!(client.event_rx.recv().await, Some(AcpEvent::ConnectionClosed)));
            assert!(client.event_rx.recv().await.is_none());
            let _ = server.await.unwrap();
        })
        .await;
}

#[tokio::test]
async fn initialization_failure_releases_the_connection_while_initialize_is_held_open() {
    LocalSet::new()
        .run_until(async {
            for failure in ["caller dropped", "rejected", "eof"] {
                let (agent_transport, client_transport) = duplex_pair();
                let (init_tx, mut init_rx) = unbounded_channel();
                let agent = Agent.v2().on_receive_request(
                    async move |_: InitializeRequest, responder, _cx| {
                        init_tx.send(responder).unwrap();
                        Ok(())
                    },
                    acp::on_receive_request!(),
                );
                let server = spawn_local(agent.connect_to(agent_transport));
                let connecting = spawn_local(connect_acp_client(client_transport, initialize_request()));
                let responder = init_rx.recv().await.unwrap();
                match failure {
                    "caller dropped" => {
                        connecting.abort();
                        assert!(matches!(connecting.await, Err(error) if error.is_cancelled()));
                    }
                    "rejected" => {
                        responder.respond_with_error(acp::Error::invalid_params()).unwrap();
                        assert!(connecting.await.unwrap().is_err());
                    }
                    _ => {
                        server.abort();
                        assert!(connecting.await.unwrap().is_err());
                    }
                }
                let _ = server.await;
            }
        })
        .await;
}

#[tokio::test]
async fn ordinary_requests_fail_on_disconnect_and_eof_even_with_retained_handles() {
    use acp::schema::v2::ListSessionsRequest;

    LocalSet::new()
        .run_until(async {
            for disconnect in [true, false] {
                let (agent_transport, client_transport) = duplex_pair();
                let (agent, mut requests) = FakeAgent::default().hold_list_sessions(true).capture();
                let server = spawn_local(agent.agent().connect_to(agent_transport));
                let mut client = connect_acp_client(client_transport, initialize_request()).await.unwrap();
                let retained = client.handle.clone();
                let handle = retained.clone();
                let request = spawn_local(async move { handle.request(ListSessionsRequest::new()).await });
                let (_, responder) = requests.list_sessions.recv().await.unwrap();
                let prompt = retained.prompt(PromptRequest::new("live", vec![]));
                let (_, prompt_responder) = requests.prompt.recv().await.unwrap();

                if disconnect {
                    client.handle.disconnect().await;
                } else {
                    server.abort();
                }
                assert!(matches!(client.event_rx.recv().await, Some(AcpEvent::ConnectionClosed)));
                assert!(matches!(request.await.unwrap(), Err(AcpClientError::Protocol(_))));
                assert!(matches!(prompt.await, Err(AcpClientError::Protocol(_))));
                assert!(client.event_rx.recv().await.is_none());
                assert!(retained.request(ListSessionsRequest::new()).await.is_err());
                retained.disconnect().await;
                drop((responder, prompt_responder));
                let _ = server.await;
            }
        })
        .await;
}

#[tokio::test]
async fn disconnect_drops_connection_before_the_ui_releases_a_pending_approval() {
    LocalSet::new()
        .run_until(async {
            let (agent_transport, client_transport) = duplex_pair();
            let (agent, mut requests) = FakeAgent::default().capture();
            let server = spawn_local(agent.agent().connect_to(agent_transport));
            let mut client = connect_acp_client(client_transport, initialize_request()).await.unwrap();
            let connection = requests.connection.recv().await.unwrap();
            let prompt = client.handle.prompt(PromptRequest::new("live", vec![]));
            let (request, prompt_responder) = requests.prompt.recv().await.unwrap();
            let approval = spawn_local(async move {
                connection
                    .send_request(CreateElicitationRequest::new(
                        ElicitationFormMode::new(
                            ElicitationSessionScope::new(request.session_id),
                            ElicitationSchema::new(),
                        ),
                        "Continue?",
                    ))
                    .block_task()
                    .await
            });
            let Some(AcpEvent::ElicitationRequest { responder, .. }) = client.event_rx.recv().await else {
                panic!("approval must be pending before detach");
            };
            client.handle.disconnect().await;
            assert!(matches!(client.event_rx.recv().await, Some(AcpEvent::ConnectionClosed)));
            assert!(prompt.await.is_err());
            client.handle.disconnect().await;
            assert!(client.event_rx.recv().await.is_none());
            drop(responder);
            assert!(approval.await.unwrap().is_err(), "detaching must not answer the approval");
            drop(prompt_responder);
            let _ = server.await.expect("agent connection task must not panic");
            assert!(requests.cancel.try_recv().is_err(), "disconnect must not send session/cancel");
            assert!(requests.close_session.try_recv().is_err(), "disconnect must not send session/close");
        })
        .await;
}
