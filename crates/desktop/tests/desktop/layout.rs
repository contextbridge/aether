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
