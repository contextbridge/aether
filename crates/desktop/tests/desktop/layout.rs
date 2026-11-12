use crate::support::{Edges, replying};
use aether_desktop::sidebar::MIN_MAIN_WIDTH;
use aether_desktop::testing::DesktopHarnessBuilder;
use clankerdiff_rouge_shell::rouge::core::Point;

#[test]
fn widening_the_sidebar_leaves_room_for_the_thread() {
    let mut harness = DesktopHarnessBuilder::default().window_size(640.0, 480.0).build();
    let min_main = MIN_MAIN_WIDTH * harness.rem_size();

    harness.drag("panel-resize-handle", Point::new(600.0, 240.0)).unwrap();

    let main = harness.bounds("main").expect("the main region is painted");
    assert!((main.size.width - min_main).abs() < 0.5, "{main:?}");
}

#[test]
fn collapsing_the_sidebar_gives_its_room_to_the_thread_and_its_toggle_to_the_header() {
    let (mut harness, _) = DesktopHarnessBuilder::default()
        .window_size(640.0, 480.0)
        .agent("/alpha", replying("alpha-session", "hello"))
        .start("/alpha");
    let settings = harness.bounds("open-settings").expect("settings is painted in the sidebar");
    let sidebar = harness.bounds("sidebar").expect("the sidebar is painted");
    let expanded = harness.bounds("toggle-sidebar").expect("the toggle is painted in the sidebar");
    assert!(expanded.left() >= settings.right(), "{settings:?} {expanded:?}");
    assert!(expanded.right() <= sidebar.right(), "{sidebar:?} {expanded:?}");
    assert!((expanded.center().y - settings.center().y).abs() < 0.5, "{settings:?} {expanded:?}");

    harness.click("toggle-sidebar").unwrap();

    assert!(harness.bounds("sidebar").is_none(), "the sidebar is hidden");
    let main = harness.bounds("main").expect("the main region is painted");
    assert!((main.size.width - 640.0).abs() < 0.5, "{main:?}");
    assert!(harness.bounds("toggle-sidebar").is_some(), "the toggle is painted in the thread header");
    assert!(harness.bounds("thread-header").is_some());

    harness.click("toggle-sidebar").unwrap();
    assert!(harness.bounds("project-alpha").is_some(), "the sidebar shows again");

    harness.press("secondary-b");
    assert!(harness.bounds("sidebar").is_none(), "the shortcut hides the sidebar");
    harness.press("secondary-b");
    assert!(harness.bounds("sidebar").is_some(), "the shortcut shows the sidebar again");
}
