use crate::support::replying;
use acp_utils::testing::{FakeAgent, idle_notification};
use aether_desktop::testing::{DesktopHarness, DesktopHarnessBuilder};
use aether_desktop::threads::{Thread, ThreadStatus};
use agent_client_protocol::schema::v2::{NewSessionResponse, PromptResponse};
use gpui::{Entity, SystemNotification, TestAppContext};

#[gpui::test]
fn a_turn_that_finishes_on_an_unselected_thread_stays_unread_until_selected(cx: &mut TestAppContext) {
    let (harness, alpha) = unselected_alpha(cx);

    harness.submit(cx, &alpha, "hi");

    assert!(harness.threads(cx).read_with(cx, |threads, _| threads.is_unread(&alpha)));
    let unread = harness.bounds(cx, "thread-row-0-unread").expect("alpha shows the unread dot");
    let title = harness.bounds(cx, "thread-row-0-title").expect("alpha shows its title");
    assert!(unread.left() >= title.right(), "unread is after the title, not in the activity slot");
    assert!(harness.bounds(cx, "thread-row-0-conversation-icon").is_some());

    harness.click(cx, "thread-row-0").unwrap();

    assert!(!harness.threads(cx).read_with(cx, |threads, _| threads.is_unread(&alpha)));
    assert!(harness.bounds(cx, "thread-row-0-unread").is_none(), "selecting alpha marks it seen");
}

#[gpui::test]
fn an_unread_response_stays_visible_when_the_agent_disconnects(cx: &mut TestAppContext) {
    let (harness, alpha) = unselected_alpha(cx);
    harness.submit(cx, &alpha, "hi");

    harness.connector().kill_agents("/alpha");
    harness.run_until(cx, |cx| alpha.read(cx).status() == ThreadStatus::Disconnected);

    assert!(harness.bounds(cx, "thread-row-0-status-disconnected-warning-triangle").is_some());
    assert!(harness.bounds(cx, "thread-row-0-conversation-icon").is_none());
    assert!(harness.bounds(cx, "thread-row-0-unread").is_some());
}

#[gpui::test]
fn a_turn_that_finishes_on_the_selected_thread_is_neither_unread_nor_announced(cx: &mut TestAppContext) {
    let harness = DesktopHarnessBuilder::default().agent("/alpha", replying("alpha-session", "from alpha")).build(cx);
    let alpha = harness.start_thread(cx, "/alpha");

    harness.submit(cx, &alpha, "hi");

    assert!(!harness.threads(cx).read_with(cx, |threads, _| threads.is_unread(&alpha)));
    assert!(harness.bounds(cx, "thread-row-0-unread").is_none());
    assert_eq!(cx.shown_system_notifications(), []);
}

#[gpui::test]
fn a_turn_that_finishes_on_an_unselected_thread_posts_one_notification(cx: &mut TestAppContext) {
    let (harness, alpha) = unselected_alpha(cx);

    harness.submit(cx, &alpha, "Fix the build");

    assert_eq!(
        cx.shown_system_notifications(),
        [notification("alpha-session", "Fix the build", "The agent finished its turn")]
    );
}

#[gpui::test]
fn a_turn_that_finishes_while_the_window_is_inactive_stays_unread_until_the_window_is_back(cx: &mut TestAppContext) {
    let harness = DesktopHarnessBuilder::default().agent("/alpha", replying("alpha-session", "from alpha")).build(cx);
    let alpha = harness.start_thread(cx, "/alpha");
    harness.deactivate(cx);

    harness.submit(cx, &alpha, "hi");

    assert_eq!(cx.shown_system_notifications(), [notification("alpha-session", "hi", "The agent finished its turn")]);
    assert!(harness.threads(cx).read_with(cx, |threads, _| threads.is_unread(&alpha)));

    harness.activate(cx);

    assert!(!harness.threads(cx).read_with(cx, |threads, _| threads.is_unread(&alpha)));
}

#[gpui::test]
fn a_question_on_an_unselected_thread_is_announced_with_the_question(cx: &mut TestAppContext) {
    let alpha_agent = FakeAgent::default()
        .new_session_response(NewSessionResponse::new("alpha-session"))
        .prompt_elicitation("Pick a branch");
    let harness = DesktopHarnessBuilder::default()
        .agent("/alpha", alpha_agent)
        .agent("/beta", replying("beta-session", "from beta"))
        .build(cx);
    let alpha = harness.start_thread(cx, "/alpha");
    harness.start_thread(cx, "/beta");

    alpha.update(cx, |thread, cx| thread.submit("pick one", cx));
    harness.run_until(cx, |cx| alpha.read(cx).status() == ThreadStatus::NeedsInput);

    assert_eq!(cx.shown_system_notifications(), [notification("alpha-session", "pick one", "Pick a branch")]);
}

