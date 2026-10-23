use crate::support::replying;
use acp_utils::testing::FakeAgent;
use aether_desktop::registry::Registry;
use aether_desktop::sidebar::relative_time;
use aether_desktop::testing::{DesktopHarness, DesktopHarnessBuilder, FakeConnector};
use aether_desktop::threads::ThreadStatus;
use agent_client_protocol::schema::v2::SessionId;
use chrono::{Duration, Utc};
use gpui::TestAppContext;
use std::path::{Path, PathBuf};

#[gpui::test]
fn added_projects_list_their_saved_sessions_and_projects_only_history_knows_about_are_hidden(cx: &mut TestAppContext) {
    let harness = DesktopHarnessBuilder::default()
        .session("older", "/alpha", "2026-01-01T00:00:00Z", "older prompt")
        .session("newer", "/alpha", "2026-01-03T00:00:00Z", None)
        .session("beta-session", "/beta", "2026-01-02T00:00:00Z", "beta prompt")
        .session("gamma-session", "/gamma", "2026-01-02T00:00:00Z", "gamma prompt")
        .project("/alpha")
        .project("/beta")
        .build(cx);

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
    let harness = DesktopHarnessBuilder::default()
        .session("recent", "/alpha", "2026-03-05T00:00:00Z", "recent prompt")
        .session("stale", "/alpha", "2026-02-20T00:00:00Z", "stale prompt")
        .session("stale-pinned", "/alpha", "2026-02-19T00:00:00Z", "pinned prompt")
        .project("/alpha")
        .now("2026-03-07T00:00:00Z")
        .build(cx);

    assert!(harness.bounds(cx, "history-row-recent").is_some());
    assert!(harness.bounds(cx, "history-row-stale").is_none(), "a session older than a week is not listed");
    assert!(harness.bounds(cx, "project-alpha-show-more").is_none(), "nor is it behind Show more");

    Registry::update(&harness.registry(cx), cx, |state| {
        state.pinned.insert(SessionId::new("stale-pinned"));
    });

    assert!(harness.bounds(cx, "history-row-stale-pinned").is_some(), "a pinned session is listed however old");
}

#[gpui::test]
fn opening_a_thread_in_a_project_adds_the_project_to_the_sidebar(cx: &mut TestAppContext) {
    let connector = FakeConnector::default().agent("/beta", FakeAgent::default().replay_message("beta-session", "hi"));
    let harness = DesktopHarnessBuilder::default()
        .connector(connector)
        .session("beta-session", "/beta", "2026-01-02T00:00:00Z", "Refactor auth")
        .build(cx);
    assert!(harness.bounds(cx, "project-beta").is_none());

    harness.simulate_keystrokes(cx, "secondary-k");
    harness.simulate_input(cx, "refactor");
    harness.simulate_keystrokes(cx, "enter");

    assert!(harness.bounds(cx, "project-beta").is_some());
    assert_eq!(projects(&harness, cx), [PathBuf::from("/beta")]);
}

#[gpui::test]
fn removing_a_project_hides_it_and_keeps_its_sessions_in_the_switcher(cx: &mut TestAppContext) {
    let harness = DesktopHarnessBuilder::default()
        .session("saved", "/alpha", "2026-01-02T00:00:00Z", "prompt")
        .project("/alpha")
        .project("/beta")
        .build(cx);

    harness.click(cx, "project-alpha-actions").unwrap();
    harness.click(cx, "menu-remove-project").unwrap();

    assert!(harness.bounds(cx, "project-alpha").is_none());
    assert!(harness.bounds(cx, "project-beta").is_some());
    assert_eq!(projects(&harness, cx), [PathBuf::from("/beta")]);
    harness.simulate_keystrokes(cx, "secondary-k");
    assert!(harness.bounds(cx, "switcher-session-saved").is_some(), "the session can still be found");
}

