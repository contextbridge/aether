use crate::support::{Edges, choose, files_mode, replying, serve_diff, texts, write_agents_file};
use aether_desktop::testing::{DesktopHarness, DesktopHarnessBuilder};
use aether_desktop::threads::Thread;
use clankerdiff_core::{DiffScope, FileStatus, FilesMode, StageState};
use client_core::testing::FakeGit;
use serde_json::json;
use utils::testing::TestWorkspace;

#[test]
fn the_files_tab_counts_the_files_the_agent_reports_as_they_change() {
    let (mut harness, thread, git) = with_changes(&["src/main.rs", "README.md"]);
    assert!(harness.shows("changes-count", "2"));
    harness.click("tab-files").unwrap();

    git.add_file("src/lib.rs", "pub fn lib() {}\n");

    wait_for_changes(&mut harness, thread, 3);
    assert!(harness.shows("changes-count", "3"), "no refresh is needed");
}

#[test]
fn the_view_switch_in_the_header_swaps_the_chat_for_the_diff_without_moving() {
    let (mut harness, _, _) = with_changes(&["src/main.rs"]);
    assert!(harness.bounds("tab-chat-icon").is_some());
    assert!(harness.bounds("tab-files-icon").is_some());
    let switch = harness.bounds("view-switch").expect("a switch while there are changes to see");
    let header = harness.bounds("thread-header").expect("a header");
    assert!(switch.top() >= header.top() && switch.bottom() <= header.bottom(), "the switch sits in the header");

    harness.click("tab-files").unwrap();
    assert!(harness.bounds("changes-viewer").is_some(), "the diff is painted");
    assert_eq!(harness.bounds("view-switch"), Some(switch), "the switch has not moved");
    for chat in ["transcript", "composer", "session-options", "status"] {
        assert!(harness.bounds(chat).is_none(), "`{chat}` belongs to the chat");
    }

    harness.click("tab-chat").unwrap();
    assert!(harness.bounds("transcript").is_some(), "the chat is back");
    assert!(harness.bounds("changes-viewer").is_none(), "the diff is hidden");
}

#[test]
fn shortcuts_switch_between_chat_and_files_and_escape_returns_to_the_chat() {
    let (mut harness, _, _) = with_changes(&["src/main.rs"]);

    harness.press("secondary-2");
    assert!(harness.bounds("changes-viewer").is_some());
    harness.press("secondary-1");
    assert!(harness.bounds("transcript").is_some());

    harness.press("secondary-2 escape");
    assert!(harness.bounds("transcript").is_some());
}

#[test]
fn changes_not_yet_seen_are_marked_until_the_files_tab_opens() {
    let (mut harness, _, _) = with_changes(&["src/main.rs"]);
    assert!(harness.bounds("changes-unseen").is_some(), "new changes are marked");

    harness.click("tab-files").unwrap();
    harness.click("tab-chat").unwrap();

    assert!(harness.bounds("changes-unseen").is_none(), "seen changes are no longer marked");
}

#[test]
fn a_clean_working_tree_is_browsed_and_its_diff_shows_no_changes_in_whichever_scope() {
    let (mut harness, thread, _) = with_changes(&[]);
    assert!(harness.bounds("view-switch").is_some(), "the switch keeps its place");
    assert!(harness.bounds("changes-unseen").is_none(), "there is nothing new to see");

    harness.press("secondary-2");
    assert!(!harness.shows("changes-count", "0"), "a clean working tree counts nothing");
    assert_eq!(files_mode(&harness, thread), Some(FilesMode::Browse), "with nothing to review, the files are browsed");
    harness.click("files-mode-diff").unwrap();
    assert!(viewer_says(&mut harness, "No changes"));

    harness.click("scope-Staged").unwrap();
    harness.run_until(|_| scope(thread) == Some(DiffScope::Staged));
}

#[test]
fn the_review_pill_floats_over_the_diff_counting_its_comments_and_submits_them() {
    let (mut harness, thread, _) = with_changes(&["src/main.rs"]);
    harness.click("tab-files").unwrap();
    assert!(harness.bounds("review-submit").is_none(), "there is nothing to submit yet");

    comment(&mut harness, "rename this");
    assert!(harness.shows("review-comments", "1 comment"));
    let pill = harness.bounds("review-pill").expect("a pill once there is a comment");
    let viewer = harness.bounds("changes-viewer").expect("the diff");
    assert!(pill.top() >= viewer.top() && pill.bottom() <= viewer.bottom(), "the pill floats over the diff");
    assert!(harness.bounds("submit-review").is_none(), "the diff has no review bar of its own");
    assert!(harness.bounds("cancel-review").is_none());
    harness.click("review-submit").unwrap();

    assert_review_sent(&mut harness, thread, "rename this");
}

#[test]
fn submitting_a_review_sends_its_comments_as_a_prompt_and_returns_to_the_chat() {
    let (mut harness, thread, _) = with_changes(&["src/main.rs"]);
    harness.click("tab-files").unwrap();

    comment(&mut harness, "rename this");
    harness.press("s");

    assert_review_sent(&mut harness, thread, "rename this");
}

