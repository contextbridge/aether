use crate::support::{choose, replying, save_session, texts, wait_for_history};
use aether_desktop::testing::{DesktopHarness, DesktopHarnessBuilder, FakeConnector};
use aether_desktop::threads::{Thread, ThreadStatus};
use aether_sessions::testing::TestStore;
use gpui::{Entity, TestAppContext};
use std::path::Path;

#[gpui::test]
fn renaming_a_thread_retitles_its_row_and_header_until_the_name_is_cleared(cx: &mut TestAppContext) {
    let connector = FakeConnector::default().agent("/alpha", replying("alpha-session", "hello"));
    let harness = DesktopHarnessBuilder::default().connector(connector).project("/alpha").build(cx);
    prompted_thread(&harness, cx, "first prompt");
    assert_eq!(harness.row_title(cx, "thread-row-0").as_deref(), Some("first prompt"));

    choose(&harness, cx, "thread-row-0", "menu-rename");
    harness.simulate_input(cx, "Renamed");
    harness.simulate_keystrokes(cx, "enter");

    assert!(harness.bounds(cx, "rename-input").is_none(), "Enter ends the edit");
    assert_eq!(harness.row_title(cx, "thread-row-0").as_deref(), Some("Renamed"));
    assert_eq!(harness.header_title(cx).as_deref(), Some("Renamed"));

    choose(&harness, cx, "thread-row-0", "menu-rename");
    harness.simulate_keystrokes(cx, "backspace enter");

    assert_eq!(harness.row_title(cx, "thread-row-0").as_deref(), Some("first prompt"));
    assert_eq!(harness.header_title(cx).as_deref(), Some("first prompt"));
}

#[gpui::test]
fn escape_cancels_a_rename(cx: &mut TestAppContext) {
    let connector = FakeConnector::default().agent("/alpha", replying("alpha-session", "hello"));
    let harness = DesktopHarnessBuilder::default().connector(connector).project("/alpha").build(cx);
    prompted_thread(&harness, cx, "first prompt");

    choose(&harness, cx, "thread-row-0", "menu-rename");
    harness.simulate_input(cx, "Never mind");
    harness.simulate_keystrokes(cx, "escape");

    assert!(harness.bounds(cx, "rename-input").is_none());
    assert_eq!(harness.row_title(cx, "thread-row-0").as_deref(), Some("first prompt"));
}

#[gpui::test]
fn a_rename_is_kept_across_launches(first: &mut TestAppContext, second: &mut TestAppContext) {
    let state = tempfile::tempdir().unwrap();
    let harness = launch(first, state.path(), sessions(&["saved"]));
    choose(&harness, first, "history-row-saved", "menu-rename");
    harness.simulate_input(first, "Renamed");
    harness.simulate_keystrokes(first, "enter");
    first.run_until_parked();

    let relaunched = launch(second, state.path(), sessions(&["saved"]));

    assert_eq!(relaunched.row_title(second, "history-row-saved").as_deref(), Some("Renamed"));
}

#[gpui::test]
fn pinning_moves_a_row_to_the_top_of_its_project_and_is_kept_across_launches(
    first: &mut TestAppContext,
    second: &mut TestAppContext,
) {
    let state = tempfile::tempdir().unwrap();
    let connector = FakeConnector::default().agent("/alpha", replying("live-session", "hello"));
    let harness = DesktopHarnessBuilder::default()
        .state_dir(state.path())
        .history(sessions(&["day-1", "day-2", "day-3"]))
        .connector(connector)
        .project("/alpha")
        .build(first);
    wait_for_history(&harness, first);
    live_thread(&harness, first);
    assert_eq!(row_order(&harness, first, &["thread-row-0", "history-row-day-3", "history-row-day-1"]), [0, 1, 2]);

    choose(&harness, first, "history-row-day-1", "menu-pin");

    assert_eq!(row_order(&harness, first, &["history-row-day-1", "thread-row-0", "history-row-day-3"]), [0, 1, 2]);
    first.run_until_parked();
    let relaunched = launch(second, state.path(), sessions(&["day-1", "day-2", "day-3"]));
    assert_eq!(
        row_order(&relaunched, second, &["history-row-day-1", "history-row-day-3", "history-row-day-2"]),
        [0, 1, 2]
    );

    choose(&relaunched, second, "history-row-day-1", "menu-unpin");

    assert_eq!(
        row_order(&relaunched, second, &["history-row-day-3", "history-row-day-2", "history-row-day-1"]),
        [0, 1, 2]
    );
}

#[gpui::test]
fn pinned_rows_keep_their_order_among_themselves(cx: &mut TestAppContext) {
    let state = tempfile::tempdir().unwrap();
    let harness = launch(cx, state.path(), sessions(&["day-1", "day-2", "day-3"]));

    choose(&harness, cx, "history-row-day-1", "menu-pin");
    choose(&harness, cx, "history-row-day-2", "menu-pin");

    assert_eq!(row_order(&harness, cx, &["history-row-day-2", "history-row-day-1", "history-row-day-3"]), [0, 1, 2]);
}

