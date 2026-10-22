use crate::support::{assert_close, replying, save_session, start_thread, wait_for_history};
use acp_utils::testing::FakeAgent;
use aether_desktop::sidebar::relative_time;
use aether_desktop::testing::{DesktopHarness, DesktopHarnessBuilder, FakeConnector};
use aether_desktop::threads::ThreadStatus;
use aether_sessions::testing::TestStore;
use agent_client_protocol::schema::v2::SessionId;
use chrono::{Duration, Utc};
use gpui::{AppContext, TestAppContext, px};
use std::path::{Path, PathBuf};

#[gpui::test]
fn a_busy_spinner_does_not_request_every_display_frame(cx: &mut TestAppContext) {
    let harness = DesktopHarnessBuilder::default().project("/alpha").build(cx);
    harness.click(cx, "project-alpha-new-thread").unwrap();
    assert!(harness.bounds(cx, "thread-row-0-status-connecting-spinner").is_some());
    assert!(harness.bounds(cx, "thread-row-0-conversation-icon").is_none());
    let window = cx.windows()[0];
    cx.update_window(window, |_, window, cx| window.simulate_next_frame(cx)).unwrap();
    cx.run_until_parked();

    for _ in 0..3 {
        let callbacks = cx.update_window(window, |_, window, cx| window.simulate_next_frame(cx)).unwrap();
        assert_eq!(callbacks, 0, "the spinner should be timer-driven, not display-frame-driven");
        cx.executor().advance_clock(std::time::Duration::from_millis(105));
        cx.run_until_parked();
    }
}

#[gpui::test]
fn added_projects_list_their_saved_sessions_and_projects_only_history_knows_about_are_hidden(cx: &mut TestAppContext) {
    let sessions = TestStore::new();
    save_session(&sessions, "older", "/alpha", "2026-01-01T00:00:00Z", Some("older prompt"));
    save_session(&sessions, "newer", "/alpha", "2026-01-03T00:00:00Z", None);
    save_session(&sessions, "beta-session", "/beta", "2026-01-02T00:00:00Z", Some("beta prompt"));
    save_session(&sessions, "gamma-session", "/gamma", "2026-01-02T00:00:00Z", Some("gamma prompt"));
    let harness = DesktopHarnessBuilder::default().history(sessions).project("/alpha").project("/beta").build(cx);
    wait_for_history(&harness, cx);

    for selector in
        ["project-alpha", "project-beta", "history-row-older", "history-row-newer", "history-row-beta-session"]
    {
        assert!(harness.bounds(cx, selector).is_some(), "`{selector}` is painted");
    }
    assert!(harness.bounds(cx, "project-gamma").is_none(), "a project only history knows about is not listed");
    let mut row_top = |selector| harness.bounds(cx, selector).expect("row is painted").top();
    assert!(row_top("project-alpha") < row_top("history-row-newer"), "rows sit under their project");
    assert!(row_top("history-row-newer") < row_top("history-row-older"), "newest first");
    assert!(row_top("history-row-older") < row_top("project-beta"), "each project's rows stay under it");
}

#[gpui::test]
fn saved_sessions_older_than_a_week_are_hidden_unless_pinned(cx: &mut TestAppContext) {
    let sessions = TestStore::new();
    save_session(&sessions, "recent", "/alpha", "2026-03-05T00:00:00Z", Some("recent prompt"));
    save_session(&sessions, "stale", "/alpha", "2026-02-20T00:00:00Z", Some("stale prompt"));
    save_session(&sessions, "stale-pinned", "/alpha", "2026-02-19T00:00:00Z", Some("pinned prompt"));
    let harness =
        DesktopHarnessBuilder::default().history(sessions).project("/alpha").now("2026-03-07T00:00:00Z").build(cx);
    wait_for_history(&harness, cx);

    assert!(harness.bounds(cx, "history-row-recent").is_some());
    assert!(harness.bounds(cx, "history-row-stale").is_none(), "a session older than a week is not listed");
    assert!(harness.bounds(cx, "project-alpha-show-more").is_none(), "nor is it behind Show more");

    harness.registry(cx).update(cx, |registry, cx| registry.pin(&SessionId::new("stale-pinned"), cx));

    assert!(harness.bounds(cx, "history-row-stale-pinned").is_some(), "a pinned session is listed however old");
}

