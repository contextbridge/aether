use crate::support::{replying, save_session, texts, wait_for_history};
use acp_utils::testing::{FakeAgent, idle_notification};
use aether_desktop::testing::{DesktopHarness, DesktopHarnessBuilder, FakeConnector};
use aether_desktop::threads::{Thread, ThreadStatus};
use aether_sessions::testing::TestStore;
use agent_client_protocol::schema::v2::{NewSessionResponse, PromptResponse};
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
    let unread = harness.bounds(cx, "thread-row-0-unread").expect("alpha shows the unread dot");
    let title = harness.bounds(cx, "thread-row-0-title").expect("alpha shows its title");
    assert!(unread.left() >= title.right(), "unread is after the title, not in the activity slot");
    assert!(harness.bounds(cx, "thread-row-0-conversation-icon").is_some());
    assert_hint(&harness, cx, "thread-row-0-unread", "Unread response");

    harness.click(cx, "thread-row-0").unwrap();

    assert!(!harness.threads(cx).read_with(cx, |threads, _| threads.is_unread(alpha_id)));
    assert!(harness.bounds(cx, "thread-row-0-unread").is_none(), "selecting alpha marks it seen");
}

#[gpui::test]
fn an_unread_response_stays_visible_when_the_agent_disconnects(cx: &mut TestAppContext) {
    let connector = FakeConnector::default()
        .agent("/alpha", replying("alpha-session", "from alpha"))
        .agent("/beta", replying("beta-session", "from beta"));
    let harness = DesktopHarnessBuilder::default().connector(connector.clone()).build(cx);
    let alpha = idle_thread(&harness, cx, "/alpha");
    idle_thread(&harness, cx, "/beta");
    alpha.update(cx, |thread, cx| thread.submit("hi", cx));
    harness.run_until(cx, |cx| texts(alpha.read(cx)).len() == 2 && alpha.read(cx).status() == ThreadStatus::Idle);

    connector.kill_agents("/alpha");
    harness.run_until(cx, |cx| alpha.read(cx).status() == ThreadStatus::Disconnected);

    assert!(harness.bounds(cx, "thread-row-0-status-disconnected-warning-triangle").is_some());
    assert!(harness.bounds(cx, "thread-row-0-conversation-icon").is_none());
    assert!(harness.bounds(cx, "thread-row-0-unread").is_some());
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
    assert!(harness.bounds(cx, "thread-row-0-unread").is_none());
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
fn activity_replaces_the_conversation_icon_without_moving_the_title(cx: &mut TestAppContext) {
    let (agent, mut requests) =
        FakeAgent::default().new_session_response(NewSessionResponse::new("alpha-session")).capture();
    let connector = FakeConnector::default().agent("/alpha", agent);
    let harness = DesktopHarnessBuilder::default().connector(connector).build(cx);
    let alpha = idle_thread(&harness, cx, "/alpha");
    assert!(harness.bounds(cx, "thread-row-0-status-running").is_none(), "an idle thread shows no activity icon");
    let icon = harness.bounds(cx, "thread-row-0-conversation-icon").expect("idle thread has a conversation icon");
    let title_left = harness.bounds(cx, "thread-row-0-title").unwrap().left();

    alpha.update(cx, |thread, cx| thread.submit("work forever", cx));
    harness.run_until(cx, |_| !requests.prompt.is_empty());

    let spinner = harness.bounds(cx, "thread-row-0-status-running-spinner").expect("running thread has a spinner");
    assert!(harness.bounds(cx, "thread-row-0-conversation-icon").is_none());
    assert_eq!(spinner.left(), icon.left());
    assert_eq!(harness.bounds(cx, "thread-row-0-title").unwrap().left(), title_left);
    assert_hint(&harness, cx, "thread-row-0-status-running", "Agent working");

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
    let connector = FakeConnector::default().agent("/alpha", agent);
    let harness = DesktopHarnessBuilder::default().connector(connector).build(cx);
    let alpha = idle_thread(&harness, cx, "/alpha");

    alpha.update(cx, |thread, cx| thread.submit("pick one", cx));
    harness.run_until(cx, |cx| alpha.read(cx).status() == ThreadStatus::NeedsInput);

    assert!(harness.bounds(cx, "thread-row-0-status-needs-input-speech-bubble").is_some());
    assert!(harness.bounds(cx, "thread-row-0-conversation-icon").is_none());
    assert!(harness.bounds(cx, "thread-row-0-status-running").is_none(), "waiting is not busy");
    assert_hint(&harness, cx, "thread-row-0-status-needs-input", "Waiting for your answer");
}

#[gpui::test]
fn a_thread_whose_agent_cannot_start_shows_a_warning_triangle(cx: &mut TestAppContext) {
    let harness = DesktopHarnessBuilder::default().build(cx);

    let thread = harness.new_thread(cx, "/nowhere");
    harness.run_until(cx, |cx| matches!(thread.read(cx).status(), ThreadStatus::Errored(_)));

    assert!(harness.bounds(cx, "thread-row-0-status-errored-warning-triangle").is_some());
    assert!(harness.bounds(cx, "thread-row-0-conversation-icon").is_none());
    assert_hint(&harness, cx, "thread-row-0-status-errored", "Unable to connect — reconnect to try again");
}

#[gpui::test]
fn a_saved_session_has_a_muted_title_and_no_activity_icon(cx: &mut TestAppContext) {
    let sessions = TestStore::new();
    save_session(&sessions, "saved", "/alpha", "2026-01-01T00:00:00Z", Some("prompt"));
    let harness = DesktopHarnessBuilder::default().history(sessions).project("/alpha").build(cx);
    wait_for_history(&harness, cx);

    let title = harness.painted_text(cx, "prompt").expect("saved title is painted");
    assert_eq!(title.foreground, harness.theme(cx).ui().colors.text_muted);
    assert!(harness.bounds(cx, "history-row-saved-conversation-icon").is_some());
    assert!(harness.bounds(cx, "history-row-saved-status-dormant").is_none());
    assert!(harness.bounds(cx, "history-row-saved-unread").is_none());
}

/// The notification a thread with the session `tag` posts, with no action buttons.
fn notification(tag: &str, title: &str, body: &str) -> SystemNotification {
    SystemNotification { tag: tag.into(), title: title.into(), body: body.into(), actions: Vec::new() }
}

/// Starts a thread in `cwd` and waits for its agent to connect.
fn idle_thread(harness: &DesktopHarness, cx: &mut TestAppContext, cwd: &str) -> Entity<Thread> {
    let thread = harness.new_thread(cx, cwd);
    harness.run_until(cx, |cx| thread.read(cx).status() == ThreadStatus::Idle);
    thread
}

fn assert_hint(harness: &DesktopHarness, cx: &mut TestAppContext, selector: &'static str, label: &str) {
    harness.hover(cx, selector).unwrap();
    cx.executor().advance_clock(std::time::Duration::from_secs(1));
    cx.run_until_parked();
    assert!(harness.painted_text(cx, label).is_some(), "hovering {selector} explains its status");
}
