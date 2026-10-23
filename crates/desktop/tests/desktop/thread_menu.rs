use crate::support::{choose, replying};
use aether_desktop::testing::{DesktopHarness, DesktopHarnessBuilder};
use gpui::TestAppContext;

#[gpui::test]
fn renaming_a_thread_retitles_its_row_and_header_until_the_name_is_cleared(cx: &mut TestAppContext) {
    let harness = DesktopHarnessBuilder::default().agent("/alpha", replying("alpha-session", "hello")).build(cx);
    let thread = harness.start_thread(cx, "/alpha");
    harness.submit(cx, &thread, "first prompt");
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
fn a_rename_keeps_spaces_and_escape_cancels_it(cx: &mut TestAppContext) {
    let harness = with_sessions(cx, &["saved"]);

    choose(&harness, cx, "history-row-saved", "menu-rename");
    harness.simulate_keystrokes(cx, "a space b enter");
    assert_eq!(harness.row_title(cx, "history-row-saved").as_deref(), Some("a b"));

    choose(&harness, cx, "history-row-saved", "menu-rename");
    harness.simulate_input(cx, "Never mind");
    harness.simulate_keystrokes(cx, "escape");
    assert!(harness.bounds(cx, "rename-input").is_none());
    assert_eq!(harness.row_title(cx, "history-row-saved").as_deref(), Some("a b"));
}

#[gpui::test]
fn pinning_moves_rows_to_the_top_of_their_project_in_pin_order_until_unpinned(cx: &mut TestAppContext) {
    let harness = with_sessions(cx, &["day-1", "day-2", "day-3"]);

    choose(&harness, cx, "history-row-day-1", "menu-pin");
    choose(&harness, cx, "history-row-day-2", "menu-pin");
    assert_eq!(row_order(&harness, cx, &["history-row-day-2", "history-row-day-1", "history-row-day-3"]), [0, 1, 2]);

    choose(&harness, cx, "history-row-day-2", "menu-unpin");
    assert_eq!(row_order(&harness, cx, &["history-row-day-1", "history-row-day-3", "history-row-day-2"]), [0, 1, 2]);
}

#[gpui::test]
fn archiving_moves_a_row_into_its_projects_archived_group(cx: &mut TestAppContext) {
    let harness = with_sessions(cx, &["kept", "gone"]);
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
    let harness = with_sessions(cx, &["day-1", "day-2", "day-3", "day-4", "day-5", "day-6"]);
    assert!(harness.bounds(cx, "history-row-day-1").is_none(), "the oldest session is behind Show more");

    choose(&harness, cx, "history-row-day-6", "menu-archive");

    assert!(harness.bounds(cx, "history-row-day-1").is_some());
    assert!(harness.bounds(cx, "project-alpha-show-more").is_none());
}

#[gpui::test]
fn archiving_a_live_thread_leaves_its_agent_running(cx: &mut TestAppContext) {
    let harness = DesktopHarnessBuilder::default().agent("/alpha", replying("alpha-session", "hello")).build(cx);
    harness.start_thread(cx, "/alpha");

    choose(&harness, cx, "thread-row-0", "menu-archive");
    harness.click(cx, "project-alpha-archived").unwrap();

    assert_eq!(harness.connector().live_agents("/alpha"), 1);
    assert!(harness.bounds(cx, "thread-row-0-status-running").is_none());
    assert!(harness.bounds(cx, "thread-row-0").is_some(), "the thread is listed in the archived group");
}

/// The app with `/alpha` added and the saved sessions `ids` in it, each a day newer than the one
/// before it.
fn with_sessions(cx: &mut TestAppContext, ids: &[&str]) -> DesktopHarness {
    let builder =
        ids.iter().enumerate().fold(DesktopHarnessBuilder::default().project("/alpha"), |builder, (day, id)| {
            builder.session(id, "/alpha", &format!("2026-01-{:02}T00:00:00Z", day + 1), format!("{id} prompt").as_str())
        });
    builder.build(cx)
}

/// Where each of `rows` is painted, top to bottom, as its rank among them.
pub(crate) fn row_order(harness: &DesktopHarness, cx: &mut TestAppContext, rows: &[&'static str]) -> Vec<usize> {
    let tops: Vec<_> =
        rows.iter().map(|row| harness.bounds(cx, row).unwrap_or_else(|| panic!("`{row}` is painted")).top()).collect();
    tops.iter().map(|top| tops.iter().filter(|other| *other < top).count()).collect()
}
