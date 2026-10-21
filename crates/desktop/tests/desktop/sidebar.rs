use crate::support::{replying, save_session, wait_for_history};
use aether_desktop::sidebar::relative_time;
use aether_desktop::testing::{DesktopHarness, DesktopHarnessBuilder, FakeConnector};
use aether_desktop::threads::ThreadStatus;
use aether_sessions::testing::TestStore;
use chrono::{Duration, Utc};
use gpui::TestAppContext;
use std::path::{Path, PathBuf};

#[gpui::test]
fn projects_from_history_and_added_projects_list_their_saved_sessions(cx: &mut TestAppContext) {
    let sessions = TestStore::new();
    save_session(&sessions, "older", "/alpha", "2026-01-01T00:00:00Z", Some("older prompt"));
    save_session(&sessions, "newer", "/alpha", "2026-02-01T00:00:00Z", None);
    save_session(&sessions, "beta-session", "/beta", "2026-01-15T00:00:00Z", Some("beta prompt"));
    let harness = DesktopHarnessBuilder::default().history(sessions).project("/added").build(cx);
    wait_for_history(&harness, cx);
    harness.click(cx, "project-alpha-toggle").unwrap();

    for selector in ["project-added", "project-alpha", "project-beta", "history-row-older", "history-row-newer"] {
        assert!(harness.bounds(cx, selector).is_some(), "`{selector}` is painted");
    }
    let mut row_top = |selector| harness.bounds(cx, selector).expect("row is painted").top();
    assert!(row_top("project-alpha") < row_top("history-row-newer"), "rows sit under their project");
    assert!(row_top("history-row-newer") < row_top("history-row-older"), "newest first");
    assert!(row_top("history-row-older") < row_top("project-beta"), "each project's rows stay under it");
}

#[gpui::test]
fn a_project_only_history_knows_about_starts_collapsed(cx: &mut TestAppContext) {
    let sessions = TestStore::new();
    save_session(&sessions, "saved", "/alpha", "2026-01-01T00:00:00Z", Some("prompt"));
    let harness = DesktopHarnessBuilder::default().history(sessions).build(cx);
    wait_for_history(&harness, cx);
    assert!(harness.bounds(cx, "project-alpha").is_some());
    assert!(harness.bounds(cx, "history-row-saved").is_none());

    harness.click(cx, "project-alpha-toggle").unwrap();
    assert!(harness.bounds(cx, "history-row-saved").is_some());

    harness.click(cx, "project-alpha-toggle").unwrap();
    assert!(harness.bounds(cx, "history-row-saved").is_none());
}

#[gpui::test]
fn an_added_project_starts_expanded(cx: &mut TestAppContext) {
    let sessions = TestStore::new();
    save_session(&sessions, "saved", "/alpha", "2026-01-01T00:00:00Z", Some("prompt"));
    let harness = DesktopHarnessBuilder::default().history(sessions).project("/alpha").build(cx);
    wait_for_history(&harness, cx);
    assert!(harness.bounds(cx, "history-row-saved").is_some());

    harness.click(cx, "project-alpha-toggle").unwrap();
    assert!(harness.bounds(cx, "history-row-saved").is_none());

    harness.click(cx, "project-alpha-toggle").unwrap();
    assert!(harness.bounds(cx, "history-row-saved").is_some());
}

#[gpui::test]
fn a_project_lists_its_newest_five_saved_sessions_then_show_more(cx: &mut TestAppContext) {
    let harness = DesktopHarnessBuilder::default().history(eight_sessions("/alpha")).project("/alpha").build(cx);
    wait_for_history(&harness, cx);

    let painted: Vec<bool> = DAY_ROWS.iter().map(|row| harness.bounds(cx, row).is_some()).collect();
    assert_eq!(painted, [false, false, false, true, true, true, true, true]);
    let last_row = harness.bounds(cx, "history-row-day-4").expect("oldest shown row is painted");
    let show_more = harness.bounds(cx, "project-alpha-show-more").expect("show more is painted");
    assert!(last_row.top() < show_more.top(), "show more sits below the rows");
}

#[gpui::test]
fn show_more_reveals_the_remaining_saved_sessions(cx: &mut TestAppContext) {
    let harness = DesktopHarnessBuilder::default().history(eight_sessions("/alpha")).project("/alpha").build(cx);
    wait_for_history(&harness, cx);

    harness.click(cx, "project-alpha-show-more").unwrap();

    for row in DAY_ROWS {
        assert!(harness.bounds(cx, row).is_some(), "`{row}` is painted");
    }
    assert!(harness.bounds(cx, "project-alpha-show-more").is_none(), "nothing is left to show");
}

#[gpui::test]
fn a_projects_plus_starts_a_selected_thread_in_it(cx: &mut TestAppContext) {
    let connector = FakeConnector::default().agent("/alpha", replying("alpha-session", "hello"));
    let harness = DesktopHarnessBuilder::default().connector(connector).project("/alpha").build(cx);

    harness.click(cx, "project-alpha-new-thread").unwrap();

    let thread = harness.selected_thread(cx).expect("the new thread is selected");
    harness.run_until(cx, |cx| thread.read(cx).status() == ThreadStatus::Idle);
    assert_eq!(thread.read_with(cx, |thread, _| thread.target().cwd().to_path_buf()), PathBuf::from("/alpha"));
    let project = harness.bounds(cx, "project-alpha").expect("project header is painted");
    let row = harness.bounds(cx, "thread-row-0").expect("thread row is painted");
    assert!(project.top() < row.top());
}

