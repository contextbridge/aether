use crate::support::replying;
use aether_desktop::sidebar::MIN_MAIN_WIDTH;
use aether_desktop::testing::DesktopHarnessBuilder;
use gpui::{TestAppContext, point, px};

#[gpui::test]
fn widening_the_sidebar_leaves_room_for_the_thread(cx: &mut TestAppContext) {
    let harness = DesktopHarnessBuilder::default().window_size(640.0, 480.0).build(cx);
    let min_main = MIN_MAIN_WIDTH.to_pixels(harness.rem_size(cx));

    harness.drag(cx, "sidebar-resize-handle", point(px(600.0), px(240.0))).unwrap();

    let main = harness.bounds(cx, "main").expect("the main region is painted");
    assert!((main.size.width - min_main).abs() < px(0.5), "{main:?}");
}

#[gpui::test]
fn collapsing_the_sidebar_gives_its_room_to_the_thread(cx: &mut TestAppContext) {
    let harness = DesktopHarnessBuilder::default().window_size(640.0, 480.0).project("/alpha").build(cx);

    harness.click(cx, "toggle-sidebar").unwrap();

    assert!(harness.bounds(cx, "sidebar").is_none(), "the sidebar is hidden");
    let main = harness.bounds(cx, "main").expect("the main region is painted");
    assert!((main.size.width - px(640.0)).abs() < px(0.5), "{main:?}");

    harness.click(cx, "toggle-sidebar").unwrap();

    assert!(harness.bounds(cx, "project-alpha").is_some(), "the sidebar shows again");
}

#[gpui::test]
fn the_sidebar_toggle_stays_put_when_a_thread_is_selected(cx: &mut TestAppContext) {
    let harness = DesktopHarnessBuilder::default().agent("/alpha", replying("alpha-session", "hello")).build(cx);
    harness.start_thread(cx, "/alpha");
    let expanded = harness.bounds(cx, "toggle-sidebar").expect("the toggle is painted in the sidebar");

    harness.simulate_keystrokes(cx, "secondary-b");

    assert!(harness.bounds(cx, "sidebar").is_none(), "the shortcut hides the sidebar");
    let collapsed = harness.bounds(cx, "toggle-sidebar").expect("the toggle is painted in the thread header");
    assert!((collapsed.left() - expanded.left()).abs() < px(0.5), "{expanded:?} {collapsed:?}");
    assert!(harness.bounds(cx, "thread-header").is_some());

    harness.simulate_keystrokes(cx, "secondary-b");

    assert!(harness.bounds(cx, "sidebar").is_some(), "the shortcut shows the sidebar again");
}