#[gpui::test]
fn opening_a_thread_in_a_project_adds_the_project_to_the_sidebar(cx: &mut TestAppContext) {
    let sessions = TestStore::new();
    save_session(&sessions, "beta-session", "/beta", "2026-01-02T00:00:00Z", Some("Refactor auth"));
    let connector = FakeConnector::default().agent("/beta", FakeAgent::default().replay_message("beta-session", "hi"));
    let harness = DesktopHarnessBuilder::default().connector(connector).history(sessions).build(cx);
    wait_for_history(&harness, cx);
    assert!(harness.bounds(cx, "project-beta").is_none());

    harness.simulate_keystrokes(cx, "secondary-k");
    harness.simulate_input(cx, "refactor");
    harness.simulate_keystrokes(cx, "enter");

    assert!(harness.bounds(cx, "project-beta").is_some());
    let registry = harness.registry(cx);
    assert_eq!(registry.read_with(cx, |registry, _| registry.projects().to_vec()), [PathBuf::from("/beta")]);
}

#[gpui::test]
fn removing_a_project_hides_it_and_keeps_its_sessions_in_the_switcher(cx: &mut TestAppContext) {
    let sessions = TestStore::new();
    save_session(&sessions, "saved", "/alpha", "2026-01-02T00:00:00Z", Some("prompt"));
    let harness = DesktopHarnessBuilder::default().history(sessions).project("/alpha").project("/beta").build(cx);
    wait_for_history(&harness, cx);

    harness.click(cx, "project-alpha-actions").unwrap();
    harness.click(cx, "menu-remove-project").unwrap();

    assert!(harness.bounds(cx, "project-alpha").is_none());
    assert!(harness.bounds(cx, "project-beta").is_some());
    let registry = harness.registry(cx);
    assert_eq!(registry.read_with(cx, |registry, _| registry.projects().to_vec()), [PathBuf::from("/beta")]);
    harness.simulate_keystrokes(cx, "secondary-k");
    assert!(harness.bounds(cx, "switcher-session-saved").is_some(), "the session can still be found");
}

#[gpui::test]
fn archiving_inactive_threads_archives_every_saved_thread_but_pinned_and_live_ones(cx: &mut TestAppContext) {
    let sessions = TestStore::new();
    save_session(&sessions, "first", "/alpha", "2026-01-01T00:00:00Z", Some("first prompt"));
    save_session(&sessions, "second", "/alpha", "2026-01-02T00:00:00Z", Some("second prompt"));
    save_session(&sessions, "kept", "/alpha", "2026-01-03T00:00:00Z", Some("kept prompt"));
    let connector = FakeConnector::default().agent("/alpha", replying("live-session", "hello"));
    let harness = DesktopHarnessBuilder::default().connector(connector).history(sessions).project("/alpha").build(cx);
    wait_for_history(&harness, cx);
    let thread = start_thread(&harness, cx, "project-alpha-new-thread");
    harness.registry(cx).update(cx, |registry, cx| registry.pin(&SessionId::new("kept"), cx));

    harness.click(cx, "project-alpha-actions").unwrap();
    harness.click(cx, "menu-archive-inactive").unwrap();

    for row in ["history-row-first", "history-row-second"] {
        assert!(harness.bounds(cx, row).is_none(), "`{row}` is archived");
    }
    assert!(harness.bounds(cx, "history-row-kept").is_some(), "a pinned thread stays");
    assert!(harness.bounds(cx, "thread-row-0").is_some(), "a live thread stays");
    assert_eq!(thread.read_with(cx, |thread, _| thread.status()), ThreadStatus::Idle);
    harness.click(cx, "project-alpha-archived").unwrap();
    assert!(harness.bounds(cx, "history-row-first").is_some());
}

