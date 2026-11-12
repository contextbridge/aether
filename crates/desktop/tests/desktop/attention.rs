use crate::support::Edges;
use crate::support::{asked, replying};
use acp_utils::testing::{FakeAgent, Question, announced, idle_state, running_state};
use aether_desktop::platform::Notification;
use aether_desktop::testing::{DesktopHarness, DesktopHarnessBuilder};
use aether_desktop::threads::{Thread, ThreadStatus};
use agent_client_protocol::v2::schema::{
    ElicitationSchema, SessionUpdate, StopReason, SubagentUpdate, ToolCallStatus, ToolCallUpdate,
};

#[test]
fn a_turn_that_finishes_on_an_unselected_thread_is_announced_once_and_stays_unread_until_selected() {
    let (mut harness, alpha) = unselected_alpha();

    harness.submit(alpha, "Fix the build");

    assert_eq!(
        harness.platform().notifications(),
        [notification("alpha-session", "Fix the build", "The agent finished its turn")]
    );
    assert!(harness.threads().is_unread(alpha));
    let unread = harness.bounds("thread-row-0-unread").expect("alpha shows the unread dot");
    let title = harness.bounds("thread-row-0-title").expect("alpha shows its title");
    assert!(unread.left() >= title.right(), "unread is after the title, not in the activity slot");
    assert!(harness.bounds("thread-row-0-conversation-icon").is_some());

    harness.click("thread-row-0").unwrap();

    assert!(!harness.threads().is_unread(alpha));
    assert!(harness.bounds("thread-row-0-unread").is_none(), "selecting alpha marks it seen");
}

#[test]
fn an_unread_response_stays_visible_when_the_agent_disconnects() {
    let (mut harness, alpha) = unselected_alpha();
    harness.submit(alpha, "hi");

    harness.connector().kill_agents("/alpha");
    harness.run_until(|_| alpha.status() == ThreadStatus::Disconnected);

    assert!(harness.bounds("thread-row-0-status-disconnected-warning-triangle").is_some());
    assert!(harness.bounds("thread-row-0-conversation-icon").is_none());
    assert!(harness.bounds("thread-row-0-unread").is_some());
}

#[test]
fn a_turn_that_finishes_on_the_selected_thread_is_neither_unread_nor_announced() {
    let (mut harness, alpha) = DesktopHarness::alpha(replying("alpha-session", "from alpha"));

    harness.submit(alpha, "hi");

    assert!(!harness.threads().is_unread(alpha));
    assert!(harness.bounds("thread-row-0-unread").is_none());
    assert_eq!(harness.platform().notifications(), []);
}

#[test]
fn a_turn_that_finishes_while_the_window_is_inactive_stays_unread_until_the_window_is_back() {
    let (mut harness, alpha) = DesktopHarness::alpha(replying("alpha-session", "from alpha"));
    harness.deactivate();

    harness.submit(alpha, "hi");

    assert_eq!(
        harness.platform().notifications(),
        [notification("alpha-session", "hi", "The agent finished its turn")]
    );
    assert!(harness.threads().is_unread(alpha));

    harness.activate();

    assert!(!harness.threads().is_unread(alpha));
}

#[test]
fn a_question_on_an_unselected_thread_is_announced_with_the_question() {
    let alpha_agent =
        FakeAgent::new("alpha-session").prompt_asks("Pick a branch", Question::Form(ElicitationSchema::new()));
    let (mut harness, alpha) = DesktopHarnessBuilder::default()
        .agent("/alpha", alpha_agent)
        .agent("/beta", replying("beta-session", "from beta"))
        .start("/alpha");
    harness.start_thread("/beta");

    alpha.send("pick one".into());
    harness.run_until(|_| alpha.status() == ThreadStatus::NeedsInput);

    assert_eq!(harness.platform().notifications(), [notification("alpha-session", "pick one", "Pick a branch")]);
}

