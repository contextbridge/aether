use crate::support::Edges;
use crate::support::replying;
use acp_utils::testing::FakeAgent;
use aether_desktop::testing::{DesktopHarness, DesktopHarnessBuilder};
use aether_desktop::threads::ThreadStatus;
use std::path::{Path, PathBuf};

#[test]
fn added_projects_list_their_saved_sessions_and_projects_only_history_knows_about_are_hidden() {
    let mut harness = DesktopHarnessBuilder::default()
        .session("older", "/alpha", "2026-01-01T00:00:00Z", "older prompt")
        .session("newer", "/alpha", "2026-01-03T00:00:00Z", None)
        .session("beta-session", "/beta", "2026-01-02T00:00:00Z", "beta prompt")
        .session("gamma-session", "/gamma", "2026-01-02T00:00:00Z", "gamma prompt")
        .project("/alpha")
        .project("/beta")
        .build();

    for selector in
        ["project-alpha", "project-beta", "history-row-older", "history-row-newer", "history-row-beta-session"]
    {
        assert!(harness.bounds(selector).is_some(), "`{selector}` is painted");
    }
    assert!(harness.bounds("project-gamma").is_none(), "a project only history knows about is not listed");
    let mut row_top = |selector| harness.bounds(selector).expect("row is painted").top();
    assert!(row_top("project-alpha") < row_top("history-row-newer"), "rows sit under their project");
    assert!(row_top("history-row-newer") < row_top("history-row-older"), "newest first");
    assert!(row_top("history-row-older") < row_top("project-beta"), "each project's rows stay under it");
}

#[test]
fn saved_sessions_older_than_a_week_are_hidden() {
    let mut harness = DesktopHarnessBuilder::default()
        .session("recent", "/alpha", "2026-03-05T00:00:00Z", "recent prompt")
        .session("stale", "/alpha", "2026-02-20T00:00:00Z", "stale prompt")
        .project("/alpha")
        .now("2026-03-07T00:00:00Z")
        .build();

    assert!(harness.bounds("history-row-recent").is_some());
    assert!(harness.bounds("history-row-stale").is_none(), "a session older than a week is not listed");
    assert!(harness.bounds("project-alpha-show-more").is_none(), "nor is it behind Show more");
}

#[test]
fn opening_a_thread_in_a_project_adds_the_project_to_the_sidebar() {
    let mut harness = DesktopHarnessBuilder::default()
        .script("/beta", FakeAgent::default().replay_message("beta-session", "hi"))
        .session("beta-session", "/beta", "2026-01-02T00:00:00Z", "Refactor auth")
        .build();
    assert!(harness.bounds("project-beta").is_none());

    harness.press("secondary-k");
    harness.type_text("refactor");
    harness.press("enter");

    assert!(harness.bounds("project-beta").is_some());
    assert_eq!(harness.projects(), [PathBuf::from("/beta")]);
}

#[test]
fn removing_a_project_hides_it_and_keeps_its_sessions_in_the_switcher() {
    let mut harness = DesktopHarnessBuilder::default()
        .session("saved", "/alpha", "2026-01-02T00:00:00Z", "prompt")
        .project("/alpha")
        .project("/beta")
        .build();

    harness.click("project-alpha-actions").unwrap();
    harness.click("menu-remove-project").unwrap();

    assert!(harness.bounds("project-alpha").is_none());
    assert!(harness.bounds("project-beta").is_some());
    assert_eq!(harness.projects(), [PathBuf::from("/beta")]);
    harness.press("secondary-k");
    assert!(harness.bounds("switcher-history-row-saved").is_some(), "the session can still be found");
}

