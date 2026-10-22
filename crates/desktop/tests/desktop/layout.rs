use crate::support::assert_close;
use aether_desktop::sidebar::{DEFAULT_SIDEBAR_WIDTH, MAX_SIDEBAR_WIDTH, MIN_MAIN_WIDTH, MIN_SIDEBAR_WIDTH};
use aether_desktop::testing::{DesktopHarness, DesktopHarnessBuilder};
use gpui::{Pixels, TestAppContext, point, px, size};

#[gpui::test]
fn sidebar_and_main_fill_the_default_window(cx: &mut TestAppContext) {
    assert_regions_fill_window(cx, 1280.0, 840.0);
}

#[gpui::test]
fn sidebar_keeps_its_width_in_a_narrow_window(cx: &mut TestAppContext) {
    assert_regions_fill_window(cx, 640.0, 480.0);
}

#[gpui::test]
fn dragging_the_sidebar_edge_resizes_it(cx: &mut TestAppContext) {
    let harness = DesktopHarnessBuilder::default().build(cx);

    harness.drag(cx, "sidebar-resize-handle", px(400.0)).unwrap();

    assert_close(sidebar_width(&harness, cx), px(400.0));
    let main = harness.bounds(cx, "main").expect("main region is painted");
    assert_close(main.origin.x, px(400.0));
}

#[gpui::test]
fn the_sidebar_stays_between_its_narrowest_and_widest(cx: &mut TestAppContext) {
    let harness = DesktopHarnessBuilder::default().build(cx);
    let rem_size = harness.rem_size(cx);

    harness.drag(cx, "sidebar-resize-handle", px(1200.0)).unwrap();
    assert_close(sidebar_width(&harness, cx), MAX_SIDEBAR_WIDTH.to_pixels(rem_size));

    harness.drag(cx, "sidebar-resize-handle", px(10.0)).unwrap();
    assert_close(sidebar_width(&harness, cx), MIN_SIDEBAR_WIDTH.to_pixels(rem_size));
}

#[gpui::test]
fn widening_the_sidebar_leaves_room_for_the_thread(cx: &mut TestAppContext) {
    let harness = DesktopHarnessBuilder::default().window_size(640.0, 480.0).build(cx);
    let rem_size = harness.rem_size(cx);

    harness.drag(cx, "sidebar-resize-handle", px(600.0)).unwrap();

    assert_close(sidebar_width(&harness, cx), px(640.0) - MIN_MAIN_WIDTH.to_pixels(rem_size));
}

#[gpui::test]
fn double_clicking_the_sidebar_edge_restores_its_width(cx: &mut TestAppContext) {
    let harness = DesktopHarnessBuilder::default().build(cx);
    harness.drag(cx, "sidebar-resize-handle", px(400.0)).unwrap();

    harness.double_click(cx, "sidebar-resize-handle").unwrap();

    assert_close(sidebar_width(&harness, cx), DEFAULT_SIDEBAR_WIDTH.to_pixels(harness.rem_size(cx)));
}

#[gpui::test]
fn the_sidebar_width_is_kept_across_launches(first: &mut TestAppContext, second: &mut TestAppContext) {
    let state = tempfile::tempdir().unwrap();
    let harness = DesktopHarnessBuilder::default().state_dir(state.path()).build(first);
    harness.drag(first, "sidebar-resize-handle", px(400.0)).unwrap();
    first.run_until_parked();

    let relaunched = DesktopHarnessBuilder::default().state_dir(state.path()).build(second);

    assert_close(sidebar_width(&relaunched, second), px(400.0));
}

fn assert_regions_fill_window(cx: &mut TestAppContext, width: f32, height: f32) {
    let harness = DesktopHarnessBuilder::default().window_size(width, height).build(cx);

    let sidebar_width = DEFAULT_SIDEBAR_WIDTH.to_pixels(harness.rem_size(cx));
    let sidebar = harness.bounds(cx, "sidebar").expect("sidebar is painted");
    let main = harness.bounds(cx, "main").expect("main region is painted");

    assert_eq!(sidebar.origin, point(px(0.0), px(0.0)));
    assert_eq!(sidebar.size, size(sidebar_width, px(height)));
    assert_eq!(main.origin, point(sidebar_width, px(0.0)));
    assert_eq!(main.size, size(px(width) - sidebar_width, px(height)));
}

fn sidebar_width(harness: &DesktopHarness, cx: &mut TestAppContext) -> Pixels {
    harness.bounds(cx, "sidebar").expect("sidebar is painted").size.width
}