#[gpui::test]
fn a_rows_archive_button_archives_it_and_its_unarchive_button_brings_it_back(cx: &mut TestAppContext) {
    let sessions = TestStore::new();
    save_session(&sessions, "saved", "/alpha", "2026-01-02T00:00:00Z", Some("prompt"));
    let harness = DesktopHarnessBuilder::default().history(sessions).project("/alpha").build(cx);
    wait_for_history(&harness, cx);

    harness.click(cx, "history-row-saved-archive").unwrap();

    assert!(harness.bounds(cx, "history-row-saved").is_none());
    assert!(harness.bounds(cx, "project-alpha-archived-icon").is_some());
    harness.click(cx, "project-alpha-archived").unwrap();
    assert!(harness.bounds(cx, "project-alpha-archived-icon").is_some());
    assert!(harness.bounds(cx, "history-row-saved-conversation-icon").is_some());
    harness.click(cx, "history-row-saved-unarchive").unwrap();
    assert!(harness.bounds(cx, "project-alpha-archived").is_none(), "nothing is archived any more");
    assert!(harness.bounds(cx, "history-row-saved").is_some());
}

#[gpui::test]
fn add_project_offers_the_recent_projects_history_knows_about(cx: &mut TestAppContext) {
    let sessions = TestStore::new();
    save_session(&sessions, "alpha-session", "/alpha", "2026-01-02T00:00:00Z", Some("prompt"));
    save_session(&sessions, "beta-session", "/beta", "2026-01-02T00:00:00Z", Some("prompt"));
    let harness = DesktopHarnessBuilder::default().history(sessions).project("/alpha").build(cx);
    wait_for_history(&harness, cx);

    harness.click(cx, "add-project").unwrap();
    assert!(harness.bounds(cx, "add-project-recent-alpha").is_none(), "a listed project is not offered");
    harness.click(cx, "add-project-recent-beta").unwrap();

    assert!(harness.bounds(cx, "project-beta").is_some());
    assert!(harness.bounds(cx, "history-row-beta-session").is_some());
}

#[gpui::test]
fn an_added_project_starts_expanded(cx: &mut TestAppContext) {
    let sessions = TestStore::new();
    save_session(&sessions, "saved", "/alpha", "2026-01-01T00:00:00Z", Some("prompt"));
    let harness = DesktopHarnessBuilder::default().history(sessions).project("/alpha").build(cx);
    wait_for_history(&harness, cx);
    assert!(harness.bounds(cx, "history-row-saved").is_some());
    assert!(harness.bounds(cx, "project-alpha-folder-open").is_some());
    assert!(harness.bounds(cx, "project-alpha-folder-closed").is_none());

    harness.click(cx, "project-alpha-toggle").unwrap();
    assert!(harness.bounds(cx, "history-row-saved").is_none());
    assert!(harness.bounds(cx, "project-alpha-folder-closed").is_some());
    assert!(harness.bounds(cx, "project-alpha-folder-open").is_none());

    harness.click(cx, "project-alpha-toggle").unwrap();
    assert!(harness.bounds(cx, "history-row-saved").is_some());
    assert!(harness.bounds(cx, "project-alpha-folder-open").is_some());
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
    let harness = DesktopHarnessBuilder::default().connector(connector).history(sessions).project("/alpha").build(cx);
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
    harness.click(cx, "add-project-browse").unwrap();
    cx.simulate_path_prompt_response(|options| {
        assert!(options.directories && !options.files);
        Some(vec![PathBuf::from("/picked")])
    });
    cx.run_until_parked();

    assert!(harness.bounds(cx, "project-picked").is_some());
}

