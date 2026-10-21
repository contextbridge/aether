use crate::support::{save_session, wait_for_history};
use acp_utils::testing::FakeAgent;
use aether_desktop::registry::{Registry, STATE_FILE};
use aether_desktop::testing::{DesktopHarness, DesktopHarnessBuilder, FakeConnector};
use aether_desktop::threads::ThreadStatus;
use aether_desktop::{SIDEBAR_WIDTH, ThemePair};
use aether_sessions::testing::TestStore;
use agent_client_protocol::schema::v2::SessionId;
use clankerdiff_theme::ThemeId;
use gpui::{AppContext, TestAppContext, px, size};
use std::path::{Path, PathBuf};

#[gpui::test]
fn an_added_project_is_listed_on_the_next_launch(first: &mut TestAppContext, second: &mut TestAppContext) {
    let state = tempfile::tempdir().unwrap();
    let harness = DesktopHarnessBuilder::default().state_dir(state.path()).build(first);
    harness.click(first, "add-project").unwrap();
    first.simulate_path_prompt_response(|_| Some(vec![PathBuf::from("/picked")]));
    first.run_until_parked();

    let relaunched = DesktopHarnessBuilder::default().state_dir(state.path()).build(second);

    assert!(relaunched.bounds(second, "project-picked").is_some());
}

#[gpui::test]
fn a_projects_disclosure_is_kept_across_launches(first: &mut TestAppContext, second: &mut TestAppContext) {
    let state = tempfile::tempdir().unwrap();
    let harness = launch(first, state.path(), saved_in_alpha(), FakeConnector::default());
    assert!(harness.bounds(first, "history-row-saved").is_none(), "a project only history knows starts collapsed");
    harness.click(first, "project-alpha-toggle").unwrap();
    first.run_until_parked();

    let relaunched = launch(second, state.path(), saved_in_alpha(), FakeConnector::default());

    assert!(relaunched.bounds(second, "history-row-saved").is_some());
}

#[gpui::test]
fn the_last_selected_session_is_resumed_and_selected_on_launch(
    first: &mut TestAppContext,
    second: &mut TestAppContext,
) {
    let state = tempfile::tempdir().unwrap();
    let harness = launch(first, state.path(), saved_in_alpha(), resuming("saved", "an earlier answer"));
    let thread = harness
        .threads(first)
        .update(first, |threads, cx| threads.resume(SessionId::new("saved"), "/alpha".into(), cx));
    harness.run_until(first, |cx| thread.read(cx).status() == ThreadStatus::Idle);

    let connector = resuming("saved", "an earlier answer");
    let relaunched = launch(second, state.path(), saved_in_alpha(), connector.clone());
    let threads = relaunched.threads(second);
    relaunched.run_until(second, |cx| {
        let threads = threads.read(cx);
        threads
            .selected()
            .and_then(|id| threads.thread(id, cx))
            .is_some_and(|thread| thread.read(cx).status() == ThreadStatus::Idle)
    });

    let thread = relaunched.selected_thread(second).expect("the session is selected again");
    assert_eq!(thread.read_with(second, |thread, _| thread.session_id().cloned()), Some(SessionId::new("saved")));
    assert_eq!(connector.live_agents("/alpha"), 1);
    assert!(relaunched.bounds(second, "thread-row-0").is_some(), "its project expands to show it");
}

#[gpui::test]
fn a_last_selected_session_missing_from_history_is_not_resumed(
    first: &mut TestAppContext,
    second: &mut TestAppContext,
) {
    let state = tempfile::tempdir().unwrap();
    let registry = first.new(|cx| Registry::load(Some(state.path().join(STATE_FILE)), cx));
    registry.update(first, |registry, cx| registry.set_last_selected(Some(SessionId::new("deleted")), cx));
    first.run_until_parked();

    let relaunched = launch(second, state.path(), saved_in_alpha(), FakeConnector::default());
    wait_for_history(&relaunched, second);

    assert!(relaunched.selected_thread(second).is_none());
}

#[gpui::test]
fn the_window_opens_at_the_size_it_last_had(first: &mut TestAppContext, second: &mut TestAppContext) {
    let state = tempfile::tempdir().unwrap();
    let harness = DesktopHarnessBuilder::default().state_dir(state.path()).build(first);
    harness.resize(first, 900.0, 700.0);
    first.run_until_parked();

    let relaunched = DesktopHarnessBuilder::default().state_dir(state.path()).build(second);

    let main = relaunched.bounds(second, "main").expect("main region is painted");
    assert_eq!(main.size, size(px(900.0) - SIDEBAR_WIDTH, px(700.0)));
}

#[gpui::test]
fn the_saved_theme_override_is_installed_on_launch(first: &mut TestAppContext, second: &mut TestAppContext) {
    let state = tempfile::tempdir().unwrap();
    let registry = first.new(|cx| Registry::load(Some(state.path().join(STATE_FILE)), cx));
    let theme = ThemePair { light: "solarized-light".into(), dark: "solarized-dark".into() };
    registry.update(first, |registry, cx| registry.set_theme_override(Some(theme), cx));
    first.run_until_parked();

    let relaunched = DesktopHarnessBuilder::default().state_dir(state.path()).build(second);

    // gpui's test windows report a light appearance.
    assert_eq!(relaunched.theme(second).review().id(), &ThemeId::Builtin("solarized-light".to_owned()));
}

/// One saved session, `saved`, that ran in `/alpha`.
fn saved_in_alpha() -> TestStore {
    let sessions = TestStore::new();
    save_session(&sessions, "saved", "/alpha", "2026-01-01T00:00:00Z", Some("earlier prompt"));
    sessions
}

/// A connector whose one agent in `/alpha` resumes `session_id`, replaying `answer`.
fn resuming(session_id: &str, answer: &str) -> FakeConnector {
    FakeConnector::default().agent("/alpha", FakeAgent::default().replay_message(session_id, answer))
}

/// Opens the app over the state kept in `state`, with `sessions` in history, and waits for
/// history to be read.
fn launch(cx: &mut TestAppContext, state: &Path, sessions: TestStore, connector: FakeConnector) -> DesktopHarness {
    let harness = DesktopHarnessBuilder::default().state_dir(state).history(sessions).connector(connector).build(cx);
    wait_for_history(&harness, cx);
    harness
}
