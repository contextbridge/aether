use crate::support::{replying, save_session, texts, wait_for_history};
use acp_utils::testing::FakeAgent;
use aether_desktop::testing::{DesktopHarness, DesktopHarnessBuilder, FakeConnector};
use aether_desktop::threads::{Thread, ThreadStatus};
use aether_sessions::testing::TestStore;
use agent_client_protocol::schema::v2::NewSessionResponse;
use gpui::{Entity, SystemNotification, TestAppContext};

#[gpui::test]
fn a_turn_that_finishes_on_an_unselected_thread_stays_unread_until_selected(cx: &mut TestAppContext) {
    let connector = FakeConnector::default()
        .agent("/alpha", replying("alpha-session", "from alpha"))
        .agent("/beta", replying("beta-session", "from beta"));
    let harness = DesktopHarnessBuilder::default().connector(connector).build(cx);
    let alpha = idle_thread(&harness, cx, "/alpha");
    idle_thread(&harness, cx, "/beta");
    let alpha_id = alpha.read_with(cx, |thread, _| thread.id());

    alpha.update(cx, |thread, cx| thread.submit("hi", cx));
    harness.run_until(cx, |cx| texts(alpha.read(cx)).len() == 2 && alpha.read(cx).status() == ThreadStatus::Idle);

    assert!(harness.threads(cx).read_with(cx, |threads, _| threads.is_unread(alpha_id)));
    assert!(harness.bounds(cx, "thread-row-0-status-unread").is_some(), "alpha shows the unread dot");

    harness.click(cx, "thread-row-0").unwrap();

    assert!(!harness.threads(cx).read_with(cx, |threads, _| threads.is_unread(alpha_id)));
    assert!(harness.bounds(cx, "thread-row-0-status-unread").is_none(), "selecting alpha marks it seen");
}

#[gpui::test]
fn a_turn_that_finishes_on_the_selected_thread_is_neither_unread_nor_announced(cx: &mut TestAppContext) {
    let connector = FakeConnector::default().agent("/alpha", replying("alpha-session", "from alpha"));
    let harness = DesktopHarnessBuilder::default().connector(connector).build(cx);
    let alpha = idle_thread(&harness, cx, "/alpha");
    let alpha_id = alpha.read_with(cx, |thread, _| thread.id());

    alpha.update(cx, |thread, cx| thread.submit("hi", cx));
    harness.run_until(cx, |cx| texts(alpha.read(cx)).len() == 2 && alpha.read(cx).status() == ThreadStatus::Idle);

    assert!(!harness.threads(cx).read_with(cx, |threads, _| threads.is_unread(alpha_id)));
    assert!(harness.bounds(cx, "thread-row-0-status-unread").is_none());
    assert_eq!(cx.shown_system_notifications(), []);
}

#[gpui::test]
fn a_turn_that_finishes_on_an_unselected_thread_posts_one_notification(cx: &mut TestAppContext) {
    let connector = FakeConnector::default()
        .agent("/alpha", replying("alpha-session", "from alpha"))
        .agent("/beta", replying("beta-session", "from beta"));
    let harness = DesktopHarnessBuilder::default().connector(connector).build(cx);
    let alpha = idle_thread(&harness, cx, "/alpha");
    idle_thread(&harness, cx, "/beta");

    alpha.update(cx, |thread, cx| thread.submit("Fix the build", cx));
    harness.run_until(cx, |cx| texts(alpha.read(cx)).len() == 2 && alpha.read(cx).status() == ThreadStatus::Idle);

    assert_eq!(
        cx.shown_system_notifications(),
        [notification("alpha-session", "Fix the build", "The agent finished its turn")]
    );
}

#[gpui::test]
fn a_turn_that_finishes_while_the_window_is_inactive_stays_unread_until_the_window_is_back(cx: &mut TestAppContext) {
    let connector = FakeConnector::default().agent("/alpha", replying("alpha-session", "from alpha"));
    let harness = DesktopHarnessBuilder::default().connector(connector).build(cx);
    let alpha = idle_thread(&harness, cx, "/alpha");
    let alpha_id = alpha.read_with(cx, |thread, _| thread.id());
    harness.deactivate(cx);

    alpha.update(cx, |thread, cx| thread.submit("hi", cx));
    harness.run_until(cx, |cx| texts(alpha.read(cx)).len() == 2 && alpha.read(cx).status() == ThreadStatus::Idle);

    assert_eq!(cx.shown_system_notifications(), [notification("alpha-session", "hi", "The agent finished its turn")]);
    assert!(harness.threads(cx).read_with(cx, |threads, _| threads.is_unread(alpha_id)));

    harness.activate(cx);

    assert!(!harness.threads(cx).read_with(cx, |threads, _| threads.is_unread(alpha_id)));
}