#[gpui::test]
fn clicking_a_notification_brings_the_window_back_with_its_thread_selected(cx: &mut TestAppContext) {
    let (harness, alpha) = unselected_alpha(cx);
    harness.deactivate(cx);
    harness.submit(cx, &alpha, "hi");

    harness.click_notification(cx, "alpha-session");

    assert!(harness.is_active(cx));
    assert_eq!(harness.selected_thread(cx), Some(alpha.clone()));
    assert!(!harness.threads(cx).read_with(cx, |threads, _| threads.is_unread(&alpha)));
}

#[gpui::test]
fn activity_replaces_the_conversation_icon_without_moving_the_title(cx: &mut TestAppContext) {
    let (agent, mut requests) =
        FakeAgent::default().new_session_response(NewSessionResponse::new("alpha-session")).capture();
    let harness = DesktopHarnessBuilder::default().agent("/alpha", agent).build(cx);
    let alpha = harness.start_thread(cx, "/alpha");
    assert!(harness.bounds(cx, "thread-row-0-status-running").is_none(), "an idle thread shows no activity icon");
    let icon = harness.bounds(cx, "thread-row-0-conversation-icon").expect("idle thread has a conversation icon");
    let title_left = harness.bounds(cx, "thread-row-0-title").unwrap().left();

    alpha.update(cx, |thread, cx| thread.submit("work forever", cx));
    harness.run_until(cx, |_| !requests.prompt.is_empty());

    let spinner = harness.bounds(cx, "thread-row-0-status-running-spinner").expect("running thread has a spinner");
    assert!(harness.bounds(cx, "thread-row-0-conversation-icon").is_none());
    assert_eq!(spinner.left(), icon.left());
    assert_eq!(harness.bounds(cx, "thread-row-0-title").unwrap().left(), title_left);

    let (_, responder) = requests.prompt.try_recv().unwrap();
    responder.respond(PromptResponse::new("user-message")).unwrap();
    requests.connection.try_recv().unwrap().send_notification(idle_notification("alpha-session", None)).unwrap();
    harness.run_until(cx, |cx| alpha.read(cx).status() == ThreadStatus::Idle);

    assert!(harness.bounds(cx, "thread-row-0-conversation-icon").is_some());
    assert!(harness.bounds(cx, "thread-row-0-status-running").is_none());
    assert_eq!(harness.bounds(cx, "thread-row-0-title").unwrap().left(), title_left);
}

#[gpui::test]
fn a_thread_waiting_on_an_answer_shows_a_speech_bubble(cx: &mut TestAppContext) {
    let agent = FakeAgent::default()
        .new_session_response(NewSessionResponse::new("alpha-session"))
        .prompt_elicitation("Pick a branch");
    let harness = DesktopHarnessBuilder::default().agent("/alpha", agent).build(cx);
    let alpha = harness.start_thread(cx, "/alpha");

    alpha.update(cx, |thread, cx| thread.submit("pick one", cx));
    harness.run_until(cx, |cx| alpha.read(cx).status() == ThreadStatus::NeedsInput);

    assert!(harness.bounds(cx, "thread-row-0-status-needs-input-speech-bubble").is_some());
    assert!(harness.bounds(cx, "thread-row-0-conversation-icon").is_none());
    assert!(harness.bounds(cx, "thread-row-0-status-running").is_none(), "waiting is not busy");
}

#[gpui::test]
fn a_saved_session_has_no_activity_icon(cx: &mut TestAppContext) {
    let harness = DesktopHarnessBuilder::default()
        .session("saved", "/alpha", "2026-01-01T00:00:00Z", "prompt")
        .project("/alpha")
        .build(cx);

    assert!(harness.bounds(cx, "history-row-saved-conversation-icon").is_some());
    assert!(harness.bounds(cx, "history-row-saved-status-dormant").is_none());
    assert!(harness.bounds(cx, "history-row-saved-unread").is_none());
}

/// The notification a thread with the session `tag` posts, with no action buttons.
fn notification(tag: &str, title: &str, body: &str) -> SystemNotification {
    SystemNotification { tag: tag.into(), title: title.into(), body: body.into(), actions: Vec::new() }
}

/// A connected thread in `/alpha`, left unselected by starting a second one in `/beta`.
fn unselected_alpha(cx: &mut TestAppContext) -> (DesktopHarness, Entity<Thread>) {
    let harness = DesktopHarnessBuilder::default()
        .agent("/alpha", replying("alpha-session", "from alpha"))
        .agent("/beta", replying("beta-session", "from beta"))
        .build(cx);
    let alpha = harness.start_thread(cx, "/alpha");
    harness.start_thread(cx, "/beta");
    (harness, alpha)
}
