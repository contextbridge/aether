use crate::support::{choose, replying};
use aether_desktop::testing::{DesktopHarness, DesktopHarnessBuilder};

#[test]
fn renaming_a_thread_retitles_its_row_and_header_until_the_name_is_cleared() {
    let (mut harness, thread) = DesktopHarness::alpha(replying("alpha-session", "hello"));
    harness.submit(thread, "first prompt");
    assert!(harness.shows("thread-row-0-title", "first prompt"));

    choose(&mut harness, "thread-row-0", "menu-item-Rename");
    harness.type_text("Renamed");
    harness.press("enter");

    assert!(harness.bounds("rename-input").is_none(), "Enter ends the edit");
    assert!(harness.shows("thread-row-0-title", "Renamed"));
    assert!(harness.shows("thread-title", "Renamed"));

    choose(&mut harness, "thread-row-0", "menu-item-Rename");
    harness.press("backspace enter");

    assert!(harness.shows("thread-row-0-title", "first prompt"));
    assert!(harness.shows("thread-title", "first prompt"));
}

#[test]
fn a_rename_keeps_spaces_and_escape_cancels_it() {
    let mut harness = with_sessions(&["saved"]);

    choose(&mut harness, "history-row-saved", "menu-item-Rename");
    harness.press("a space b enter");
    assert!(harness.shows("history-row-saved-title", "a b"));

    choose(&mut harness, "history-row-saved", "menu-item-Rename");
    harness.type_text("Never mind");
    harness.press("escape");
    assert!(harness.bounds("rename-input").is_none());
    assert!(harness.shows("history-row-saved-title", "a b"));
}

#[test]
fn archiving_removes_a_row_from_the_sidebar() {
    let mut harness = with_sessions(&["kept", "gone"]);

    choose(&mut harness, "history-row-gone", "menu-item-Archive");

    assert!(harness.bounds("history-row-gone").is_none());
    assert!(harness.bounds("history-row-kept").is_some());
    assert!(harness.bounds("project-alpha-archived").is_none(), "archived rows have no group to show in");
}

#[test]
fn archived_rows_do_not_count_toward_show_more() {
    let mut harness = with_sessions(&["day-1", "day-2", "day-3", "day-4", "day-5", "day-6"]);
    assert!(harness.bounds("history-row-day-1").is_none(), "the oldest session is behind Show more");

    choose(&mut harness, "history-row-day-6", "menu-item-Archive");

    assert!(harness.bounds("history-row-day-1").is_some());
    assert!(harness.bounds("project-alpha-show-more").is_none());
}

#[test]
fn archiving_a_live_thread_stops_its_agent_and_removes_its_row() {
    let (mut harness, _) = DesktopHarness::alpha(replying("alpha-session", "hello"));

    choose(&mut harness, "thread-row-0", "menu-item-Archive");

    harness.run_until(|harness| harness.connector().live_agents("/alpha") == 0);
    assert!(harness.bounds("thread-row-0").is_none());
    assert!(harness.bounds("history-row-alpha-session").is_none());
}

fn with_sessions(ids: &[&str]) -> DesktopHarness {
    let builder =
        ids.iter().enumerate().fold(DesktopHarnessBuilder::default().project("/alpha"), |builder, (day, id)| {
            builder.session(id, "/alpha", &format!("2026-01-{:02}T00:00:00Z", day + 1), format!("{id} prompt").as_str())
        });
    builder.build()
}
