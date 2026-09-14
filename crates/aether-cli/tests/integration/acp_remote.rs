mod client;
mod websocket;

use aether_cli::acp::testing::AcpTestHarness;
use aether_core::core::agent;
use aether_core::events::{AgentEvent, MessageEvent, TurnEvent, TurnOutcome};
use aether_sessions::{SessionEvent, UserEvent};
use agent_client_protocol::schema::v2::{
    AbsolutePath, CloseSessionRequest, ContentBlock, NewSessionRequest, PromptRequest, ReplayFrom, ReplayFromStart,
    ResumeSessionRequest, SessionId, SessionUpdate, SetSessionConfigOptionRequest, StateUpdate, StopReason,
};
use llm::{LlmResponse, testing::FakeLlmProvider};
use std::sync::Arc;
use tokio::{
    sync::{Notify, mpsc, oneshot},
    task::LocalSet,
};

#[tokio::test(flavor = "current_thread")]
async fn initialize_remote_metadata_tracks_live_session_and_omits_stdio() {
    LocalSet::new()
        .run_until(async {
            let mut stdio = AcpTestHarness::start().await;
            assert!(
                acp_utils::notifications::RemoteServerInfo::from_meta(stdio.initialize_response.meta.as_ref())
                    .is_none()
            );
            stdio.shutdown().await;

            let mut harness = AcpTestHarness::builder().persistent_host().remote_cwd("/server/default").start().await;
            let info = acp_utils::notifications::RemoteServerInfo::from_meta(harness.initialize_response.meta.as_ref())
                .unwrap();
            assert_eq!(info.cwd, std::path::PathBuf::from("/server/default"));
            assert!(info.session_id.is_none());
            let session = harness.insert_agent_switching_session().await;
            let id = session.session_id().clone();
            harness.disconnect().await;
            harness.reconnect().await;
            let info = acp_utils::notifications::RemoteServerInfo::from_meta(harness.initialize_response.meta.as_ref())
                .unwrap();
            assert_eq!(info.cwd, std::path::PathBuf::from("/tmp"));
            assert_eq!(info.session_id, Some(id.clone()));
            harness.client_cx.send_request(CloseSessionRequest::new(id)).block_task().await.unwrap();
            harness.disconnect().await;
            harness.reconnect().await;
            let info = acp_utils::notifications::RemoteServerInfo::from_meta(harness.initialize_response.meta.as_ref())
                .unwrap();
            assert_eq!(info.cwd, std::path::PathBuf::from("/server/default"));
            assert!(info.session_id.is_none());
            harness.shutdown().await;
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn gated_turn_completes_without_resume() {
    LocalSet::new()
        .run_until(async {
            let mut harness = AcpTestHarness::start().await;
            let (id, release) = start_paused_turn(&mut harness).await;
            finish_original_turn(&mut harness, &id, release).await;
            harness.shutdown().await;
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn same_id_resume_preserves_paused_turn() {
    LocalSet::new()
        .run_until(async {
            let mut harness = AcpTestHarness::start().await;
            let (id, release) = start_paused_turn(&mut harness).await;

            resume(&harness, &id).await;
            loop {
                match harness.peer.next_session_notification().await.update {
                    SessionUpdate::StateUpdate(StateUpdate::Running(_)) => break,
                    SessionUpdate::StateUpdate(StateUpdate::Idle(_)) => panic!("reattachment lost running state"),
                    _ => {}
                }
            }
            finish_original_turn(&mut harness, &id, release).await;
            harness.shutdown().await;
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn persistent_host_reattaches_to_original_paused_turn() {
    LocalSet::new()
        .run_until(async {
            let mut harness = AcpTestHarness::builder().persistent_host().start().await;
            let (id, release) = start_paused_turn(&mut harness).await;

            harness.disconnect().await;
            assert_no_ended_turn(&harness, &id);
            harness.reconnect().await;
            resume(&harness, &id).await;
            loop {
                match harness.peer.next_session_notification().await.update {
                    SessionUpdate::StateUpdate(StateUpdate::Running(_)) => break,
                    SessionUpdate::StateUpdate(StateUpdate::Idle(_)) => panic!("reattachment lost running state"),
                    _ => {}
                }
            }
            finish_original_turn(&mut harness, &id, release).await;
            harness.shutdown().await;
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn completed_while_detached_is_persisted_and_replayed() {
    LocalSet::new()
        .run_until(async {
            let mut harness = AcpTestHarness::builder().persistent_host().start().await;
            let (id, release, completed) = start_observed_paused_turn(&mut harness).await;
            harness.disconnect().await;
            assert_no_ended_turn(&harness, &id);
            release.notify_one();
            completed.await.expect("original provider completes without a client");
            harness.reconnect().await;
            resume(&harness, &id).await;
            let mut users = 0;
            let mut responses = 0;
            loop {
                match harness.peer.next_session_notification().await.update {
                    SessionUpdate::UserMessage(_) => users += 1,
                    SessionUpdate::AgentMessage(message) => {
                        assert!(message.content.value().unwrap().iter().any(
                            |block| matches!(block, ContentBlock::Text(text) if text.text == "before gate after gate")
                        ));
                        responses += 1;
                    }
                    SessionUpdate::StateUpdate(StateUpdate::Idle(_)) => break,
                    _ => {}
                }
            }
            assert_eq!((users, responses), (1, 1));
            assert_eq!(harness.live_runtime_count(), 1);
            assert_eq!(
                harness
                    .stored_events(&id)
                    .iter()
                    .filter(|event| matches!(
                        event,
                        SessionEvent::Agent(AgentEvent::Message(MessageEvent::Text { is_complete: true, .. }))
                    ))
                    .count(),
                1
            );
            assert!(harness.stored_events(&id).iter().any(|event| matches!(
                event,
                SessionEvent::Agent(AgentEvent::Turn(TurnEvent::Ended { outcome: TurnOutcome::Completed }))
            )));
            harness.shutdown().await;
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn connection_scoped_disconnect_stops_paused_turn() {
    LocalSet::new()
        .run_until(async {
            let mut harness = AcpTestHarness::start().await;
            let (id, _release) = start_paused_turn(&mut harness).await;

            harness.disconnect().await;
            assert_stopped_turn(&harness, &id);
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn persistent_host_only_stops_paused_turn_on_explicit_shutdown() {
    LocalSet::new()
        .run_until(async {
            let mut harness = AcpTestHarness::builder().persistent_host().start().await;
            let (id, _release) = start_paused_turn(&mut harness).await;

            harness.disconnect().await;
            assert_no_ended_turn(&harness, &id);
            harness.reconnect().await;
            assert_no_ended_turn(&harness, &id);
            harness.shutdown().await;
            assert_stopped_turn(&harness, &id);
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn invalid_live_resume_leaves_original_turn_running() {
    LocalSet::new().run_until(async {
        let mut harness = AcpTestHarness::start().await;
        let (id, release) = start_paused_turn(&mut harness).await;
        for overrides in [
            serde_json::json!({"cwd": "/different-workspace"}),
            serde_json::json!({"mcpServers": [{"type": "http", "name": "changed", "url": "http://localhost/mcp", "headers": []}]}),
            serde_json::json!({"replayFrom": {"type": "message", "messageId": "missing"}}),
            serde_json::json!({"sessionId": "missing"}),
        ] {
            let mut value = serde_json::to_value(ResumeSessionRequest::new(id.clone(), AbsolutePath::new("/tmp"))).unwrap();
            value.as_object_mut().unwrap().extend(overrides.as_object().unwrap().clone());
            let request: ResumeSessionRequest = serde_json::from_value(value).unwrap();
            assert!(harness.client_cx.send_request(request).block_task().await.is_err());
            assert_no_ended_turn(&harness, &id);
        }
        finish_original_turn(&mut harness, &id, release).await;
        harness.shutdown().await;
    }).await;
}

#[tokio::test(flavor = "current_thread")]
async fn pending_and_detached_elicitations_cancel_without_stopping_session() {
    LocalSet::new()
        .run_until(async {
            for detach_before_elicitation in [false, true] {
                let mut harness = AcpTestHarness::builder().persistent_host().start().await;
                let (started, release) = harness.pause_prompt_expansion();
                let mut results = harness.elicit_during_prompt_expansion();
                let session = harness.insert_agent_switching_session().await;
                let id = session.session_id().clone();
                let capture = harness.peer.capture_next_elicitation();
                let prompt =
                    harness.client_cx.send_request(PromptRequest::new(id.clone(), vec!["/plan".into()])).block_task();
                started.notified().await;
                let pending = if detach_before_elicitation {
                    harness.disconnect().await;
                    release.notify_one();
                    None
                } else {
                    release.notify_one();
                    let responder = capture.await.expect("client received elicitation");
                    harness.disconnect().await;
                    Some(responder)
                };
                assert_eq!(
                    results.recv().await.expect("MCP receives cancellation"),
                    mcp_utils::client::cancel_result()
                );
                drop(pending);
                assert!(prompt.await.is_err(), "disconnected prompt response is not retried");
                harness.reconnect().await;
                resume(&harness, &id).await;
                let mut completed = false;
                loop {
                    let update = harness.peer.next_session_notification().await.update;
                    match update {
                        SessionUpdate::AgentMessage(_) => completed = true,
                        SessionUpdate::StateUpdate(StateUpdate::Idle(_)) if completed => break,
                        _ => {}
                    }
                }
                harness
                    .client_cx
                    .send_request(PromptRequest::new(id.clone(), vec!["still usable".into()]))
                    .block_task()
                    .await
                    .expect("session remains usable");
                harness.expect_idle(&id, StopReason::EndTurn).await;
                session.planner().assert_saw(&["expanded plan", "planner reply", "still usable"]);
                harness.shutdown().await;
            }
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn repeated_attachment_replays_history_and_preserves_config_and_mcp() {
    LocalSet::new()
        .run_until(async {
            let mut harness = AcpTestHarness::builder().persistent_host().start().await;
            let session = harness.insert_agent_switching_session().await;
            let id = session.session_id().clone();
            harness
                .client_cx
                .send_request(PromptRequest::new(id.clone(), vec!["first prompt".into()]))
                .block_task()
                .await
                .expect("first prompt accepted");
            harness.expect_idle(&id, StopReason::EndTurn).await;
            let configured = harness
                .client_cx
                .send_request(SetSessionConfigOptionRequest::new(id.clone(), "reasoning_effort", "high"))
                .block_task()
                .await
                .expect("configure live session");
            for cycle in 0..4 {
                harness.disconnect().await;
                harness.reconnect().await;
                let mut request = ResumeSessionRequest::new(id.clone(), AbsolutePath::new("/tmp"));
                if cycle < 3 {
                    request = request.replay_from(ReplayFrom::Start(ReplayFromStart::new()));
                }
                let response = harness.client_cx.send_request(request).block_task().await.expect("reattach");
                assert_eq!(response.config_options, configured.config_options);
                let mut users = 0;
                let mut agents = 0;
                loop {
                    match harness.peer.next_session_notification().await.update {
                        SessionUpdate::UserMessage(_) => users += 1,
                        SessionUpdate::AgentMessage(_) => agents += 1,
                        SessionUpdate::StateUpdate(StateUpdate::Idle(_)) => break,
                        SessionUpdate::StateUpdate(StateUpdate::Running(_)) => panic!("idle session reported running"),
                        _ => {}
                    }
                }
                assert_eq!((users, agents), if cycle < 3 { (1, 1) } else { (0, 0) });
                harness.expect_mcp_server_status_exact(&["planner-mcp"]).await;
                harness.expect_available_commands(&["plan"], &["edit"]).await;
            }
            harness
                .client_cx
                .send_request(PromptRequest::new(id.clone(), vec!["next prompt".into()]))
                .block_task()
                .await
                .expect("next prompt accepted");
            harness.expect_idle(&id, StopReason::EndTurn).await;
            session.planner().assert_saw_exactly(&["first prompt", "planner reply", "next prompt"]);
            harness.shutdown().await;
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn new_and_restored_sessions_retain_their_client_mcp_inputs() {
    LocalSet::new()
        .run_until(async {
            let mut harness = AcpTestHarness::start().await;
            let servers = vec![
                serde_json::from_value(serde_json::json!({
                    "type": "http", "name": "client-mcp", "url": "http://localhost/mcp", "headers": []
                }))
                .unwrap(),
            ];
            let created = harness
                .client_cx
                .send_request(NewSessionRequest::new(AbsolutePath::new("/tmp")).mcp_servers(servers.clone()))
                .block_task()
                .await
                .expect("create session with client MCP inputs");
            for restore in [false, true] {
                if restore {
                    harness
                        .client_cx
                        .send_request(CloseSessionRequest::new(created.session_id.clone()))
                        .block_task()
                        .await
                        .expect("close before saved restore");
                }
                let request = ResumeSessionRequest::new(created.session_id.clone(), AbsolutePath::new("/tmp"))
                    .mcp_servers(servers.clone());
                harness.client_cx.send_request(request).block_task().await.expect("matching inputs accepted");
                assert!(
                    harness
                        .client_cx
                        .send_request(ResumeSessionRequest::new(created.session_id.clone(), AbsolutePath::new("/tmp")))
                        .block_task()
                        .await
                        .is_err(),
                    "omitting client MCP configuration must not rebuild a live session"
                );
            }
            harness.shutdown().await;
        })
        .await;
}

async fn start_paused_turn(harness: &mut AcpTestHarness) -> (SessionId, Arc<Notify>) {
    let (id, release, _) = start_observed_paused_turn(harness).await;
    (id, release)
}

async fn start_observed_paused_turn(harness: &mut AcpTestHarness) -> (SessionId, Arc<Notify>, oneshot::Receiver<()>) {
    let release = Arc::new(Notify::new());
    let provider = FakeLlmProvider::new(vec![vec![
        LlmResponse::Start,
        LlmResponse::text("before gate"),
        LlmResponse::text(" after gate"),
        LlmResponse::done(),
    ]])
    .pause_turn_after(0, 1, release.clone());
    let (tx, mut events, handle) = agent(provider).spawn().await.expect("fake agent spawns");
    let (forward, rx) = mpsc::channel(1);
    let (completed, completion) = oneshot::channel();
    tokio::spawn(async move {
        let mut completed = Some(completed);
        while let Some(event) = events.recv().await {
            let done = matches!(event, AgentEvent::Turn(TurnEvent::Ended { outcome: TurnOutcome::Completed }));
            if forward.send(event).await.is_err() {
                break;
            }
            if done && let Some(completed) = completed.take() {
                let _ = completed.send(());
            }
        }
    });
    let id = SessionId::new("remote-lifecycle");
    harness.append_stored_session(id.0.as_ref(), "2026-05-01T00:00:00Z");
    harness.insert_stub_session(tx, rx, handle, id.clone(), "anthropic:claude-sonnet-4-5").await;
    harness
        .client_cx
        .send_request(PromptRequest::new(id.clone(), vec!["original prompt".into()]))
        .block_task()
        .await
        .expect("prompt accepted");

    let mut running = false;
    loop {
        let notification = harness.peer.next_session_notification().await;
        assert_eq!(notification.session_id, id);
        match notification.update {
            SessionUpdate::StateUpdate(StateUpdate::Running(_)) => running = true,
            SessionUpdate::AgentMessageChunk(chunk) if matches!(&chunk.content, ContentBlock::Text(text) if text.text == "before gate") =>
            {
                assert!(running, "running must precede provider output");
                return (id, release, completion);
            }
            SessionUpdate::StateUpdate(StateUpdate::Idle(_)) => panic!("turn ended before reaching the gate"),
            _ => {}
        }
    }
}

async fn resume(harness: &AcpTestHarness, id: &SessionId) {
    harness
        .client_cx
        .send_request(
            ResumeSessionRequest::new(id.clone(), AbsolutePath::new("/tmp"))
                .replay_from(ReplayFrom::Start(ReplayFromStart::new())),
        )
        .block_task()
        .await
        .expect("same-ID resume succeeds");
}

async fn finish_original_turn(harness: &mut AcpTestHarness, id: &SessionId, release: Arc<Notify>) {
    let second_prompt = harness
        .client_cx
        .send_request(PromptRequest::new(id.clone(), vec!["must not start another turn".into()]))
        .block_task()
        .await;
    assert!(second_prompt.is_err(), "resume must preserve the running turn, not accept a new prompt");
    assert_no_ended_turn(harness, id);
    release.notify_one();

    let mut completed = false;
    loop {
        let notification = harness.peer.next_session_notification().await;
        assert_eq!(&notification.session_id, id);
        match notification.update {
            SessionUpdate::AgentMessage(message) => {
                completed |= message.content.value().is_some_and(|content| {
                    content
                        .iter()
                        .any(|block| matches!(block, ContentBlock::Text(text) if text.text == "before gate after gate"))
                });
            }
            SessionUpdate::StateUpdate(StateUpdate::Idle(idle)) => {
                assert_eq!(idle.stop_reason, Some(StopReason::EndTurn));
                assert!(completed, "the original gated response must complete before idle");
                break;
            }
            _ => {}
        }
    }

    let events = harness.stored_events(id);
    let prompts: Vec<_> =
        events.iter().filter(|event| matches!(event, SessionEvent::User(UserEvent::Message { .. }))).collect();
    assert_eq!(prompts.len(), 1, "reattachment must not duplicate the user prompt");
    let completed_text: Vec<_> = events
        .iter()
        .filter_map(|event| match event {
            SessionEvent::Agent(AgentEvent::Message(MessageEvent::Text { chunk, is_complete: true, .. })) => {
                Some(chunk.as_str())
            }
            _ => None,
        })
        .collect();
    assert_eq!(completed_text, ["before gate after gate"]);
    assert!(events.iter().any(|event| matches!(
        event,
        SessionEvent::Agent(AgentEvent::Turn(TurnEvent::Ended { outcome: TurnOutcome::Completed }))
    )));
}

fn assert_no_ended_turn(harness: &AcpTestHarness, id: &SessionId) {
    assert_eq!(harness.live_runtime_count(), 1, "the original fake runtime must remain alive");
    assert!(
        !harness
            .stored_events(id)
            .iter()
            .any(|event| matches!(event, SessionEvent::Agent(AgentEvent::Turn(TurnEvent::Ended { .. })))),
        "the original turn must remain paused, not be cancelled or completed"
    );
}

fn assert_stopped_turn(harness: &AcpTestHarness, id: &SessionId) {
    assert_eq!(harness.live_runtime_count(), 0, "shutdown must stop the original fake runtime");
    let events = harness.stored_events(id);
    assert!(
        events.iter().any(|event| matches!(event,
            SessionEvent::User(UserEvent::Message { content, .. })
                if content == &vec![llm::ContentBlock::text("original prompt")]
        )),
        "the accepted prompt must remain persisted after shutdown"
    );
    assert!(
        !events.iter().any(|event| matches!(
            event,
            SessionEvent::Agent(AgentEvent::Turn(TurnEvent::Ended { outcome: TurnOutcome::Completed }))
        )),
        "shutdown must not complete the gated turn"
    );
}
