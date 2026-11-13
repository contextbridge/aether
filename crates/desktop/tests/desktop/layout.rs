use crate::support::{Edges, replying};
use aether_desktop::sidebar::MIN_MAIN_WIDTH;
use aether_desktop::testing::{DesktopHarness, DesktopHarnessBuilder};
use aether_desktop::threads::{Thread, ThreadStatus};
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

#[test]
fn the_transcript_scrolls_beneath_the_header_and_the_composer_that_float_over_it() {
    let (mut harness, thread) = long_thread();
    let transcript = harness.bounds("transcript").expect("the transcript is painted");
    let heading = harness.bounds("thread-heading").expect("the heading is painted");
    let composer = harness.bounds("composer").expect("the composer is painted");

    assert!(transcript.top() <= heading.top(), "{transcript:?} {heading:?}");
    assert!(composer.bottom() <= transcript.bottom(), "{transcript:?} {composer:?}");
    assert_eq!(thread.status(), ThreadStatus::Idle);
}

#[test]
fn the_latest_reply_rests_above_the_composer_however_tall_the_composer_grows() {
    let (mut harness, _) = long_thread();
    let rests_above = |harness: &mut DesktopHarness| {
        let reply = harness.bounds("reply-1").expect("the reply is painted");
        let composer = harness.bounds("composer").expect("the composer is painted");
        assert!(reply.bottom() <= composer.top(), "{reply:?} {composer:?}");
        composer
    };
    let short = rests_above(&mut harness);

    harness.type_text(&"a draft long enough to wrap onto several lines ".repeat(12));

    let tall = rests_above(&mut harness);
    assert!(tall.size.height > short.size.height, "{short:?} {tall:?}");
}

#[test]
fn the_first_prompt_clears_the_header_when_the_transcript_is_scrolled_to_its_start() {
    let (mut harness, _) = long_thread();

    harness.scroll("transcript-view", Point::new(0.0, 100_000.0)).unwrap();

    let heading = harness.bounds("thread-heading").expect("the heading is painted");
    let prompt = harness.bounds("prompt-0").expect("the prompt is painted");
    assert!(prompt.top() >= heading.bottom(), "{heading:?} {prompt:?}");
}

fn long_thread() -> (DesktopHarness, Thread) {
    let (mut harness, thread) = DesktopHarness::alpha(replying("alpha-session", &"A long reply.\n\n".repeat(80)));
    harness.send(thread, "go");
    (harness, thread)
}
