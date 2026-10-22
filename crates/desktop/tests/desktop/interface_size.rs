use aether_desktop::sidebar::DEFAULT_SIDEBAR_WIDTH;
use aether_desktop::testing::{DesktopHarness, DesktopHarnessBuilder};
use aether_desktop::ui::interface_size::{DEFAULT_INTERFACE_SIZE, MAX_INTERFACE_SIZE};
use gpui::{TestAppContext, px};

#[gpui::test]
fn the_interface_starts_at_the_platforms_default_size(cx: &mut TestAppContext) {
    let harness = DesktopHarnessBuilder::default().build(cx);

    assert_eq!(harness.rem_size(cx), px(DEFAULT_INTERFACE_SIZE));
    assert_sidebar_scales(&harness, cx);
}

#[gpui::test]
fn the_size_shortcuts_grow_shrink_and_reset_the_whole_interface(cx: &mut TestAppContext) {
    let harness = DesktopHarnessBuilder::default().build(cx);

    harness.simulate_keystrokes(cx, "secondary-=");
    assert_eq!(harness.rem_size(cx), px(DEFAULT_INTERFACE_SIZE + 1.0));
    assert_sidebar_scales(&harness, cx);

    harness.simulate_keystrokes(cx, "secondary-- secondary--");
    assert_eq!(harness.rem_size(cx), px(DEFAULT_INTERFACE_SIZE - 1.0));
    assert_sidebar_scales(&harness, cx);

    harness.simulate_keystrokes(cx, "secondary-0");
    assert_eq!(harness.rem_size(cx), px(DEFAULT_INTERFACE_SIZE));
}

#[gpui::test]
fn the_interface_stops_growing_at_its_largest_size(cx: &mut TestAppContext) {
    let harness = DesktopHarnessBuilder::default().build(cx);

    for _ in 0..40 {
        harness.simulate_keystrokes(cx, "secondary-=");
    }

    assert_eq!(harness.rem_size(cx), px(MAX_INTERFACE_SIZE));
}

#[gpui::test]
fn the_interface_size_is_kept_across_launches(first: &mut TestAppContext, second: &mut TestAppContext) {
    let state = tempfile::tempdir().unwrap();
    let harness = DesktopHarnessBuilder::default().state_dir(state.path()).build(first);
    harness.simulate_keystrokes(first, "secondary-= secondary-=");
    first.run_until_parked();

    let relaunched = DesktopHarnessBuilder::default().state_dir(state.path()).build(second);

    assert_eq!(relaunched.rem_size(second), px(DEFAULT_INTERFACE_SIZE + 2.0));
}

fn assert_sidebar_scales(harness: &DesktopHarness, cx: &mut TestAppContext) {
    let sidebar = harness.bounds(cx, "sidebar").expect("the sidebar is painted");
    assert_eq!(sidebar.size.width, DEFAULT_SIDEBAR_WIDTH.to_pixels(harness.rem_size(cx)));
}