#[gpui::test]
fn archiving_moves_a_row_into_its_projects_archived_group(cx: &mut TestAppContext) {
    let state = tempfile::tempdir().unwrap();
    let harness = launch(cx, state.path(), sessions(&["kept", "gone"]));
    assert!(harness.bounds(cx, "project-alpha-archived").is_none(), "nothing is archived yet");

    choose(&harness, cx, "history-row-gone", "menu-archive");

    assert!(harness.bounds(cx, "history-row-gone").is_none(), "the archived group starts collapsed");
    assert!(harness.bounds(cx, "history-row-kept").is_some());
    harness.click(cx, "project-alpha-archived").unwrap();
    let group = harness.bounds(cx, "project-alpha-archived").expect("the archived group is painted");
    let row = harness.bounds(cx, "history-row-gone").expect("expanding the group shows the row");
    assert!(harness.bounds(cx, "history-row-kept").unwrap().top() < group.top(), "the group sits at the bottom");
    assert!(group.top() < row.top());
    harness.click(cx, "project-alpha-archived").unwrap();
    assert!(harness.bounds(cx, "history-row-gone").is_none(), "collapsing the group hides the row");

    harness.click(cx, "project-alpha-archived").unwrap();
    choose(&harness, cx, "history-row-gone", "menu-unarchive");

    assert!(harness.bounds(cx, "project-alpha-archived").is_none(), "the group is empty again");
    assert_eq!(row_order(&harness, cx, &["history-row-gone", "history-row-kept"]), [0, 1]);
}

#[gpui::test]
fn archived_rows_do_not_count_toward_show_more(cx: &mut TestAppContext) {
    let state = tempfile::tempdir().unwrap();
    let harness = launch(cx, state.path(), sessions(&["day-1", "day-2", "day-3", "day-4", "day-5", "day-6"]));
    assert!(harness.bounds(cx, "history-row-day-1").is_none(), "the oldest session is behind Show more");

    choose(&harness, cx, "history-row-day-6", "menu-archive");

    assert!(harness.bounds(cx, "history-row-day-1").is_some());
    assert!(harness.bounds(cx, "project-alpha-show-more").is_none());
}

#[gpui::test]
fn an_archive_is_kept_across_launches(first: &mut TestAppContext, second: &mut TestAppContext) {
    let state = tempfile::tempdir().unwrap();
    let harness = launch(first, state.path(), sessions(&["kept", "gone"]));
    choose(&harness, first, "history-row-gone", "menu-archive");
    first.run_until_parked();

    let relaunched = launch(second, state.path(), sessions(&["kept", "gone"]));

    assert!(relaunched.bounds(second, "history-row-gone").is_none());
    assert!(relaunched.bounds(second, "project-alpha-archived").is_some());
}

#[gpui::test]
fn archiving_a_live_thread_leaves_its_agent_running(cx: &mut TestAppContext) {
    let connector = FakeConnector::default().agent("/alpha", replying("alpha-session", "hello"));
    let harness = DesktopHarnessBuilder::default().connector(connector.clone()).project("/alpha").build(cx);
    live_thread(&harness, cx);

    choose(&harness, cx, "thread-row-0", "menu-archive");
    harness.click(cx, "project-alpha-archived").unwrap();

    assert_eq!(connector.live_agents("/alpha"), 1);
    assert!(harness.bounds(cx, "thread-row-0-status-running").is_none());
    assert!(harness.bounds(cx, "thread-row-0").is_some(), "the thread is listed in the archived group");
}

#[gpui::test]
fn stop_in_the_menu_ends_the_threads_agent(cx: &mut TestAppContext) {
    let connector = FakeConnector::default().agent("/alpha", replying("alpha-session", "hello"));
    let harness = DesktopHarnessBuilder::default().connector(connector.clone()).project("/alpha").build(cx);
    live_thread(&harness, cx);

    choose(&harness, cx, "thread-row-0", "menu-stop");
    harness.run_until(cx, |_| connector.live_agents("/alpha") == 0);

    assert!(harness.threads(cx).read_with(cx, |threads, _| threads.threads().is_empty()));
}

/// Saved sessions in `/alpha`, each a day newer than the one before it.
fn sessions(ids: &[&str]) -> TestStore {
    let sessions = TestStore::new();
    for (day, id) in ids.iter().enumerate() {
        save_session(
            &sessions,
            id,
            "/alpha",
            &format!("2026-01-{:02}T00:00:00Z", day + 1),
            Some(&format!("{id} prompt")),
        );
    }
    sessions
}

/// Opens the app with `/alpha` added and `sessions` in history, keeping its state in `state`.
fn launch(cx: &mut TestAppContext, state: &Path, sessions: TestStore) -> DesktopHarness {
    let harness = DesktopHarnessBuilder::default().state_dir(state).history(sessions).project("/alpha").build(cx);
    wait_for_history(&harness, cx);
    harness
}

/// Starts a thread in `/alpha` and waits for its agent to connect.
fn live_thread(harness: &DesktopHarness, cx: &mut TestAppContext) -> Entity<Thread> {
    harness.click(cx, "project-alpha-new-thread").unwrap();
    let thread = harness.selected_thread(cx).expect("the new thread is selected");
    harness.run_until(cx, |cx| thread.read(cx).status() == ThreadStatus::Idle);
    thread
}

/// Starts a thread in `/alpha` and sends it `prompt`, waiting for the reply.
fn prompted_thread(harness: &DesktopHarness, cx: &mut TestAppContext, prompt: &str) -> Entity<Thread> {
    let thread = live_thread(harness, cx);
    thread.update(cx, |thread, cx| thread.submit(prompt, cx));
    harness.run_until(cx, |cx| texts(thread.read(cx)).len() == 2 && thread.read(cx).status() == ThreadStatus::Idle);
    thread
}

/// Where each of `rows` is painted, top to bottom, as its rank among them.
fn row_order(harness: &DesktopHarness, cx: &mut TestAppContext, rows: &[&'static str]) -> Vec<usize> {
    let tops: Vec<_> =
        rows.iter().map(|row| harness.bounds(cx, row).unwrap_or_else(|| panic!("`{row}` is painted")).top()).collect();
    tops.iter().map(|top| tops.iter().filter(|other| *other < top).count()).collect()
}