#[test]
fn archiving_inactive_threads_archives_every_saved_thread_but_live_ones() {
    let (mut harness, thread) = DesktopHarnessBuilder::default()
        .session("first", "/alpha", "2026-01-01T00:00:00Z", "first prompt")
        .session("second", "/alpha", "2026-01-02T00:00:00Z", "second prompt")
        .agent("/alpha", replying("live-session", "hello"))
        .start("/alpha");

    harness.click("project-alpha-actions").unwrap();
    harness.click("menu-archive-inactive").unwrap();

    for row in ["history-row-first", "history-row-second"] {
        assert!(harness.bounds(row).is_none(), "`{row}` is archived");
    }
    assert!(harness.bounds("thread-row-0").is_some(), "a live thread stays");
    assert_eq!(thread.status(), ThreadStatus::Idle);
}

#[test]
fn a_rows_archive_button_removes_it_from_the_sidebar() {
    let mut harness = DesktopHarnessBuilder::default()
        .session("saved", "/alpha", "2026-01-02T00:00:00Z", "prompt")
        .project("/alpha")
        .build();

    harness.click("history-row-saved-archive").unwrap();

    assert!(harness.bounds("history-row-saved").is_none());
    assert!(harness.bounds("project-alpha-archived").is_none());
}

#[test]
fn add_project_offers_the_recent_projects_history_knows_about_or_a_picked_folder() {
    let mut harness = DesktopHarnessBuilder::default()
        .session("alpha-session", "/alpha", "2026-01-02T00:00:00Z", "prompt")
        .session("beta-session", "/beta", "2026-01-02T00:00:00Z", "prompt")
        .project("/alpha")
        .build();

    harness.click("add-project").unwrap();
    assert!(harness.bounds("add-project-recent-alpha").is_none(), "a listed project is not offered");
    harness.click("add-project-recent-beta").unwrap();
    assert!(harness.bounds("project-beta").is_some());
    assert!(harness.bounds("history-row-beta-session").is_some());

    harness.click("add-project").unwrap();
    harness.click("add-project-browse").unwrap();
    let prompt = harness.platform().answer_path_prompt(Some(vec![PathBuf::from("/picked")]));
    assert!(prompt.is_some_and(|prompt| prompt.directories && !prompt.multiple));
    assert!(harness.bounds("project-picked").is_some());
}

#[test]
fn a_project_folds_its_rows_away_and_back() {
    let mut harness = DesktopHarnessBuilder::default()
        .session("saved", "/alpha", "2026-01-01T00:00:00Z", "prompt")
        .project("/alpha")
        .build();
    assert!(harness.bounds("history-row-saved").is_some(), "an added project starts expanded");
    assert!(harness.bounds("project-alpha-folder-open").is_some());

    harness.click("project-alpha").unwrap();
    assert!(harness.bounds("history-row-saved").is_none());
    assert!(harness.bounds("project-alpha-folder-closed").is_some());

    harness.click("project-alpha").unwrap();
    assert!(harness.bounds("history-row-saved").is_some());
    assert!(harness.bounds("project-alpha-folder-open").is_some());
}

#[test]
fn a_project_lists_its_newest_five_saved_sessions_then_show_more_reveals_the_rest() {
    let mut harness = (1..=8)
        .fold(DesktopHarnessBuilder::default().project("/alpha"), |builder, day| {
            builder.session(&format!("day-{day}"), "/alpha", &format!("2026-01-0{day}T00:00:00Z"), "prompt")
        })
        .build();

    let painted: Vec<bool> = DAY_ROWS.iter().map(|row| harness.bounds(row).is_some()).collect();
    assert_eq!(painted, [false, false, false, true, true, true, true, true]);
    let last_row = harness.bounds("history-row-day-4").expect("oldest shown row is painted");
    let show_more = harness.bounds("project-alpha-show-more").expect("show more is painted");
    assert!(last_row.top() < show_more.top(), "show more sits below the rows");

    harness.click("project-alpha-show-more").unwrap();

    assert!(DAY_ROWS.iter().all(|row| harness.bounds(row).is_some()), "every row is painted");
    assert!(harness.bounds("project-alpha-show-more").is_none(), "nothing is left to show");
}

