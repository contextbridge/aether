use crate::support::{choose, replying};
use aether_desktop::testing::{DesktopHarness, DesktopHarnessBuilder};
use gpui::TestAppContext;

#[gpui::test]
fn renaming_a_thread_retitles_its_row_and_header_until_the_name_is_cleared(cx: &mut TestAppContext) {
    let (harness, thread) = DesktopHarness::alpha(cx, replying("alpha-session", "hello"));
    harness.submit(cx, &thread, "first prompt");
    assert!(harness.shows(cx, "thread-row-0-title", "first prompt"));

    choose(&harness, cx, "thread-row-0", "menu-rename");
    harness.simulate_input(cx, "Renamed");
    harness.simulate_keystrokes(cx, "enter");

    assert!(harness.bounds(cx, "rename-input").is_none(), "Enter ends the edit");
    assert!(harness.shows(cx, "thread-row-0-title", "Renamed"));
    assert!(harness.shows(cx, "thread-title", "Renamed"));

    choose(&harness, cx, "thread-row-0", "menu-rename");
    harness.simulate_keystrokes(cx, "backspace enter");

    assert!(harness.shows(cx, "thread-row-0-title", "first prompt"));
    assert!(harness.shows(cx, "thread-title", "first prompt"));
}

#[gpui::test]
fn a_rename_keeps_spaces_and_escape_cancels_it(cx: &mut TestAppContext) {
    let harness = with_sessions(cx, &["saved"]);

    choose(&harness, cx, "history-row-saved", "menu-rename");
    harness.simulate_keystrokes(cx, "a space b enter");
    assert!(harness.shows(cx, "history-row-saved-title", "a b"));

    choose(&harness, cx, "history-row-saved", "menu-rename");
    harness.simulate_input(cx, "Never mind");
    harness.simulate_keystrokes(cx, "escape");
    assert!(harness.bounds(cx, "rename-input").is_none());
    assert!(harness.shows(cx, "history-row-saved-title", "a b"));
}

#[gpui::test]
fn archiving_removes_a_row_from_the_sidebar(cx: &mut TestAppContext) {
    let harness = with_sessions(cx, &["kept", "gone"]);

    choose(&harness, cx, "history-row-gone", "menu-archive");

    assert!(harness.bounds(cx, "history-row-gone").is_none());
    assert!(harness.bounds(cx, "history-row-kept").is_some());
    assert!(harness.bounds(cx, "project-alpha-archived").is_none(), "archived rows have no group to show in");
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
fn archiving_a_live_thread_stops_its_agent_and_removes_its_row(cx: &mut TestAppContext) {
    let (harness, _) = DesktopHarness::alpha(cx, replying("alpha-session", "hello"));

    choose(&harness, cx, "thread-row-0", "menu-archive");

    harness.run_until(cx, |_| harness.connector().live_agents("/alpha") == 0);
    assert!(harness.bounds(cx, "thread-row-0").is_none());
    assert!(harness.bounds(cx, "history-row-alpha-session").is_none());
}

fn with_sessions(cx: &mut TestAppContext, ids: &[&str]) -> DesktopHarness {
    let builder =
        ids.iter().enumerate().fold(DesktopHarnessBuilder::default().project("/alpha"), |builder, (day, id)| {
            builder.session(id, "/alpha", &format!("2026-01-{:02}T00:00:00Z", day + 1), format!("{id} prompt").as_str())
        });
    builder.build(cx)
}
