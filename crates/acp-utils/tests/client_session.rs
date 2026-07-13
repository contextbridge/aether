use acp_utils::client::spawn_acp_session;
use acp_utils::testing::duplex_pair;
use agent_client_protocol::schema::{
    CancelNotification, Implementation, InitializeRequest, InitializeResponse, NewSessionRequest, NewSessionResponse,
    PromptRequest, ProtocolVersion, SessionId, SetSessionConfigOptionRequest,
};
use agent_client_protocol::{self as acp, Agent};
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::Notify;
use tokio::task::{LocalSet, spawn_local};

#[tokio::test(flavor = "current_thread")]
async fn cancel_reaches_the_agent_while_a_config_response_is_outstanding() {
    LocalSet::new()
        .run_until(async {
            let (agent_transport, client_transport) = duplex_pair();
            let cancelled = Arc::new(Notify::new());

            let agent_builder = Agent
                .builder()
                .on_receive_request(
                    async |_req: InitializeRequest, responder, _cx| {
                        responder.respond(
                            InitializeResponse::new(ProtocolVersion::V1)
                                .agent_info(Implementation::new("Fake Agent", "0.0.0")),
                        )
                    },
                    acp::on_receive_request!(),
                )
                .on_receive_request(
                    async |_req: NewSessionRequest, responder, _cx| {
                        responder.respond(NewSessionResponse::new(SessionId::new("sess-1")))
                    },
                    acp::on_receive_request!(),
                )
                .on_receive_request(
                    // Never answer, so the prompt stays in flight for the whole test.
                    async |_req: PromptRequest, responder, _cx| {
                        std::mem::forget(responder);
                        Ok(())
                    },
                    acp::on_receive_request!(),
                )
                .on_receive_request(
                    // Never answer, so the config response stays outstanding when Cancel is sent.
                    async |_req: SetSessionConfigOptionRequest, responder, _cx| {
                        std::mem::forget(responder);
                        Ok(())
                    },
                    acp::on_receive_request!(),
                )
                .on_receive_notification(
                    {
                        let cancelled = Arc::clone(&cancelled);
                        async move |_n: CancelNotification, _cx| {
                            cancelled.notify_one();
                            Ok(())
                        }
                    },
                    acp::on_receive_notification!(),
                );
            spawn_local(async move {
                let _ = agent_builder.connect_to(agent_transport).await;
            });

            let session = spawn_acp_session(
                client_transport,
                InitializeRequest::new(ProtocolVersion::V1),
                NewSessionRequest::new(PathBuf::from("/tmp")),
            )
            .await
            .expect("session establishes");

            session.prompt_handle.prompt(&session.session_id, "hi", None).expect("prompt queues");
            session.prompt_handle.set_config_option(&session.session_id, "mode", "Plan").expect("config queues");
            session.prompt_handle.cancel(&session.session_id).expect("cancel queues");

            cancelled.notified().await;
        })
        .await;
}
