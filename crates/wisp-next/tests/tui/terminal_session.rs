use wisp_next::test_support::terminal::{inline_viewport_height, inline_viewport_needs_resync};

#[test]
fn inline_viewport_reserves_two_rows_for_scrollback() {
    assert_eq!(inline_viewport_height(15), 13);
    assert_eq!(inline_viewport_height(3), 1);
    assert_eq!(inline_viewport_height(2), 1);
    assert_eq!(inline_viewport_height(1), 1);
    assert_eq!(inline_viewport_height(0), 0);
}

#[test]
fn viewport_matching_the_window_does_not_resync() {
    assert!(!inline_viewport_needs_resync(15, 13));
    assert!(!inline_viewport_needs_resync(3, 1));
    assert!(!inline_viewport_needs_resync(1, 1));
    assert!(!inline_viewport_needs_resync(0, 0));
}

#[test]
fn shrunken_window_resyncs_to_a_clamped_viewport() {
    assert!(inline_viewport_needs_resync(10, 13));
    assert_eq!(inline_viewport_height(10), 8);
}

#[test]
fn regrown_window_resyncs_back_up() {
    assert!(inline_viewport_needs_resync(20, 8));
    assert_eq!(inline_viewport_height(20), 18);
}
