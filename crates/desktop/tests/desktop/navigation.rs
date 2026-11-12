use crate::support::replying;
use acp_utils::testing::FakeAgent;
use aether_desktop::testing::{DesktopHarness, DesktopHarnessBuilder};
use agent_client_protocol::v2::schema::SessionId;

#[test]
fn the_cycling_shortcuts_step_through_live_threads_in_sidebar_order_and_wrap() {
    let mut harness = three_projects();
    let [alpha, beta, gamma] = ["/alpha", "/beta", "/gamma"].map(|cwd| harness.start_thread(cwd));

    harness.press("secondary-[");
    assert_eq!(harness.selected_thread(), Some(beta));

    harness.press("secondary-]");
    assert_eq!(harness.selected_thread(), Some(gamma));

    harness.press("secondary-]");
    assert_eq!(harness.selected_thread(), Some(alpha), "stepping past the last thread wraps to the first");

    harness.press("secondary-[");
    assert_eq!(harness.selected_thread(), Some(gamma), "stepping before the first thread wraps to the last");
}

#[test]
fn the_switcher_finds_a_live_thread_by_its_title_and_selects_it() {
    let mut harness = three_projects();
    let alpha = harness.start_thread("/alpha");
    harness.submit(alpha, "Fix the build");
    harness.start_thread("/beta");

    harness.press("secondary-k");
    assert!(harness.bounds("switcher").is_some(), "the switcher opens");
    harness.type_text("fix");
    assert!(harness.bounds("switcher-thread-row-1").is_none(), "beta does not match");
    harness.press("enter");

    assert_eq!(harness.selected_thread(), Some(alpha));
    assert!(harness.bounds("switcher").is_none(), "choosing a thread closes the switcher");
}

#[test]
fn the_switcher_resumes_a_saved_session() {
    let mut harness = DesktopHarnessBuilder::default()
        .agent("/alpha", FakeAgent::default().replay_message("saved-session", "an earlier answer"))
        .session("saved-session", "/alpha", "2026-01-01T00:00:00Z", "Refactor auth")
        .build();

    harness.press("secondary-k");
    harness.type_text("auth");
    harness.click("switcher-history-row-saved-session").unwrap();

    let thread = harness.selected_thread().expect("the resumed thread is selected");
    assert_eq!(thread.session_id(), Some(SessionId::new("saved-session")));
}

#[test]
fn the_arrow_keys_move_the_switchers_highlight_and_escape_keeps_the_selection() {
    let mut harness = three_projects();
    let [_, beta, gamma] = ["/alpha", "/beta", "/gamma"].map(|cwd| harness.start_thread(cwd));

    harness.press("secondary-k down escape");
    assert!(harness.bounds("switcher").is_none());
    assert_eq!(harness.selected_thread(), Some(gamma));

    harness.press("secondary-k down down up enter");
    assert_eq!(harness.selected_thread(), Some(beta), "the second of the threads, listed in sidebar order");
}

fn three_projects() -> DesktopHarness {
    DesktopHarnessBuilder::default()
        .agent("/alpha", replying("alpha-session", "a"))
        .agent("/beta", replying("beta-session", "b"))
        .agent("/gamma", replying("gamma-session", "c"))
        .build()
}
