use crate::support::{files_mode, replying, texts};
use acp_utils::notifications::RemoteServerInfo;
use acp_utils::testing::FakeAgent;
use aether_desktop::testing::{DesktopHarness, DesktopHarnessBuilder, static_remote};
use aether_desktop::threads::Thread;
use agent_client_protocol::v2::schema as acp;
use clankerdiff_core::FilesMode;
use client_core::testing::FakeGit;
use serde_json::json;
use std::path::Path;

#[test]
fn secondary_p_finds_a_file_by_name_and_opens_it_read_only_in_the_files_tab() {
    let git = FakeGit::new("/alpha").with_committed("src/util.rs", UTIL).with_untracked("src/main.rs", MAIN);
    let (mut harness, thread) = start(git);

    open_picker(&mut harness, thread);
    assert!(harness.bounds("file-picker-src/main.rs").is_some(), "files in the diff are listed");
    assert!(harness.bounds("file-picker-src/util.rs").is_some(), "and so are the files around them");
    harness.type_text("util");
    assert!(harness.bounds("file-picker-src/main.rs").is_none(), "main does not match");
    harness.press("enter");

    wait_for_line(&mut harness, thread, "src/util.rs", 1);
    assert!(harness.bounds("file-picker").is_none(), "the picker closes");
    assert!(harness.bounds("changes-viewer").is_some(), "the file opens in the files tab");
    assert!(harness.bounds("source-mode-header").is_some(), "read-only, since it is not in the diff");
}

#[test]
fn a_file_picked_after_switching_threads_opens_in_the_thread_it_was_listed_for() {
    let mut harness = DesktopHarnessBuilder::default()
        .agent("/alpha", replying("alpha-session", "a"))
        .agent("/beta", replying("beta-session", "b"))
        .git("/alpha", FakeGit::new("/alpha").with_committed("src/util.rs", UTIL))
        .git("/beta", FakeGit::new("/beta").with_committed("README.md", "# beta\n"))
        .build();
    let beta = harness.start_thread("/beta");
    let alpha = harness.start_thread("/alpha");
    wait_for_review(&mut harness, beta);
    wait_for_review(&mut harness, alpha);

    open_picker(&mut harness, alpha);
    harness.threads().select(beta);
    assert_eq!(harness.selected_thread(), Some(beta));
    harness.click("file-picker-src/util.rs").unwrap();

    assert_eq!(harness.selected_thread(), Some(alpha), "the picked file's thread is selected");
    wait_for_line(&mut harness, alpha, "src/util.rs", 1);
}

#[test]
fn a_cloud_thread_browses_and_mentions_the_files_of_its_remote_checkout() {
    let remote = RemoteServerInfo { cwd: "/devbox".into(), session_id: None };
    let (agent, mut requests) = FakeAgent::new("cloud-session-0").remote_server(&remote).capture();
    let mut harness = DesktopHarnessBuilder::default()
        .script("/devbox", agent)
        .git("/devbox", FakeGit::new("/devbox").with_committed("src/util.rs", UTIL))
        .remote(static_remote("devbox", "/devbox"))
        .build();
    harness.click("remote-devbox-new-thread").unwrap();
    let thread = harness.connected_thread();
    wait_for_review(&mut harness, thread);
    let cwd = thread.target().cwd().map(Path::to_path_buf);
    assert_eq!(cwd, None, "the checkout is not on this machine");

    open_picker(&mut harness, thread);
    harness.press("enter");
    wait_for_line(&mut harness, thread, "src/util.rs", 1);

    harness.click("tab-chat").unwrap();
    harness.type_text("explain @util");
    harness.run_until_painted("completion-item-0");
    harness.press("enter enter");
    let prompt = harness.next(&mut requests.prompt).prompt;
    let link = acp::ResourceLink::new("src/util.rs", "file:///devbox/src/util.rs");
    assert!(prompt.contains(&acp::ContentBlock::ResourceLink(link)), "the agent reads the file itself: {prompt:?}");
}

