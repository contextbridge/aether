use crate::support::replying;
use aether_desktop::sidebar::MIN_MAIN_WIDTH;
use aether_desktop::testing::DesktopHarnessBuilder;
#[cfg(target_os = "macos")]
use aether_desktop::ui::WINDOW_BUTTONS_INSET;
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
fn collapsing_the_sidebar_gives_its_room_to_the_thread_and_its_toggle_to_the_header(cx: &mut TestAppContext) {
    let (harness, _) = DesktopHarnessBuilder::default()
        .window_size(640.0, 480.0)
        .agent("/alpha", replying("alpha-session", "hello"))
        .start(cx, "/alpha");
    let settings = harness.bounds(cx, "open-settings").expect("settings is painted in the sidebar");
    let sidebar = harness.bounds(cx, "sidebar").expect("the sidebar is painted");
    let expanded = harness.bounds(cx, "toggle-sidebar").expect("the toggle is painted in the sidebar");
    assert!(expanded.left() >= settings.right(), "{settings:?} {expanded:?}");
    assert!(expanded.right() <= sidebar.right(), "{sidebar:?} {expanded:?}");
    assert!((expanded.center().y - settings.center().y).abs() < px(0.5), "{settings:?} {expanded:?}");

    harness.click(cx, "toggle-sidebar").unwrap();

    assert!(harness.bounds(cx, "sidebar").is_none(), "the sidebar is hidden");
    let main = harness.bounds(cx, "main").expect("the main region is painted");
    assert!((main.size.width - px(640.0)).abs() < px(0.5), "{main:?}");
    assert!(harness.bounds(cx, "toggle-sidebar").is_some(), "the toggle is painted in the thread header");
    assert!(harness.bounds(cx, "thread-header").is_some());

    harness.click(cx, "toggle-sidebar").unwrap();
    assert!(harness.bounds(cx, "project-alpha").is_some(), "the sidebar shows again");

    harness.simulate_keystrokes(cx, "secondary-b");
    assert!(harness.bounds(cx, "sidebar").is_none(), "the shortcut hides the sidebar");
    harness.simulate_keystrokes(cx, "secondary-b");
    assert!(harness.bounds(cx, "sidebar").is_some(), "the shortcut shows the sidebar again");
}

#[cfg(target_os = "macos")]
#[gpui::test]
fn the_top_bar_clears_the_window_buttons_except_in_full_screen(cx: &mut TestAppContext) {
    let (harness, _) = DesktopHarnessBuilder::default()
        .window_size(800.0, 600.0)
        .agent("/alpha", replying("alpha-session", "hello"))
        .start(cx, "/alpha");
    let clears_buttons = |toggle: gpui::Bounds<gpui::Pixels>| (toggle.left() - WINDOW_BUTTONS_INSET).abs() < px(0.5);

    harness.simulate_keystrokes(cx, "secondary-b");

    let collapsed = harness.bounds(cx, "toggle-sidebar").expect("the toggle is painted in the thread header");
    assert!(clears_buttons(collapsed), "{collapsed:?}");

    harness.enter_full_screen(cx, 1440.0, 900.0);

    let full_screen = harness.bounds(cx, "toggle-sidebar").expect("the toggle is painted in the thread header");
    assert!(full_screen.left() < WINDOW_BUTTONS_INSET, "macOS hides the buttons in full screen: {full_screen:?}");
}
