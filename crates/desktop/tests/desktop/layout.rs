use aether_desktop::{SIDEBAR_WIDTH, testing::DesktopHarnessBuilder};
use gpui::{TestAppContext, point, px, size};

#[gpui::test]
fn sidebar_and_main_fill_the_default_window(cx: &mut TestAppContext) {
    assert_regions_fill_window(cx, 1280.0, 840.0);
}

#[gpui::test]
fn sidebar_keeps_its_width_in_a_narrow_window(cx: &mut TestAppContext) {
    assert_regions_fill_window(cx, 640.0, 480.0);
}

fn assert_regions_fill_window(cx: &mut TestAppContext, width: f32, height: f32) {
    let harness = DesktopHarnessBuilder::default().window_size(width, height).build(cx);

    let sidebar = harness.bounds(cx, "sidebar").expect("sidebar is painted");
    let main = harness.bounds(cx, "main").expect("main region is painted");

    assert_eq!(sidebar.origin, point(px(0.0), px(0.0)));
    assert_eq!(sidebar.size, size(SIDEBAR_WIDTH, px(height)));
    assert_eq!(main.origin, point(SIDEBAR_WIDTH, px(0.0)));
    assert_eq!(main.size, size(px(width) - SIDEBAR_WIDTH, px(height)));
}
