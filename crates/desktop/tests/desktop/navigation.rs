use crate::support::replying;
use acp_utils::testing::FakeAgent;
use aether_desktop::testing::{DesktopHarness, DesktopHarnessBuilder};
use agent_client_protocol::schema::v2::SessionId;
use gpui::TestAppContext;

#[gpui::test]
fn the_cycling_shortcuts_step_through_live_threads_in_sidebar_order_and_wrap(cx: &mut TestAppContext) {
    let harness = three_projects(cx);
    let [alpha, beta, gamma] = ["/alpha", "/beta", "/gamma"].map(|cwd| harness.start_thread(cx, cwd));

    // The composer of the selected thread has focus, as when the user is writing a prompt.
    harness.simulate_keystrokes(cx, "secondary-[");
    assert_eq!(harness.selected_thread(cx), Some(beta));

    harness.simulate_keystrokes(cx, "secondary-]");
    assert_eq!(harness.selected_thread(cx).as_ref(), Some(&gamma));

    harness.simulate_keystrokes(cx, "secondary-]");
    assert_eq!(harness.selected_thread(cx), Some(alpha), "stepping past the last thread wraps to the first");

    harness.simulate_keystrokes(cx, "secondary-[");
    assert_eq!(harness.selected_thread(cx), Some(gamma), "stepping before the first thread wraps to the last");
}

#[gpui::test]
fn the_switcher_finds_a_live_thread_by_its_title_and_selects_it(cx: &mut TestAppContext) {
    let harness = three_projects(cx);
    let alpha = harness.start_thread(cx, "/alpha");
    harness.submit(cx, &alpha, "Fix the build");
    harness.start_thread(cx, "/beta");

    harness.simulate_keystrokes(cx, "secondary-k");
    assert!(harness.bounds(cx, "thread-switcher").is_some(), "the switcher opens");
    harness.simulate_input(cx, "fix");
    assert!(harness.bounds(cx, "switcher-thread-1").is_none(), "beta does not match");
    harness.simulate_keystrokes(cx, "enter");

    assert_eq!(harness.selected_thread(cx), Some(alpha));
    assert!(harness.bounds(cx, "thread-switcher").is_none(), "choosing a thread closes the switcher");
}

#[gpui::test]
fn the_switcher_resumes_a_saved_session(cx: &mut TestAppContext) {
    let harness = DesktopHarnessBuilder::default()
        .agent("/alpha", FakeAgent::default().replay_message("saved-session", "an earlier answer"))
        .session("saved-session", "/alpha", "2026-01-01T00:00:00Z", "Refactor auth")
        .build(cx);

    harness.simulate_keystrokes(cx, "secondary-k");
    harness.simulate_input(cx, "auth");
    harness.click(cx, "switcher-session-saved-session").unwrap();

    let thread = harness.selected_thread(cx).expect("the resumed thread is selected");
    assert_eq!(thread.read_with(cx, |thread, _| thread.session_id().cloned()), Some(SessionId::new("saved-session")));
}

#[gpui::test]
fn the_arrow_keys_move_the_switchers_highlight_and_escape_keeps_the_selection(cx: &mut TestAppContext) {
    let harness = three_projects(cx);
    let [_, beta, gamma] = ["/alpha", "/beta", "/gamma"].map(|cwd| harness.start_thread(cx, cwd));

    harness.simulate_keystrokes(cx, "secondary-k down escape");
    assert!(harness.bounds(cx, "thread-switcher").is_none());
    assert_eq!(harness.selected_thread(cx), Some(gamma));

    harness.simulate_keystrokes(cx, "secondary-k down down up enter");
    assert_eq!(harness.selected_thread(cx), Some(beta), "the second of the threads, listed in sidebar order");
}

fn three_projects(cx: &mut TestAppContext) -> DesktopHarness {
    DesktopHarnessBuilder::default()
        .agent("/alpha", replying("alpha-session", "a"))
        .agent("/beta", replying("beta-session", "b"))
        .agent("/gamma", replying("gamma-session", "c"))
        .build(cx)
}