#[gpui::test]
fn a_question_on_an_unselected_thread_is_announced_with_the_question(cx: &mut TestAppContext) {
    let alpha_agent = FakeAgent::default()
        .new_session_response(NewSessionResponse::new("alpha-session"))
        .prompt_elicitation("Pick a branch");
    let connector =
        FakeConnector::default().agent("/alpha", alpha_agent).agent("/beta", replying("beta-session", "from beta"));
    let harness = DesktopHarnessBuilder::default().connector(connector).build(cx);
    let alpha = idle_thread(&harness, cx, "/alpha");
    idle_thread(&harness, cx, "/beta");

    alpha.update(cx, |thread, cx| thread.submit("pick one", cx));
    harness.run_until(cx, |cx| alpha.read(cx).status() == ThreadStatus::NeedsInput);

    assert_eq!(cx.shown_system_notifications(), [notification("alpha-session", "pick one", "Pick a branch")]);
}

#[gpui::test]
fn clicking_a_notification_brings_the_window_back_with_its_thread_selected(cx: &mut TestAppContext) {
    let connector = FakeConnector::default()
        .agent("/alpha", replying("alpha-session", "from alpha"))
        .agent("/beta", replying("beta-session", "from beta"));
    let harness = DesktopHarnessBuilder::default().connector(connector).build(cx);
    let alpha = idle_thread(&harness, cx, "/alpha");
    idle_thread(&harness, cx, "/beta");
    let alpha_id = alpha.read_with(cx, |thread, _| thread.id());
    harness.deactivate(cx);
    alpha.update(cx, |thread, cx| thread.submit("hi", cx));
    harness.run_until(cx, |cx| texts(alpha.read(cx)).len() == 2 && alpha.read(cx).status() == ThreadStatus::Idle);

    harness.click_notification(cx, "alpha-session");

    assert!(harness.is_active(cx));
    assert_eq!(harness.threads(cx).read_with(cx, |threads, _| threads.selected()), Some(alpha_id));
    assert!(!harness.threads(cx).read_with(cx, |threads, _| threads.is_unread(alpha_id)));
}

#[gpui::test]
fn a_running_thread_shows_the_running_dot(cx: &mut TestAppContext) {
    let (agent, requests) =
        FakeAgent::default().new_session_response(NewSessionResponse::new("alpha-session")).capture();
    let connector = FakeConnector::default().agent("/alpha", agent);
    let harness = DesktopHarnessBuilder::default().connector(connector).build(cx);
    let alpha = idle_thread(&harness, cx, "/alpha");
    assert!(harness.bounds(cx, "thread-row-0-status-running").is_none(), "an idle thread shows no dot");

    alpha.update(cx, |thread, cx| thread.submit("work forever", cx));
    harness.run_until(cx, |_| !requests.prompt.is_empty());

    assert!(harness.bounds(cx, "thread-row-0-status-running").is_some());
}

#[gpui::test]
fn a_thread_waiting_on_an_answer_shows_the_needs_input_dot(cx: &mut TestAppContext) {
    let agent = FakeAgent::default()
        .new_session_response(NewSessionResponse::new("alpha-session"))
        .prompt_elicitation("Pick a branch");
    let connector = FakeConnector::default().agent("/alpha", agent);
    let harness = DesktopHarnessBuilder::default().connector(connector).build(cx);
    let alpha = idle_thread(&harness, cx, "/alpha");

    alpha.update(cx, |thread, cx| thread.submit("pick one", cx));
    harness.run_until(cx, |cx| alpha.read(cx).status() == ThreadStatus::NeedsInput);

    assert!(harness.bounds(cx, "thread-row-0-status-needs-input").is_some());
}

#[gpui::test]
fn a_thread_whose_agent_cannot_start_shows_the_errored_dot(cx: &mut TestAppContext) {
    let harness = DesktopHarnessBuilder::default().build(cx);

    let thread = harness.threads(cx).update(cx, |threads, cx| threads.new_thread("/nowhere".into(), cx));
    harness.run_until(cx, |cx| matches!(thread.read(cx).status(), ThreadStatus::Errored(_)));

    assert!(harness.bounds(cx, "thread-row-0-status-errored").is_some());
}

#[gpui::test]
fn a_saved_session_shows_the_dormant_dot(cx: &mut TestAppContext) {
    let sessions = TestStore::new();
    save_session(&sessions, "saved", "/alpha", "2026-01-01T00:00:00Z", Some("prompt"));
    let harness = DesktopHarnessBuilder::default().history(sessions).project("/alpha").build(cx);
    wait_for_history(&harness, cx);

    assert!(harness.bounds(cx, "history-row-saved-status-dormant").is_some());
}

/// The notification a thread with the session `tag` posts, with no action buttons.
fn notification(tag: &str, title: &str, body: &str) -> SystemNotification {
    SystemNotification { tag: tag.into(), title: title.into(), body: body.into(), actions: Vec::new() }
}

/// Starts a thread in `cwd` and waits for its agent to connect.
fn idle_thread(harness: &DesktopHarness, cx: &mut TestAppContext, cwd: &str) -> Entity<Thread> {
    let thread = harness.threads(cx).update(cx, |threads, cx| threads.new_thread(cwd.into(), cx));
    harness.run_until(cx, |cx| thread.read(cx).status() == ThreadStatus::Idle);
    thread
}
