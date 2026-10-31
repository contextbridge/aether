use crate::support::replying;
use acp_utils::testing::FakeAgent;
use aether_desktop::testing::{DesktopHarness, DesktopHarnessBuilder, static_remote};
use aether_desktop::threads::Thread;
use client_core::testing::FakeGit;
use gpui::{Entity, TestAppContext};
use std::path::Path;

#[gpui::test]
fn secondary_p_finds_a_file_by_name_and_opens_it_read_only_in_the_files_tab(cx: &mut TestAppContext) {
    let git = FakeGit::new("/alpha").with_committed("src/util.rs", UTIL).with_untracked("src/main.rs", MAIN);
    let (harness, thread) = start(cx, git);

    open_picker(&harness, cx, &thread);
    assert!(harness.bounds(cx, "file-picker-src/main.rs").is_some(), "files in the diff are listed");
    assert!(harness.bounds(cx, "file-picker-src/util.rs").is_some(), "and so are the files around them");
    harness.simulate_input(cx, "util");
    assert!(harness.bounds(cx, "file-picker-src/main.rs").is_none(), "main does not match");
    harness.simulate_keystrokes(cx, "enter");

    harness.run_until_painted(cx, "code:src/util.rs:new:1");
    assert!(harness.bounds(cx, "file-picker").is_none(), "the picker closes");
    assert!(harness.bounds(cx, "changes-viewer").is_some(), "the file opens in the files tab");
    assert!(harness.bounds(cx, "source-mode-header").is_some(), "read-only, since it is not in the diff");
}

#[gpui::test]
fn a_cloud_thread_browses_the_files_of_its_remote_checkout(cx: &mut TestAppContext) {
    let harness = DesktopHarnessBuilder::default()
        .script("/devbox", replying("cloud-session-0", "hi"))
        .git("/devbox", FakeGit::new("/devbox").with_committed("src/util.rs", UTIL))
        .remote(static_remote("devbox", "/devbox"))
        .build(cx);
    harness.click(cx, "remote-devbox-new-thread").unwrap();
    let thread = harness.connected_thread(cx);
    wait_for_review(&harness, cx, &thread);
    let cwd = thread.read_with(cx, |thread, _| thread.target().cwd().map(Path::to_path_buf));
    assert_eq!(cwd, None, "the checkout is not on this machine");

    open_picker(&harness, cx, &thread);
    harness.simulate_keystrokes(cx, "enter");

    harness.run_until_painted(cx, "code:src/util.rs:new:1");
}

#[gpui::test]
fn only_one_modal_shows_at_a_time_and_escape_returns_to_the_chat(cx: &mut TestAppContext) {
    let (harness, thread) = start(cx, FakeGit::new("/alpha").with_committed("README.md", "# alpha\n"));

    harness.simulate_keystrokes(cx, "secondary-k");
    harness.simulate_keystrokes(cx, "secondary-p");
    assert!(harness.bounds(cx, "switcher").is_some());
    assert!(harness.bounds(cx, "file-picker").is_none(), "the file picker waits for the switcher to close");

    harness.simulate_keystrokes(cx, "escape");
    open_picker(&harness, cx, &thread);
    harness.simulate_keystrokes(cx, "escape");
    assert!(harness.bounds(cx, "file-picker").is_none());
    assert!(harness.bounds(cx, "transcript").is_some());
}

const UTIL: &str = "pub fn helper() -> u32 {\n    1\n}\n";
const MAIN: &str = "fn main() {}\n";

fn start(cx: &mut TestAppContext, git: FakeGit) -> (DesktopHarness, Entity<Thread>) {
    start_with(cx, replying("alpha-session", "reply"), git)
}

fn start_with(cx: &mut TestAppContext, agent: FakeAgent, git: FakeGit) -> (DesktopHarness, Entity<Thread>) {
    let (harness, thread) =
        DesktopHarnessBuilder::default().agent("/alpha", agent).git("/alpha", git).start(cx, "/alpha");
    wait_for_review(&harness, cx, &thread);
    (harness, thread)
}

fn wait_for_review(harness: &DesktopHarness, cx: &mut TestAppContext, thread: &Entity<Thread>) {
    harness.run_until(cx, |cx| thread.read(cx).review().is_some_and(|review| review.snapshot.is_some()));
}

fn open_picker(harness: &DesktopHarness, cx: &mut TestAppContext, thread: &Entity<Thread>) {
    harness.simulate_keystrokes(cx, "secondary-p");
    harness.run_until(cx, |cx| thread.read(cx).listed_files().is_ok());
    assert!(harness.bounds(cx, "file-picker").is_some(), "the picker opens");
}