#[test]
fn only_one_modal_shows_at_a_time_and_escape_returns_to_the_chat() {
    let (mut harness, thread) = start(FakeGit::new("/alpha").with_committed("README.md", "# alpha\n"));

    harness.press("secondary-k");
    harness.press("secondary-p");
    assert!(harness.bounds("switcher").is_some());
    assert!(harness.bounds("file-picker").is_none(), "the file picker waits for the switcher to close");

    harness.press("escape");
    open_picker(&mut harness, thread);
    harness.press("escape");
    assert!(harness.bounds("file-picker").is_none());
    assert!(harness.bounds("transcript").is_some());
}

#[test]
fn clicking_where_a_tool_worked_opens_that_line_without_unfolding_the_tool() {
    let agent = working_at("/alpha/src/util.rs");
    let (mut harness, thread) = start_with(agent, FakeGit::new("/alpha").with_committed("src/util.rs", UTIL));
    harness.send(thread, "go");

    harness.click("tool-location-1").unwrap();

    wait_for_line(&mut harness, thread, "src/util.rs", 2);
    harness.click("tab-chat").unwrap();
    assert!(harness.bounds("tool-details-1").is_none(), "the tool stays folded");
}

#[test]
fn clicking_where_a_tool_edited_opens_the_diff_at_that_line() {
    let git = FakeGit::new("/alpha").with_modified("src/util.rs", UTIL, UTIL_EDITED);
    let (mut harness, thread) = start_with(editing_at("/alpha/src/util.rs", 2), git);
    harness.send(thread, "go");
    harness.click("tab-files").unwrap();
    harness.click("files-mode-browse").unwrap();
    assert_eq!(files_mode(&harness, thread), Some(FilesMode::Browse));
    harness.click("tab-chat").unwrap();

    harness.click("tool-location-1").unwrap();

    wait_for_line(&mut harness, thread, "src/util.rs", 2);
    assert_eq!(files_mode(&harness, thread), Some(FilesMode::Diff), "an edit opens in the diff, even from browsing");
    assert!(harness.bounds("source-mode-header").is_none());
}

#[test]
fn a_tool_location_outside_the_repository_says_why_it_cannot_be_opened() {
    let agent = working_at("/elsewhere/notes.md");
    let (mut harness, thread) = start_with(agent, FakeGit::new("/alpha").with_committed("src/util.rs", UTIL));
    harness.send(thread, "go");

    harness.click("tool-location-1").unwrap();

    let error = "Can't open /elsewhere/notes.md: it isn't in this thread's repository";
    harness.run_until(|_| texts(thread).iter().any(|text| text == error));
    assert!(harness.bounds("transcript").is_some(), "the chat stays put");
}

const UTIL: &str = "pub fn helper() -> u32 {\n    1\n}\n";
const MAIN: &str = "fn main() {}\n";
const UTIL_EDITED: &str = "pub fn helper() -> u32 {\n    2\n}\n";

fn start(git: FakeGit) -> (DesktopHarness, Thread) {
    start_with(replying("alpha-session", "reply"), git)
}

fn start_with(agent: FakeAgent, git: FakeGit) -> (DesktopHarness, Thread) {
    let (mut harness, thread) =
        DesktopHarnessBuilder::default().agent("/alpha", agent).git("/alpha", git).start("/alpha");
    wait_for_review(&mut harness, thread);
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

fn wait_for_review(harness: &mut DesktopHarness, thread: Thread) {
    harness.run_until(|_| thread.review().is_some_and(|review| review.snapshot.is_some()));
}

fn wait_for_line(harness: &mut DesktopHarness, thread: Thread, path: &str, line: usize) {
    harness.run_until(|harness| shown_line(harness, thread) == Some((path.to_owned(), line)));
}

fn shown_line(harness: &DesktopHarness, thread: Thread) -> Option<(String, usize)> {
    let location = harness.app().diff_review(thread)?.with(|review| review.session().selected_source_line())?;
    Some((location.path.to_string(), location.line_number))
}

fn open_picker(harness: &mut DesktopHarness, thread: Thread) {
    harness.press("secondary-p");
    harness.run_until(|_| thread.listed_files().is_ok());
    assert!(harness.bounds("file-picker").is_some(), "the picker opens");
}
