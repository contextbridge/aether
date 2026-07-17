use acp_utils::client::AcpEvent;
use acp_utils::notifications::WorkspaceMoveTarget;
use acp_utils::testing::{fake_agent_session, next_event_matching};
use agent_client_protocol::schema::{PromptResponse, StopReason};
use tokio::task::LocalSet;

#[tokio::test(flavor = "current_thread")]
async fn lifecycle_commands_stay_rejected_while_an_overlapping_prompt_is_in_flight() {
    LocalSet::new()
        .run_until(async {
            let (mut agent, mut session) = fake_agent_session().await;

            session.prompt_handle.prompt(&session.session_id, "first", None).expect("first prompt queues");
            let first = agent.next_prompt_responder().await;

            session.prompt_handle.prompt(&session.session_id, "second", None).expect("second prompt queues");
            // Keep the responder alive so the second prompt stays in flight.
            let _second = agent.next_prompt_responder().await;

            first.respond(PromptResponse::new(StopReason::EndTurn)).expect("first prompt completes");
            next_event_matching(&mut session.event_rx, |e| matches!(e, AcpEvent::PromptDone(_))).await;

            session
                .prompt_handle
                .move_workspace(&session.session_id, WorkspaceMoveTarget::New { name: "ws".into() })
                .expect("move workspace queues");
            let outcome = next_event_matching(&mut session.event_rx, |e| {
                matches!(e, AcpEvent::WorkspaceMoved(_) | AcpEvent::WorkspaceMoveFailed { .. })
            })
            .await;
            match outcome {
                AcpEvent::WorkspaceMoveFailed { error } => assert_eq!(error, "a prompt is in flight"),
                _ => panic!("workspace move must be rejected while a prompt is still in flight"),
            }
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn prompt_reaches_the_agent_while_another_prompt_is_outstanding() {
    LocalSet::new()
        .run_until(async {
            let (mut agent, session) = fake_agent_session().await;

            session.prompt_handle.prompt(&session.session_id, "first", None).expect("first prompt queues");
            // Never answered, so the first prompt stays in flight for the whole test.
            let _first = agent.next_prompt_responder().await;

            session.prompt_handle.prompt(&session.session_id, "second", None).expect("second prompt queues");
            session.prompt_handle.cancel(&session.session_id).expect("cancel queues");
            agent.next_cancellation().await;

            let _second = agent.try_next_prompt_responder().expect("second prompt reaches agent before later commands");
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn cancel_reaches_the_agent_while_a_config_response_is_outstanding() {
    LocalSet::new()
        .run_until(async {
            let (mut agent, session) = fake_agent_session().await;

            // Neither the prompt nor the config request is answered, so both
            // stay outstanding when Cancel is sent.
            session.prompt_handle.prompt(&session.session_id, "hi", None).expect("prompt queues");
            session.prompt_handle.set_config_option(&session.session_id, "mode", "Plan").expect("config queues");
            session.prompt_handle.cancel(&session.session_id).expect("cancel queues");

            agent.next_cancellation().await;
        })
        .await;
}
