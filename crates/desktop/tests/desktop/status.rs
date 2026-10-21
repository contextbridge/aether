use acp_utils::testing::running_notification;
use aether_desktop::sidebar::{Seen, StatusDot};
use aether_desktop::threads::{ConnectionPhase, ThreadStatus};
use agent_client_protocol::schema::v2::{
    CreateElicitationRequest, ElicitationFormMode, ElicitationSchema, ElicitationSessionScope,
};
use client_core::session_core::{Elicitation, SessionCore};
use client_core::testing::SessionCoreConfigBuilder;

#[test]
fn a_thread_whose_session_is_not_open_yet_is_connecting() {
    assert_eq!(ThreadStatus::derive(&ConnectionPhase::Connecting, None, None), ThreadStatus::Connecting);
}

#[test]
fn a_connected_thread_with_nothing_running_is_idle() {
    let core = core();

    assert_eq!(ThreadStatus::derive(&ConnectionPhase::Connected, Some(&core), None), ThreadStatus::Idle);
}

#[test]
fn a_connected_thread_with_a_running_turn_is_running() {
    let mut core = core();
    core.on_acp_event(running_notification("test-session").into());

    assert_eq!(ThreadStatus::derive(&ConnectionPhase::Connected, Some(&core), None), ThreadStatus::Running);
}

#[test]
fn a_pending_elicitation_needs_input_even_while_the_turn_runs() {
    let mut core = core();
    core.on_acp_event(running_notification("test-session").into());
    let elicitation = Elicitation::Request(Box::new(CreateElicitationRequest::new(
        ElicitationFormMode::new(ElicitationSessionScope::new("test-session"), ElicitationSchema::new()),
        "Pick a branch",
    )));

    assert_eq!(
        ThreadStatus::derive(&ConnectionPhase::Connected, Some(&core), Some(&elicitation)),
        ThreadStatus::NeedsInput
    );
}

#[test]
fn a_thread_that_failed_to_connect_is_errored_with_the_reason() {
    let phase = ConnectionPhase::Failed("agent not found".into());

    assert_eq!(ThreadStatus::derive(&phase, None, None), ThreadStatus::Errored("agent not found".into()));
}

#[test]
fn a_thread_whose_connection_closed_is_disconnected_whatever_its_session_was_doing() {
    let mut core = core();
    core.on_acp_event(running_notification("test-session").into());

    assert_eq!(ThreadStatus::derive(&ConnectionPhase::Closed, Some(&core), None), ThreadStatus::Disconnected);
}

#[test]
fn a_stopped_thread_is_dormant_whatever_its_session_was_doing() {
    let mut core = core();
    core.on_acp_event(running_notification("test-session").into());

    assert_eq!(ThreadStatus::derive(&ConnectionPhase::Stopped, Some(&core), None), ThreadStatus::Dormant);
}

#[test]
fn only_a_thread_whose_connection_failed_or_closed_can_reconnect() {
    let reconnectable = [ThreadStatus::Errored("agent not found".into()), ThreadStatus::Disconnected];
    let others = [
        ThreadStatus::Connecting,
        ThreadStatus::Idle,
        ThreadStatus::Running,
        ThreadStatus::NeedsInput,
        ThreadStatus::Dormant,
    ];

    assert!(reconnectable.iter().all(ThreadStatus::can_reconnect));
    assert!(!others.iter().any(ThreadStatus::can_reconnect));
}

#[test]
fn each_status_shows_its_dot() {
    let cases = [
        (ThreadStatus::Connecting, Some(StatusDot::Running)),
        (ThreadStatus::Idle, None),
        (ThreadStatus::Running, Some(StatusDot::Running)),
        (ThreadStatus::NeedsInput, Some(StatusDot::NeedsInput)),
        (ThreadStatus::Errored("agent not found".into()), Some(StatusDot::Errored)),
        (ThreadStatus::Disconnected, Some(StatusDot::Errored)),
        (ThreadStatus::Dormant, Some(StatusDot::Dormant)),
    ];

    for (status, dot) in cases {
        assert_eq!(StatusDot::of(&status, Seen::Read), dot, "{status:?}");
    }
}

#[test]
fn an_idle_thread_with_an_unseen_turn_shows_the_unread_dot() {
    assert_eq!(StatusDot::of(&ThreadStatus::Idle, Seen::Unread), Some(StatusDot::Unread));
}

#[test]
fn a_thread_that_is_busy_or_broken_shows_its_status_over_an_unseen_turn() {
    let statuses = [
        ThreadStatus::Connecting,
        ThreadStatus::Running,
        ThreadStatus::NeedsInput,
        ThreadStatus::Errored("agent not found".into()),
        ThreadStatus::Disconnected,
        ThreadStatus::Dormant,
    ];

    for status in statuses {
        assert_eq!(StatusDot::of(&status, Seen::Unread), StatusDot::of(&status, Seen::Read), "{status:?}");
    }
}

fn core() -> SessionCore {
    SessionCore::new(SessionCoreConfigBuilder::new().build())
}