#[test]
fn a_projects_plus_starts_a_selected_thread_above_its_saved_sessions() {
    let mut harness = DesktopHarnessBuilder::default()
        .session("saved", "/alpha", "2026-01-01T00:00:00Z", "prompt")
        .session("alpha-session", "/alpha", "2026-01-02T00:00:00Z", "live prompt")
        .agent("/alpha", replying("alpha-session", "hello"))
        .build();

    harness.click("project-alpha-new-thread").unwrap();

    let thread = harness.connected_thread();
    assert_eq!(thread.target().cwd().map(Path::to_path_buf), Some("/alpha".into()));
    let project = harness.bounds("project-alpha").expect("project header is painted");
    let live = harness.bounds("thread-row-0").expect("live row is painted");
    let saved = harness.bounds("history-row-saved").expect("saved row is painted");
    assert!(project.top() < live.top() && live.top() < saved.top());
    assert!(harness.bounds("history-row-alpha-session").is_none(), "the live thread stands in for its session");
}

#[test]
fn clicking_a_live_row_selects_its_thread() {
    let mut harness = alpha_and_beta();
    harness.click("project-alpha-new-thread").unwrap();
    harness.click("project-beta-new-thread").unwrap();
    assert_eq!(selected_cwd(&harness), Path::new("/beta"));

    harness.click("thread-row-0").unwrap();

    assert_eq!(selected_cwd(&harness), Path::new("/alpha"));
}

#[test]
fn the_new_thread_shortcut_starts_a_thread_in_the_selected_threads_project() {
    let mut harness = DesktopHarnessBuilder::default()
        .agent("/alpha", replying("alpha-session", "hello"))
        .agent("/beta", replying("beta-session", "hello"))
        .agent("/beta", replying("second-beta-session", "hello"))
        .build();
    harness.click("project-beta-new-thread").unwrap();

    harness.press("secondary-n");

    let threads = harness.threads();
    let cwds: Vec<Option<PathBuf>> =
        threads.threads().iter().map(|t| t.target().cwd().map(Path::to_path_buf)).collect();
    assert_eq!(cwds, [Some("/beta".into()), Some("/beta".into())]);
}

#[test]
fn the_new_thread_shortcut_uses_the_first_project_when_no_thread_is_selected() {
    let mut harness = alpha_and_beta();

    harness.press("secondary-n");

    assert_eq!(selected_cwd(&harness), Path::new("/alpha"));
}

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
fn each_row_shows_how_long_ago_its_thread_started_in_a_compact_form() {
    let mut harness = DesktopHarnessBuilder::default()
        .now("2026-01-07T12:00:00Z")
        .session("seconds", "/alpha", "2026-01-07T11:59:30Z", "a")
        .session("minutes", "/alpha", "2026-01-07T11:55:00Z", "b")
        .session("hours", "/alpha", "2026-01-07T09:00:00Z", "c")
        .session("days", "/alpha", "2026-01-05T12:00:00Z", "d")
        .project("/alpha")
        .build();

    for age in [
        "history-row-seconds-age-now",
        "history-row-minutes-age-5m",
        "history-row-hours-age-3h",
        "history-row-days-age-2d",
    ] {
        assert!(harness.bounds(age).is_some(), "{age} is shown");
    }
}

#[test]
fn a_live_threads_age_counts_from_when_the_app_clock_says_it_opened() {
    let (mut harness, _) = DesktopHarnessBuilder::default()
        .now("2031-06-01T12:00:00Z")
        .agent("/alpha", replying("alpha-session", "hello"))
        .start("/alpha");

    assert!(harness.bounds("thread-row-0-age-now").is_some(), "a thread that just opened is `now` old");
}

fn alpha_and_beta() -> DesktopHarness {
    DesktopHarnessBuilder::default()
        .agent("/alpha", replying("alpha-session", "hello"))
        .agent("/beta", replying("beta-session", "hello"))
        .build()
}

fn selected_cwd(harness: &DesktopHarness) -> PathBuf {
    let thread = harness.selected_thread().expect("a thread is selected");
    thread.target().cwd().map(Path::to_path_buf).expect("the thread works locally")
}