#[test]
fn clicking_a_notification_brings_the_window_back_with_its_thread_selected() {
    let (mut harness, alpha) = unselected_alpha();
    harness.deactivate();
    harness.submit(alpha, "hi");

    harness.click_notification("alpha-session");

    assert!(harness.is_active());
    assert_eq!(harness.selected_thread(), Some(alpha));
    assert!(!harness.threads().is_unread(alpha));
}

#[test]
fn activity_replaces_the_conversation_icon_until_the_turn_and_its_sub_agents_end() {
    let (agent, mut requests) = FakeAgent::new("alpha-session").capture();
    let (mut harness, alpha) = DesktopHarness::alpha(agent);
    assert!(harness.bounds("thread-row-0-status-running").is_none(), "an idle thread shows no activity icon");
    let icon = harness.bounds("thread-row-0-conversation-icon").expect("idle thread has a conversation icon");
    let title_left = harness.bounds("thread-row-0-title").unwrap().left();

    let turn = harness.start_turn(alpha, &mut requests, "explore in the background");
    let waveform = harness.bounds("thread-row-0-status-running-waveform").expect("running thread has a waveform");
    assert!(harness.bounds("thread-row-0-conversation-icon").is_none());
    assert_eq!(waveform.left(), icon.left());
    assert_eq!(harness.bounds("thread-row-0-title").unwrap().left(), title_left);

    let spawn = ToolCallUpdate::new("spawn-1").title("spawn_subagent").status(ToolCallStatus::Completed);
    turn.update(SessionUpdate::ToolCallUpdate(spawn));
    let connection = harness.next(&mut requests.connection);
    let child = |state| announced("alpha-session", "spawn-1", SubagentUpdate::new("child-a").state(state));
    connection.send_notification(child(running_state())).unwrap();
    turn.end(StopReason::EndTurn);
    harness.run_until(|_| alpha.with_core(|core| !core.waiting_for_response()).unwrap_or(false));
    assert!(harness.bounds("thread-row-0-status-running").is_none(), "the turn is over, so nothing spins");
    assert!(harness.bounds("thread-row-0-status-background-dot").is_some(), "a sub-agent outlives the turn");

    connection.send_notification(child(idle_state(StopReason::EndTurn))).unwrap();
    harness.run_until(|_| alpha.status() == ThreadStatus::Idle);

    assert!(harness.bounds("thread-row-0-status-background").is_none());
    assert!(harness.bounds("thread-row-0-conversation-icon").is_some());
    assert_eq!(harness.bounds("thread-row-0-title").unwrap().left(), title_left);
}

#[test]
fn a_thread_waiting_on_an_answer_shows_a_speech_bubble() {
    let (mut harness, _) =
        asked(FakeAgent::new("alpha-session").prompt_asks("Pick a branch", Question::Form(ElicitationSchema::new())));

    assert!(harness.bounds("thread-row-0-status-needs-input-speech-bubble").is_some());
    assert!(harness.bounds("thread-row-0-conversation-icon").is_none());
    assert!(harness.bounds("thread-row-0-status-running").is_none(), "waiting is not busy");
}

#[test]
fn a_saved_session_has_no_activity_icon() {
    let mut harness = DesktopHarnessBuilder::default()
        .session("saved", "/alpha", "2026-01-01T00:00:00Z", "prompt")
        .project("/alpha")
        .build();

    assert!(harness.bounds("history-row-saved-conversation-icon").is_some());
    assert!(harness.bounds("history-row-saved-status-dormant").is_none());
    assert!(harness.bounds("history-row-saved-unread").is_none());
}

fn notification(tag: &str, title: &str, body: &str) -> Notification {
    Notification { tag: tag.into(), title: title.into(), body: body.into() }
}

fn unselected_alpha() -> (DesktopHarness, Thread) {
    let (mut harness, alpha) = DesktopHarnessBuilder::default()
        .agent("/alpha", replying("alpha-session", "from alpha"))
        .agent("/beta", replying("beta-session", "from beta"))
        .start("/alpha");
    harness.start_thread("/beta");
    (harness, alpha)
}