#[gpui::test]
fn a_live_thread_sits_above_its_projects_saved_sessions_and_replaces_its_own(cx: &mut TestAppContext) {
    let sessions = TestStore::new();
    save_session(&sessions, "saved", "/alpha", "2026-01-01T00:00:00Z", Some("prompt"));
    save_session(&sessions, "alpha-session", "/alpha", "2026-01-02T00:00:00Z", Some("live prompt"));
    let connector = FakeConnector::default().agent("/alpha", replying("alpha-session", "hello"));
    let harness = DesktopHarnessBuilder::default().connector(connector).history(sessions).build(cx);
    wait_for_history(&harness, cx);

    harness.click(cx, "project-alpha-new-thread").unwrap();
    let thread = harness.selected_thread(cx).expect("a thread is selected");
    harness.run_until(cx, |cx| thread.read(cx).status() == ThreadStatus::Idle);

    let live = harness.bounds(cx, "thread-row-0").expect("live row is painted");
    let saved = harness.bounds(cx, "history-row-saved").expect("saved row is painted");
    assert!(live.top() < saved.top());
    assert!(harness.bounds(cx, "history-row-alpha-session").is_none(), "the live thread stands in for its session");
}

#[gpui::test]
fn clicking_a_live_row_selects_its_thread(cx: &mut TestAppContext) {
    let connector = FakeConnector::default()
        .agent("/alpha", replying("alpha-session", "hello"))
        .agent("/beta", replying("beta-session", "hello"));
    let harness = DesktopHarnessBuilder::default().connector(connector).project("/alpha").project("/beta").build(cx);
    harness.click(cx, "project-alpha-new-thread").unwrap();
    harness.click(cx, "project-beta-new-thread").unwrap();
    assert_eq!(selected_cwd(&harness, cx), Path::new("/beta"));

    harness.click(cx, "thread-row-0").unwrap();

    assert_eq!(selected_cwd(&harness, cx), Path::new("/alpha"));
}

#[gpui::test]
fn the_new_thread_shortcut_starts_a_thread_in_the_selected_threads_project(cx: &mut TestAppContext) {
    let connector = FakeConnector::default()
        .agent("/alpha", replying("alpha-session", "hello"))
        .agent("/beta", replying("beta-session", "hello"))
        .agent("/beta", replying("second-beta-session", "hello"));
    let harness = DesktopHarnessBuilder::default().connector(connector).project("/alpha").project("/beta").build(cx);
    harness.click(cx, "project-beta-new-thread").unwrap();

    harness.simulate_keystrokes(cx, "secondary-n");

    let threads = harness.threads(cx);
    let cwds: Vec<PathBuf> = threads
        .read_with(cx, |threads, cx| threads.threads().iter().map(|t| t.read(cx).target().cwd().into()).collect());
    assert_eq!(cwds, [PathBuf::from("/beta"), PathBuf::from("/beta")]);
}

#[gpui::test]
fn the_new_thread_button_uses_the_first_project_when_no_thread_is_selected(cx: &mut TestAppContext) {
    let connector = FakeConnector::default().agent("/alpha", replying("alpha-session", "hello"));
    let harness = DesktopHarnessBuilder::default().connector(connector).project("/alpha").project("/beta").build(cx);

    harness.click(cx, "new-thread").unwrap();

    assert_eq!(selected_cwd(&harness, cx), Path::new("/alpha"));
}

#[gpui::test]
fn adding_a_project_lists_the_picked_folder(cx: &mut TestAppContext) {
    let harness = DesktopHarnessBuilder::default().build(cx);
    assert!(harness.bounds(cx, "project-picked").is_none());

    harness.click(cx, "add-project").unwrap();
    cx.simulate_path_prompt_response(|options| {
        assert!(options.directories && !options.files);
        Some(vec![PathBuf::from("/picked")])
    });
    cx.run_until_parked();

    assert!(harness.bounds(cx, "project-picked").is_some());
}

#[test]
fn relative_times_are_compact() {
    let now = Utc::now();
    let ago = |duration| relative_time(now - duration, now);

    assert_eq!(ago(Duration::seconds(30)), "now");
    assert_eq!(ago(Duration::minutes(5)), "5m");
    assert_eq!(ago(Duration::hours(3)), "3h");
    assert_eq!(ago(Duration::days(2)), "2d");
    assert_eq!(ago(Duration::days(15)), "2w");
    assert_eq!(ago(Duration::days(800)), "2y");
}

/// The rows of `eight_sessions`, oldest first.
const DAY_ROWS: [&str; 8] = [
    "history-row-day-1",
    "history-row-day-2",
    "history-row-day-3",
    "history-row-day-4",
    "history-row-day-5",
    "history-row-day-6",
    "history-row-day-7",
    "history-row-day-8",
];

/// Eight sessions in `cwd`, `day-1` through `day-8`, each a day newer than the last.
fn eight_sessions(cwd: &str) -> TestStore {
    let sessions = TestStore::new();
    for day in 1..=8 {
        save_session(&sessions, &format!("day-{day}"), cwd, &format!("2026-01-0{day}T00:00:00Z"), Some("prompt"));
    }
    sessions
}

fn selected_cwd(harness: &DesktopHarness, cx: &TestAppContext) -> PathBuf {
    let thread = harness.selected_thread(cx).expect("a thread is selected");
    thread.read_with(cx, |thread, _| thread.target().cwd().to_path_buf())
}