#[test]
fn copying_the_review_puts_it_on_the_clipboard() {
    let (mut harness, _, _) = with_changes(&["src/main.rs"]);
    harness.click("tab-files").unwrap();

    comment(&mut harness, "rename this");
    harness.press("y");

    let copied = harness.platform().copied_text();
    assert!(copied.is_some_and(|text| text.contains("rename this")));
}

#[test]
fn staging_everything_from_the_command_list_stages_the_repository() {
    let (mut harness, _, git) = with_changes(&["src/main.rs", "README.md"]);
    harness.click("tab-files").unwrap();

    harness.press("tab shift-/");
    harness.type_text("stage all");
    harness.press("enter");

    harness.run_until(|_| git.status("src/main.rs") == Some((FileStatus::Added, StageState::Staged)));
    assert_eq!(git.status("README.md"), Some((FileStatus::Added, StageState::Staged)));
}

#[test]
fn the_changes_at_an_agents_diff_endpoint_are_shown_in_place_of_those_it_serves() {
    let served = TestWorkspace::new().git_init().file("a.rs", "fn a() {}\n").file("b.rs", "fn b() {}\n");
    let diff = serve_diff(served.root());
    let project = tempfile::tempdir().unwrap();
    write_agents_file(
        project.path(),
        &json!({ "agents": { "aether": { "type": "stdio", "command": "aether", "diff": { "url": diff.url } } } }),
    );
    let (mut harness, thread) = DesktopHarnessBuilder::default()
        .agent(project.path(), replying("aether-session", "reply"))
        .git(project.path(), repository(&["src/main.rs"]))
        .start(project.path());

    wait_for_changes(&mut harness, thread, 2);
}

#[test]
fn a_diff_endpoint_that_cannot_be_resolved_shows_why_in_place_of_the_changes() {
    let project = tempfile::tempdir().unwrap();
    let diff = json!({ "url": "ws://${AETHER_DESKTOP_TEST_UNSET_HOST}/ws" });
    write_agents_file(
        project.path(),
        &json!({ "agents": { "aether": { "type": "stdio", "command": "aether", "diff": diff } } }),
    );
    let (mut harness, thread) = DesktopHarnessBuilder::default()
        .agent(project.path(), replying("aether-session", "reply"))
        .git(project.path(), FakeGit::new(project.path()))
        .start(project.path());
    harness.run_until(|_| thread.review().is_some());

    harness.press("secondary-2");

    let status = thread.review().and_then(|review| review.status());
    assert!(status.is_some_and(|status| status.contains("AETHER_DESKTOP_TEST_UNSET_HOST")));
    assert!(viewer_says(&mut harness, "Could not load changes"));
}

#[test]
fn a_diff_endpoint_still_resolving_holds_up_the_changes_but_not_the_chat() {
    let project = tempfile::tempdir().unwrap();
    let diff = json!({ "endpoint": { "command": "./diff-endpoint.sh" } });
    write_agents_file(
        project.path(),
        &json!({ "agents": { "aether": { "type": "stdio", "command": "aether", "diff": diff } } }),
    );
    let (mut harness, thread) = DesktopHarnessBuilder::default()
        .agent(project.path(), replying("aether-session", "hello"))
        .git(project.path(), FakeGit::new(project.path()))
        .start(project.path());

    harness.send(thread, "hi");

    assert_eq!(texts(thread), ["hi", "hello"]);
    assert!(thread.review().is_none(), "the changes are still loading");
}

#[test]
fn a_stopped_thread_has_no_changes_to_show() {
    let (mut harness, thread, _) = with_changes(&["src/main.rs"]);

    choose(&mut harness, "thread-row-0", "menu-item-Stop thread");

    assert_eq!(thread.changed_files(), None);
}

fn with_changes(paths: &[&str]) -> (DesktopHarness, Thread, FakeGit) {
    let git = repository(paths);
    let (mut harness, thread) = DesktopHarnessBuilder::default()
        .agent("/alpha", replying("alpha-session", "reply"))
        .git("/alpha", git.clone())
        .start("/alpha");
    wait_for_changes(&mut harness, thread, paths.len());
    (harness, thread, git)
}

fn repository(paths: &[&str]) -> FakeGit {
    paths.iter().fold(FakeGit::new("/alpha"), |git, path| git.with_untracked(*path, "fn main() {}\n"))
}

fn comment(harness: &mut DesktopHarness, text: &str) {
    harness.press("c");
    harness.type_text(text);
    harness.press("secondary-enter");
}

fn assert_review_sent(harness: &mut DesktopHarness, thread: Thread, comment: &str) {
    harness.run_until(|_| texts(thread).iter().any(|text| text == "reply"));
    let prompt = texts(thread).into_iter().find(|text| text.contains("src/main.rs"));
    assert!(prompt.is_some_and(|prompt| prompt.contains(comment)), "the review names the file and the comment");
    assert!(harness.bounds("transcript").is_some(), "the chat is back");
}

fn wait_for_changes(harness: &mut DesktopHarness, thread: Thread, files: usize) {
    harness.run_until(|_| thread.changed_files() == Some(files));
}

fn viewer_says(harness: &mut DesktopHarness, text: &str) -> bool {
    harness.text("changes-viewer").is_some_and(|shown| shown.contains(text))
}

fn scope(thread: Thread) -> Option<DiffScope> {
    thread.review()?.snapshot.as_ref().and_then(|snapshot| snapshot.target.scope())
}
