use acp_utils::testing::{announced, idle_notification, idle_state, running_notification, running_state};
use aether_cli::acp::testing::{AcpTestHarness, FakeSubAgents};
use aether_core::events::{AgentEvent, StreamState, SubAgentProgressPayload, ToolEvent, TurnEvent, TurnOutcome};
use agent_client_protocol::schema::v2::{
    self as acp, CancelSessionNotification, SessionId, SessionUpdate, StateUpdate, UpdateSessionNotification,
};
use llm::ToolCallRequest;
use llm::testing::{FakeLlmProvider, llm_response};

#[tokio::test(flavor = "current_thread")]
async fn sub_agents_are_announced_on_their_parent_and_report_on_their_own_session() {
    let notifications = spawn_sub_agents([
        sub_agent(started("Find the bug")),
        sub_agent(AgentEvent::text("reply", "Found it", StreamState::Complete)),
        sub_agent(AgentEvent::turn_ended(TurnOutcome::Completed)),
    ])
    .await;

    let reply = acp::AgentMessage::new("reply").content(vec!["Found it".into()]);
    let mut expected = spawned("parent", "spawn-call", "child", "Find the bug").to_vec();
    expected.extend([
        UpdateSessionNotification::new("child", SessionUpdate::AgentMessage(reply)),
        announced("parent", "spawn-call", worker("child").state(idle_state(acp::StopReason::EndTurn))),
        idle_notification("child", Some(acp::StopReason::EndTurn)),
    ]);
    assert_eq!(notifications, expected);
}

#[tokio::test(flavor = "current_thread")]
async fn a_failed_sub_agent_goes_idle_with_an_error() {
    let failed = TurnOutcome::Failed { message_id: "m".into(), error: "boom".into() };

    let notifications = spawn_sub_agents([sub_agent(AgentEvent::turn_ended(failed))]).await;

    let error = acp::StopReason::Error(acp::ErrorStopReason::new().error(acp::Error::internal_error().data("boom")));
    let message = acp::AgentMessage::new("m").content(vec!["Error: boom".into()]);
    assert_eq!(
        notifications,
        [
            UpdateSessionNotification::new("child", SessionUpdate::AgentMessage(message)),
            announced("parent", "spawn-call", worker("child").state(idle_state(error.clone()))),
            idle_notification("child", Some(error)),
        ]
    );
}

#[tokio::test(flavor = "current_thread")]
async fn sub_agent_tool_calls_report_on_the_sub_agent_session() {
    let call = ToolCallRequest { id: "grep-call".into(), name: "coding__grep".into(), arguments: "{}".into() };

    let notifications = spawn_sub_agents([sub_agent(AgentEvent::Tool(ToolEvent::Call { request: call }))]).await;

    let tool_calls: Vec<_> = notifications
        .iter()
        .filter_map(|notification| match &notification.update {
            SessionUpdate::ToolCallUpdate(update) => {
                Some((notification.session_id.0.as_ref(), update.tool_call_id.0.as_ref()))
            }
            _ => None,
        })
        .collect();
    assert_eq!(tool_calls, [("child", "grep-call")]);
}

#[tokio::test(flavor = "current_thread")]
async fn nested_sub_agents_are_announced_on_the_sub_agent_that_spawned_them() {
    let grandchild = SubAgentProgressPayload {
        session_id: "grandchild".into(),
        agent_name: "worker".into(),
        event: started("Dig deeper"),
    };
    let nested = AgentEvent::Tool(ToolEvent::SubAgentProgress {
        request: ToolCallRequest { id: "child-spawn".into(), name: FakeSubAgents::TOOL.into(), arguments: "{}".into() },
        payload: Box::new(grandchild),
    });

    let notifications = spawn_sub_agents([sub_agent(nested)]).await;

    assert_eq!(notifications, spawned("child", "child-spawn", "grandchild", "Dig deeper"));
}

#[tokio::test(flavor = "current_thread")]
async fn cancelling_a_sub_agent_session_cancels_the_live_sub_agent() {
    AcpTestHarness::run(|harness| async move {
        let sub_agent = harness.start_sub_agent();

        harness.client_cx.send_notification(CancelSessionNotification::new(sub_agent.id())).unwrap();

        sub_agent.cancelled().await;
    })
    .await;
}

async fn spawn_sub_agents(
    progress: impl IntoIterator<Item = SubAgentProgressPayload>,
) -> Vec<UpdateSessionNotification> {
    let sub_agents = FakeSubAgents::new(progress);
    AcpTestHarness::run(|mut harness| async move {
        let parent = delegate(&mut harness, &sub_agents).await;
        sub_agent_notifications(&mut harness, &parent).await
    })
    .await
}

async fn delegate(harness: &mut AcpTestHarness, sub_agents: &FakeSubAgents) -> SessionId {
    let provider = FakeLlmProvider::new(vec![
        llm_response().tool_call("spawn-call", FakeSubAgents::TOOL, &["{}"]).build(),
        llm_response().text(&["done"]).build(),
    ]);
    let parent = harness.session("parent", provider).sub_agents(sub_agents).insert().await;
    harness.prompt(&parent, "delegate").await.expect("the prompt is accepted");
    parent
}

async fn sub_agent_notifications(harness: &mut AcpTestHarness, parent: &SessionId) -> Vec<UpdateSessionNotification> {
    let mut notifications = Vec::new();
    loop {
        let notification = harness.peer.next_session_notification().await;
        let on_parent = notification.session_id == *parent;
        if on_parent && matches!(notification.update, SessionUpdate::StateUpdate(StateUpdate::Idle(_))) {
            return notifications;
        }
        if !on_parent || matches!(notification.update, SessionUpdate::SubagentUpdate(_)) {
            notifications.push(notification);
        }
    }
}

fn sub_agent(event: AgentEvent) -> SubAgentProgressPayload {
    SubAgentProgressPayload { session_id: "child".into(), agent_name: "worker".into(), event }
}

fn started(task: &str) -> AgentEvent {
    AgentEvent::Turn(TurnEvent::Started { content: vec![llm::ContentBlock::text(task)] })
}

fn worker(session: &str) -> acp::SubagentUpdate {
    acp::SubagentUpdate::new(session).title("worker")
}

fn spawned(parent: &str, tool_call_id: &str, child: &str, task: &str) -> [UpdateSessionNotification; 3] {
    let cancellable = acp::SubagentSessionCapabilities::new().cancel(acp::SessionCancelCapabilities::new());
    let message = acp::SessionMessage::new("task")
        .sender_session_id(SessionId::new(parent))
        .recipient_session_id(SessionId::new(child))
        .content(vec![task.into()]);
    [
        announced(parent, tool_call_id, worker(child).capabilities(cancellable).state(running_state())),
        running_notification(child),
        UpdateSessionNotification::new(child, SessionUpdate::SessionMessage(message)),
    ]
}
