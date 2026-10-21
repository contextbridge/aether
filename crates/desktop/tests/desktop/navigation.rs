use crate::support::{replying, save_session, texts, wait_for_history};
use acp_utils::testing::FakeAgent;
use aether_desktop::testing::{DesktopHarness, DesktopHarnessBuilder, FakeConnector};
use aether_desktop::threads::{Thread, ThreadId, ThreadStatus};
use aether_sessions::testing::TestStore;
use agent_client_protocol::schema::v2::SessionId;
use gpui::{Entity, TestAppContext};

#[gpui::test]
fn the_cycling_shortcuts_step_through_live_threads_in_sidebar_order_and_wrap(cx: &mut TestAppContext) {
    let connector = FakeConnector::default()
        .agent("/alpha", replying("alpha-session", "a"))
        .agent("/beta", replying("beta-session", "b"))
        .agent("/gamma", replying("gamma-session", "c"));
    let harness = DesktopHarnessBuilder::default().connector(connector).build(cx);
    let [alpha, beta, gamma] = ["/alpha", "/beta", "/gamma"].map(|cwd| id(&idle_thread(&harness, cx, cwd), cx));

    // The composer of the selected thread has focus, as when the user is writing a prompt.
    harness.simulate_keystrokes(cx, "secondary-[");
    assert_eq!(selected(&harness, cx), Some(beta));

    harness.simulate_keystrokes(cx, "secondary-]");
    assert_eq!(selected(&harness, cx), Some(gamma));

    harness.simulate_keystrokes(cx, "secondary-]");
    assert_eq!(selected(&harness, cx), Some(alpha), "stepping past the last thread wraps to the first");

    harness.simulate_keystrokes(cx, "secondary-[");
    assert_eq!(selected(&harness, cx), Some(gamma), "stepping before the first thread wraps to the last");
}

#[gpui::test]
fn the_switcher_finds_a_live_thread_by_its_title_and_selects_it(cx: &mut TestAppContext) {
    let connector = FakeConnector::default()
        .agent("/alpha", replying("alpha-session", "a"))
        .agent("/beta", replying("beta-session", "b"));
    let harness = DesktopHarnessBuilder::default().connector(connector).build(cx);
    let alpha = idle_thread(&harness, cx, "/alpha");
    alpha.update(cx, |thread, cx| thread.submit("Fix the build", cx));
    harness.run_until(cx, |cx| texts(alpha.read(cx)).len() == 2);
    idle_thread(&harness, cx, "/beta");

    harness.simulate_keystrokes(cx, "secondary-k");
    assert!(harness.bounds(cx, "thread-switcher").is_some(), "the switcher opens");
    harness.simulate_input(cx, "fix");
    assert!(harness.bounds(cx, "switcher-thread-1").is_none(), "beta does not match");
    harness.simulate_keystrokes(cx, "enter");

    assert_eq!(selected(&harness, cx), Some(id(&alpha, cx)));
    assert!(harness.bounds(cx, "thread-switcher").is_none(), "choosing a thread closes the switcher");
}

#[gpui::test]
fn the_switcher_resumes_a_saved_session(cx: &mut TestAppContext) {
    let sessions = TestStore::new();
    save_session(&sessions, "saved-session", "/alpha", "2026-01-01T00:00:00Z", Some("Refactor auth"));
    let connector = FakeConnector::default()
        .agent("/alpha", FakeAgent::default().replay_message("saved-session", "an earlier answer"));
    let harness = DesktopHarnessBuilder::default().connector(connector).history(sessions).build(cx);
    wait_for_history(&harness, cx);

    harness.simulate_keystrokes(cx, "secondary-k");
    harness.simulate_input(cx, "auth");
    harness.click(cx, "switcher-session-saved-session").unwrap();

    let thread = harness.selected_thread(cx).expect("the resumed thread is selected");
    assert_eq!(thread.read_with(cx, |thread, _| thread.session_id().cloned()), Some(SessionId::new("saved-session")));
}

#[gpui::test]
fn the_arrow_keys_move_the_switchers_highlight(cx: &mut TestAppContext) {
    let connector = FakeConnector::default()
        .agent("/alpha", replying("alpha-session", "a"))
        .agent("/beta", replying("beta-session", "b"))
        .agent("/gamma", replying("gamma-session", "c"));
    let harness = DesktopHarnessBuilder::default().connector(connector).build(cx);
    let [_, beta, _] = ["/alpha", "/beta", "/gamma"].map(|cwd| id(&idle_thread(&harness, cx, cwd), cx));

    harness.simulate_keystrokes(cx, "secondary-k down down up enter");

    assert_eq!(selected(&harness, cx), Some(beta), "the second of the threads, listed in sidebar order");
}

#[gpui::test]
fn escape_closes_the_switcher_and_keeps_the_selection(cx: &mut TestAppContext) {
    let connector = FakeConnector::default()
        .agent("/alpha", replying("alpha-session", "a"))
        .agent("/beta", replying("beta-session", "b"));
    let harness = DesktopHarnessBuilder::default().connector(connector).build(cx);
    idle_thread(&harness, cx, "/alpha");
    let beta = id(&idle_thread(&harness, cx, "/beta"), cx);

    harness.simulate_keystrokes(cx, "secondary-k down escape");

    assert!(harness.bounds(cx, "thread-switcher").is_none());
    assert_eq!(selected(&harness, cx), Some(beta));
}

/// Starts a thread in `cwd` and waits for its agent to connect.
fn idle_thread(harness: &DesktopHarness, cx: &mut TestAppContext, cwd: &str) -> Entity<Thread> {
    let thread = harness.threads(cx).update(cx, |threads, cx| threads.new_thread(cwd.into(), cx));
    harness.run_until(cx, |cx| thread.read(cx).status() == ThreadStatus::Idle);
    thread
}

fn id(thread: &Entity<Thread>, cx: &TestAppContext) -> ThreadId {
    thread.read_with(cx, |thread, _| thread.id())
}

fn selected(harness: &DesktopHarness, cx: &TestAppContext) -> Option<ThreadId> {
    harness.threads(cx).read_with(cx, |threads, _| threads.selected())
}
