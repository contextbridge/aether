use acp_utils::testing::running_notification;
use aether_desktop::sidebar::StatusIndicator;
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
fn activity_indicators_distinguish_busy_waiting_and_broken_from_quiet_threads() {
    let cases = [
        (ThreadStatus::Connecting, Some(StatusIndicator::Connecting)),
        (ThreadStatus::Idle, None),
        (ThreadStatus::Running, Some(StatusIndicator::Running)),
        (ThreadStatus::NeedsInput, Some(StatusIndicator::NeedsInput)),
        (ThreadStatus::Errored("agent not found".into()), Some(StatusIndicator::Errored)),
        (ThreadStatus::Disconnected, Some(StatusIndicator::Disconnected)),
        (ThreadStatus::Dormant, None),
    ];

    for (status, indicator) in cases {
        assert_eq!(StatusIndicator::of(&status), indicator, "{status:?}");
    }
}

#[test]
fn activity_labels_explain_what_the_agent_needs() {
    for (indicator, label) in [
        (StatusIndicator::Connecting, "Starting agent"),
        (StatusIndicator::Running, "Agent working"),
        (StatusIndicator::NeedsInput, "Waiting for your answer"),
        (StatusIndicator::Errored, "Unable to connect — reconnect to try again"),
        (StatusIndicator::Disconnected, "Disconnected — reconnect to continue"),
    ] {
        assert_eq!(indicator.label(), label);
    }
}

fn core() -> SessionCore {
    SessionCore::new(SessionCoreConfigBuilder::new().build())
}