#[gpui::test]
fn archiving_inactive_threads_archives_every_saved_thread_but_pinned_and_live_ones(cx: &mut TestAppContext) {
    let harness = DesktopHarnessBuilder::default()
        .session("first", "/alpha", "2026-01-01T00:00:00Z", "first prompt")
        .session("second", "/alpha", "2026-01-02T00:00:00Z", "second prompt")
        .session("kept", "/alpha", "2026-01-03T00:00:00Z", "kept prompt")
        .agent("/alpha", replying("live-session", "hello"))
        .build(cx);
    let thread = harness.start_thread(cx, "/alpha");
    Registry::update(&harness.registry(cx), cx, |state| {
        state.pinned.insert(SessionId::new("kept"));
    });

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
    let harness = DesktopHarnessBuilder::default()
        .session("saved", "/alpha", "2026-01-02T00:00:00Z", "prompt")
        .project("/alpha")
        .build(cx);

    harness.click(cx, "history-row-saved-archive").unwrap();

    assert!(harness.bounds(cx, "history-row-saved").is_none());
    assert!(harness.bounds(cx, "project-alpha-archived-icon").is_some());
    harness.click(cx, "project-alpha-archived").unwrap();
    assert!(harness.bounds(cx, "history-row-saved-conversation-icon").is_some());
    harness.click(cx, "history-row-saved-unarchive").unwrap();
    assert!(harness.bounds(cx, "project-alpha-archived").is_none(), "nothing is archived any more");
    assert!(harness.bounds(cx, "history-row-saved").is_some());
}

#[gpui::test]
fn add_project_offers_the_recent_projects_history_knows_about_or_a_picked_folder(cx: &mut TestAppContext) {
    let harness = DesktopHarnessBuilder::default()
        .session("alpha-session", "/alpha", "2026-01-02T00:00:00Z", "prompt")
        .session("beta-session", "/beta", "2026-01-02T00:00:00Z", "prompt")
        .project("/alpha")
        .build(cx);

    harness.click(cx, "add-project").unwrap();
    assert!(harness.bounds(cx, "add-project-recent-alpha").is_none(), "a listed project is not offered");
    harness.click(cx, "add-project-recent-beta").unwrap();
    assert!(harness.bounds(cx, "project-beta").is_some());
    assert!(harness.bounds(cx, "history-row-beta-session").is_some());

    harness.click(cx, "add-project").unwrap();
    harness.click(cx, "add-project-browse").unwrap();
    cx.simulate_path_prompt_response(|options| {
        assert!(options.directories && !options.files);
        Some(vec![PathBuf::from("/picked")])
    });
    assert!(harness.bounds(cx, "project-picked").is_some());
}

#[gpui::test]
fn a_project_folds_its_rows_away_and_back(cx: &mut TestAppContext) {
    let harness = DesktopHarnessBuilder::default()
        .session("saved", "/alpha", "2026-01-01T00:00:00Z", "prompt")
        .project("/alpha")
        .build(cx);
    assert!(harness.bounds(cx, "history-row-saved").is_some(), "an added project starts expanded");
    assert!(harness.bounds(cx, "project-alpha-folder-open").is_some());

    harness.click(cx, "project-alpha").unwrap();
    assert!(harness.bounds(cx, "history-row-saved").is_none());
    assert!(harness.bounds(cx, "project-alpha-folder-closed").is_some());

    harness.click(cx, "project-alpha").unwrap();
    assert!(harness.bounds(cx, "history-row-saved").is_some());
    assert!(harness.bounds(cx, "project-alpha-folder-open").is_some());
}

#[gpui::test]
fn a_project_lists_its_newest_five_saved_sessions_then_show_more_reveals_the_rest(cx: &mut TestAppContext) {
    let harness = (1..=8)
        .fold(DesktopHarnessBuilder::default().project("/alpha"), |builder, day| {
            builder.session(&format!("day-{day}"), "/alpha", &format!("2026-01-0{day}T00:00:00Z"), "prompt")
        })
        .build(cx);

    let painted: Vec<bool> = DAY_ROWS.iter().map(|row| harness.bounds(cx, row).is_some()).collect();
    assert_eq!(painted, [false, false, false, true, true, true, true, true]);
    let last_row = harness.bounds(cx, "history-row-day-4").expect("oldest shown row is painted");
    let show_more = harness.bounds(cx, "project-alpha-show-more").expect("show more is painted");
    assert!(last_row.top() < show_more.top(), "show more sits below the rows");

    harness.click(cx, "project-alpha-show-more").unwrap();

    assert!(DAY_ROWS.iter().all(|row| harness.bounds(cx, row).is_some()), "every row is painted");
    assert!(harness.bounds(cx, "project-alpha-show-more").is_none(), "nothing is left to show");
}

