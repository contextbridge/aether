use crate::support::{replying, texts};
use acp_utils::notifications::RemoteServerInfo;
use acp_utils::testing::FakeAgent;
use aether_desktop::testing::{DesktopHarness, DesktopHarnessBuilder, static_remote};
use aether_desktop::threads::Thread;
use agent_client_protocol::schema::v2 as acp;
use client_core::testing::FakeGit;
use gpui::{Entity, TestAppContext};
use serde_json::json;
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
fn a_cloud_thread_browses_and_mentions_the_files_of_its_remote_checkout(cx: &mut TestAppContext) {
    let remote = RemoteServerInfo { cwd: "/devbox".into(), session_id: None };
    let (agent, mut requests) = FakeAgent::new("cloud-session-0").remote_server(&remote).capture();
    let harness = DesktopHarnessBuilder::default()
        .script("/devbox", agent)
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

    harness.click(cx, "tab-chat").unwrap();
    harness.simulate_input(cx, "explain @util");
    harness.run_until_painted(cx, "completion-item-0");
    harness.simulate_keystrokes(cx, "enter enter");
    let prompt = harness.next(cx, &mut requests.prompt).prompt;
    let link = acp::ResourceLink::new("src/util.rs", "file:///devbox/src/util.rs");
    assert!(prompt.contains(&acp::ContentBlock::ResourceLink(link)), "the agent reads the file itself: {prompt:?}");
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

#[gpui::test]
fn clicking_where_a_tool_worked_opens_that_line_without_unfolding_the_tool(cx: &mut TestAppContext) {
    let agent = working_at("/alpha/src/util.rs");
    let (harness, thread) = start_with(cx, agent, FakeGit::new("/alpha").with_committed("src/util.rs", UTIL));
    harness.send(cx, &thread, "go");

    harness.click(cx, "tool-location-1").unwrap();

    harness.run_until_painted(cx, "code:src/util.rs:new:2");
    harness.click(cx, "tab-chat").unwrap();
    assert!(harness.bounds(cx, "tool-details-1").is_none(), "the tool stays folded");
}

#[gpui::test]
fn clicking_where_a_tool_edited_opens_the_diff_at_that_line(cx: &mut TestAppContext) {
    let git = FakeGit::new("/alpha").with_modified("src/util.rs", UTIL, UTIL_EDITED);
    let (harness, thread) = start_with(cx, editing_at("/alpha/src/util.rs", 2), git);
    harness.send(cx, &thread, "go");
    harness.click(cx, "tab-files").unwrap();
    harness.click(cx, "files-mode-browse").unwrap();
    harness.run_until_painted(cx, "browse-files");
    harness.click(cx, "tab-chat").unwrap();

    harness.click(cx, "tool-location-1").unwrap();

    harness.run_until_painted(cx, "code:src/util.rs:new:2");
    assert!(harness.bounds(cx, "diff-files").is_some(), "an edit opens in the diff, even from browsing");
    assert!(harness.bounds(cx, "source-mode-header").is_none());
}

#[gpui::test]
fn a_tool_location_outside_the_repository_says_why_it_cannot_be_opened(cx: &mut TestAppContext) {
    let agent = working_at("/elsewhere/notes.md");
    let (harness, thread) = start_with(cx, agent, FakeGit::new("/alpha").with_committed("src/util.rs", UTIL));
    harness.send(cx, &thread, "go");

    harness.click(cx, "tool-location-1").unwrap();

    let error = "Can't open /elsewhere/notes.md: it isn't in this thread's repository";
    harness.run_until(cx, |cx| texts(thread.read(cx)).iter().any(|text| text == error));
    assert!(harness.bounds(cx, "transcript").is_some(), "the chat stays put");
}

const UTIL: &str = "pub fn helper() -> u32 {\n    1\n}\n";
const MAIN: &str = "fn main() {}\n";
const UTIL_EDITED: &str = "pub fn helper() -> u32 {\n    2\n}\n";

fn start(cx: &mut TestAppContext, git: FakeGit) -> (DesktopHarness, Entity<Thread>) {
    start_with(cx, replying("alpha-session", "reply"), git)
}

fn start_with(cx: &mut TestAppContext, agent: FakeAgent, git: FakeGit) -> (DesktopHarness, Entity<Thread>) {
    let (harness, thread) =
        DesktopHarnessBuilder::default().agent("/alpha", agent).git("/alpha", git).start(cx, "/alpha");
    wait_for_review(&harness, cx, &thread);
    (harness, thread)
}

fn working_at(path: &str) -> FakeAgent {
    let read = acp::ToolCallUpdate::new("call-1")
        .title("Read file")
        .status(acp::ToolCallStatus::Completed)
        .locations(vec![acp::ToolCallLocation::new(path).line(2)])
        .meta(serde_json::Map::from_iter([("display_value".to_owned(), json!("util.rs, 3 lines"))]));
    FakeAgent::new("alpha-session").prompt_updates(vec![acp::SessionUpdate::ToolCallUpdate(read)])
}

fn editing_at(path: &str, line: u32) -> FakeAgent {
    let unified = "--- a/src/util.rs\n+++ b/src/util.rs\n@@ -2 +2 @@\n-    1\n+    2\n";
    let diff = acp::Diff::patch(unified, vec![acp::DiffChange::modify(path)]);
    let edit = acp::ToolCallUpdate::new("call-1")
        .title("Edit file")
        .status(acp::ToolCallStatus::Completed)
        .content(vec![acp::ToolCallContent::from(diff)])
        .locations(vec![acp::ToolCallLocation::new(path).line(line)]);
    FakeAgent::new("alpha-session").prompt_updates(vec![acp::SessionUpdate::ToolCallUpdate(edit)])
}

fn wait_for_review(harness: &DesktopHarness, cx: &mut TestAppContext, thread: &Entity<Thread>) {
    harness.run_until(cx, |cx| thread.read(cx).review().is_some_and(|review| review.snapshot.is_some()));
}

fn open_picker(harness: &DesktopHarness, cx: &mut TestAppContext, thread: &Entity<Thread>) {
    harness.simulate_keystrokes(cx, "secondary-p");
    harness.run_until(cx, |cx| thread.read(cx).listed_files().is_ok());
    assert!(harness.bounds(cx, "file-picker").is_some(), "the picker opens");
}