#[gpui::test]
fn sidebar_rows_nest_under_their_project_and_stay_aligned_when_the_interface_size_changes(cx: &mut TestAppContext) {
    let harness = DesktopHarnessBuilder::default().history(eight_sessions("/alpha")).project("/alpha").build(cx);
    wait_for_history(&harness, cx);
    harness.registry(cx).update(cx, |registry, cx| registry.archive(&SessionId::new("day-1"), cx));
    harness.click(cx, "project-alpha-archived").unwrap();

    for shortcut in ["secondary-0", "secondary-=", "secondary-- secondary--"] {
        harness.simulate_keystrokes(cx, shortcut);
        let folder = harness.bounds(cx, "project-alpha-folder-open").expect("project has a folder icon");
        let conversation = harness.bounds(cx, "history-row-day-8-conversation-icon").expect("thread has an icon");
        let archive = harness.bounds(cx, "project-alpha-archived-icon").expect("archive group has an icon");
        let archived = harness.bounds(cx, "history-row-day-1-conversation-icon").expect("archived thread has an icon");
        let indent = px(12.0) * (harness.rem_size(cx) / px(16.0));
        assert_close(conversation.left() - folder.left(), indent);
        assert_eq!(archive.left(), conversation.left(), "the archived group nests one level, like threads");
        assert_close(archived.left() - archive.left(), indent);
        assert_eq!(conversation.size.width, harness.rem_size(cx));
        assert_eq!(conversation.size.height, harness.rem_size(cx));

        let title = harness.bounds(cx, "history-row-day-8-title").unwrap();
        for selector in ["project-alpha-archived-title", "project-alpha-show-more-title"] {
            assert_eq!(harness.bounds(cx, selector).unwrap().left(), title.left(), "{selector} aligns with threads");
        }
        let project_title = harness.bounds(cx, "project-alpha-title").unwrap();
        assert_close(title.left() - project_title.left(), indent);
        let row = harness.bounds(cx, "history-row-day-8").unwrap();
        let expected_height = px(28.0) * (harness.rem_size(cx) / px(16.0));
        assert!(
            (row.size.height - expected_height).abs() <= px(1.0),
            "row height stays unchanged after pixel snapping"
        );
    }
}

#[gpui::test]
fn sidebar_icons_leave_room_for_long_titles_at_minimum_width(cx: &mut TestAppContext) {
    let sessions = TestStore::new();
    save_session(
        &sessions,
        "saved",
        "/a-project-with-a-long-name",
        "2026-01-01T00:00:00Z",
        Some("A very long thread title that should truncate instead of pushing its icon or actions out of the sidebar"),
    );
    let harness = DesktopHarnessBuilder::default().history(sessions).project("/a-project-with-a-long-name").build(cx);
    wait_for_history(&harness, cx);
    harness.drag(cx, "sidebar-resize-handle", px(10.0)).unwrap();

    let row = harness.bounds(cx, "history-row-saved").unwrap();
    let icon = harness.bounds(cx, "history-row-saved-conversation-icon").unwrap();
    let title = harness.bounds(cx, "history-row-saved-title").unwrap();
    assert!(icon.left() >= row.left());
    assert!(icon.right() < title.left());
    assert!(title.size.width > px(0.0));
    assert!(title.right() <= row.right());
    assert_eq!(icon.size.width, harness.rem_size(cx));

    let project = harness.bounds(cx, "project-a-project-with-a-long-name").unwrap();
    let title = harness.bounds(cx, "project-a-project-with-a-long-name-title").unwrap();
    let new_thread = harness.bounds(cx, "project-a-project-with-a-long-name-new-thread").unwrap();
    assert!(title.size.width > px(0.0));
    assert!(title.right() <= new_thread.left());
    assert!(new_thread.right() <= project.right());

    harness.click(cx, "project-a-project-with-a-long-name-folder-open").unwrap();
    assert!(harness.bounds(cx, "history-row-saved").is_none());
    harness.click(cx, "project-a-project-with-a-long-name-folder-closed").unwrap();
    assert!(harness.bounds(cx, "history-row-saved-conversation-icon").is_some());
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