#[gpui::test]
fn a_projects_plus_starts_a_selected_thread_above_its_saved_sessions(cx: &mut TestAppContext) {
    let harness = DesktopHarnessBuilder::default()
        .session("saved", "/alpha", "2026-01-01T00:00:00Z", "prompt")
        .session("alpha-session", "/alpha", "2026-01-02T00:00:00Z", "live prompt")
        .agent("/alpha", replying("alpha-session", "hello"))
        .build(cx);

    harness.click(cx, "project-alpha-new-thread").unwrap();

    let thread = harness.selected_thread(cx).expect("the new thread is selected");
    harness.run_until(cx, |cx| thread.read(cx).status() == ThreadStatus::Idle);
    assert_eq!(thread.read_with(cx, |thread, _| thread.target().cwd().to_path_buf()), PathBuf::from("/alpha"));
    let project = harness.bounds(cx, "project-alpha").expect("project header is painted");
    let live = harness.bounds(cx, "thread-row-0").expect("live row is painted");
    let saved = harness.bounds(cx, "history-row-saved").expect("saved row is painted");
    assert!(project.top() < live.top() && live.top() < saved.top());
    assert!(harness.bounds(cx, "history-row-alpha-session").is_none(), "the live thread stands in for its session");
}

#[gpui::test]
fn clicking_a_live_row_selects_its_thread(cx: &mut TestAppContext) {
    let harness = alpha_and_beta(cx);
    harness.click(cx, "project-alpha-new-thread").unwrap();
    harness.click(cx, "project-beta-new-thread").unwrap();
    assert_eq!(selected_cwd(&harness, cx), Path::new("/beta"));

    harness.click(cx, "thread-row-0").unwrap();

    assert_eq!(selected_cwd(&harness, cx), Path::new("/alpha"));
}

#[gpui::test]
fn the_new_thread_shortcut_starts_a_thread_in_the_selected_threads_project(cx: &mut TestAppContext) {
    let harness = DesktopHarnessBuilder::default()
        .agent("/alpha", replying("alpha-session", "hello"))
        .agent("/beta", replying("beta-session", "hello"))
        .agent("/beta", replying("second-beta-session", "hello"))
        .build(cx);
    harness.click(cx, "project-beta-new-thread").unwrap();

    harness.simulate_keystrokes(cx, "secondary-n");

    let threads = harness.threads(cx);
    let cwds: Vec<PathBuf> = threads
        .read_with(cx, |threads, cx| threads.threads().iter().map(|t| t.read(cx).target().cwd().into()).collect());
    assert_eq!(cwds, [PathBuf::from("/beta"), PathBuf::from("/beta")]);
}

#[gpui::test]
fn the_new_thread_button_uses_the_first_project_when_no_thread_is_selected(cx: &mut TestAppContext) {
    let harness = alpha_and_beta(cx);

    harness.click(cx, "new-thread").unwrap();

    assert_eq!(selected_cwd(&harness, cx), Path::new("/alpha"));
}

/// The rows of the `day-1` to `day-8` sessions, oldest first.
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

fn alpha_and_beta(cx: &mut TestAppContext) -> DesktopHarness {
    DesktopHarnessBuilder::default()
        .agent("/alpha", replying("alpha-session", "hello"))
        .agent("/beta", replying("beta-session", "hello"))
        .build(cx)
}

fn projects(harness: &DesktopHarness, cx: &TestAppContext) -> Vec<PathBuf> {
    harness.registry(cx).read_with(cx, |registry, _| registry.state().projects.clone())
}

fn selected_cwd(harness: &DesktopHarness, cx: &TestAppContext) -> PathBuf {
    let thread = harness.selected_thread(cx).expect("a thread is selected");
    thread.read_with(cx, |thread, _| thread.target().cwd().to_path_buf())
}
