use crate::support::{choose, replying, serve_diff, texts, write_agents_file};
use aether_desktop::testing::{DesktopHarness, DesktopHarnessBuilder};
use aether_desktop::threads::Thread;
use clankerdiff_core::{DiffScope, FileStatus, StageState};
use client_core::testing::FakeGit;
use gpui::{Entity, TestAppContext};
use serde_json::json;
use utils::testing::TestWorkspace;

#[gpui::test]
fn the_files_tab_counts_the_files_the_agent_reports_as_they_change(cx: &mut TestAppContext) {
    let (harness, thread, git) = with_changes(cx, &["src/main.rs", "README.md"]);
    assert!(harness.shows(cx, "changes-count", "2"));
    harness.click(cx, "tab-files").unwrap();

    git.add_file("src/lib.rs", "pub fn lib() {}\n");

    wait_for_changes(&harness, cx, &thread, 3);
    assert!(harness.shows(cx, "changes-count", "3"), "no refresh is needed");
}

#[gpui::test]
fn the_view_switch_in_the_header_swaps_the_chat_for_the_diff_without_moving(cx: &mut TestAppContext) {
    let (harness, _, _) = with_changes(cx, &["src/main.rs"]);
    assert!(harness.bounds(cx, "tab-chat-icon").is_some());
    assert!(harness.bounds(cx, "tab-files-icon").is_some());
    let switch = harness.bounds(cx, "view-switch").expect("a switch while there are changes to see");
    let header = harness.bounds(cx, "thread-header").expect("a header");
    assert!(switch.top() >= header.top() && switch.bottom() <= header.bottom(), "the switch sits in the header");

    harness.click(cx, "tab-files").unwrap();
    assert!(harness.bounds(cx, "changes-viewer").is_some(), "the diff is painted");
    assert_eq!(harness.bounds(cx, "view-switch"), Some(switch), "the switch has not moved");
    for chat in ["transcript", "composer", "session-options", "status"] {
        assert!(harness.bounds(cx, chat).is_none(), "`{chat}` belongs to the chat");
    }

    harness.click(cx, "tab-chat").unwrap();
    assert!(harness.bounds(cx, "transcript").is_some(), "the chat is back");
    assert!(harness.bounds(cx, "changes-viewer").is_none(), "the diff is hidden");
}

#[gpui::test]
fn shortcuts_switch_between_chat_and_files_and_escape_returns_to_the_chat(cx: &mut TestAppContext) {
    let (harness, _, _) = with_changes(cx, &["src/main.rs"]);

    harness.simulate_keystrokes(cx, "secondary-2");
    assert!(harness.bounds(cx, "changes-viewer").is_some());
    harness.simulate_keystrokes(cx, "secondary-1");
    assert!(harness.bounds(cx, "transcript").is_some());

    harness.simulate_keystrokes(cx, "secondary-2 escape");
    assert!(harness.bounds(cx, "transcript").is_some());
}

#[gpui::test]
fn changes_not_yet_seen_are_marked_until_the_files_tab_opens(cx: &mut TestAppContext) {
    let (harness, _, _) = with_changes(cx, &["src/main.rs"]);
    assert!(harness.bounds(cx, "changes-unseen").is_some(), "new changes are marked");

    harness.click(cx, "tab-files").unwrap();
    harness.click(cx, "tab-chat").unwrap();

    assert!(harness.bounds(cx, "changes-unseen").is_none(), "seen changes are no longer marked");
}

#[gpui::test]
fn a_clean_working_tree_is_browsed_and_its_diff_shows_no_changes_in_whichever_scope(cx: &mut TestAppContext) {
    let (harness, thread, _) = with_changes(cx, &[]);
    assert!(harness.bounds(cx, "view-switch").is_some(), "the switch keeps its place");
    assert!(harness.bounds(cx, "changes-unseen").is_none(), "there is nothing new to see");

    harness.simulate_keystrokes(cx, "secondary-2");
    assert!(!harness.shows(cx, "changes-count", "0"), "a clean working tree counts nothing");
    assert!(harness.bounds(cx, "browse-files").is_some(), "with nothing to review, the files are browsed");
    harness.click(cx, "files-mode-diff").unwrap();
    assert!(harness.bounds(cx, "changes-clean").is_some());

    harness.click(cx, "scope-staged").unwrap();
    harness.run_until(cx, |cx| scope(thread.read(cx)) == Some(DiffScope::Staged));
}

#[gpui::test]
fn the_review_pill_floats_over_the_diff_counting_its_comments_and_submits_them(cx: &mut TestAppContext) {
    let (harness, thread, _) = with_changes(cx, &["src/main.rs"]);
    harness.click(cx, "tab-files").unwrap();
    assert!(harness.bounds(cx, "review-submit").is_none(), "there is nothing to submit yet");

    comment(&harness, cx, "rename this");
    assert!(harness.shows(cx, "review-comments", "1 comment"));
    let pill = harness.bounds(cx, "review-pill").expect("a pill once there is a comment");
    let viewer = harness.bounds(cx, "changes-viewer").expect("the diff");
    assert!(pill.top() >= viewer.top() && pill.bottom() <= viewer.bottom(), "the pill floats over the diff");
    assert!(harness.bounds(cx, "submit-review").is_none(), "the diff has no review bar of its own");
    assert!(harness.bounds(cx, "cancel-review").is_none());
    harness.click(cx, "review-submit").unwrap();

    assert_review_sent(&harness, cx, &thread, "rename this");
}

#[gpui::test]
fn submitting_a_review_sends_its_comments_as_a_prompt_and_returns_to_the_chat(cx: &mut TestAppContext) {
    let (harness, thread, _) = with_changes(cx, &["src/main.rs"]);
    harness.click(cx, "tab-files").unwrap();

    comment(&harness, cx, "rename this");
    harness.simulate_keystrokes(cx, "s");

    assert_review_sent(&harness, cx, &thread, "rename this");
}

#[gpui::test]
fn copying_the_review_puts_it_on_the_clipboard(cx: &mut TestAppContext) {
    let (harness, _, _) = with_changes(cx, &["src/main.rs"]);
    harness.click(cx, "tab-files").unwrap();

    comment(&harness, cx, "rename this");
    harness.simulate_keystrokes(cx, "y");

    let copied = cx.read_from_clipboard().and_then(|item| item.text());
    assert!(copied.is_some_and(|text| text.contains("rename this")));
}

#[gpui::test]
fn staging_everything_from_the_command_list_stages_the_repository(cx: &mut TestAppContext) {
    let (harness, _, git) = with_changes(cx, &["src/main.rs", "README.md"]);
    harness.click(cx, "tab-files").unwrap();

    harness.simulate_keystrokes(cx, "tab shift-/");
    harness.simulate_input(cx, "stage all");
    harness.simulate_keystrokes(cx, "enter");

    harness.run_until(cx, |_| git.status("src/main.rs") == Some((FileStatus::Added, StageState::Staged)));
    assert_eq!(git.status("README.md"), Some((FileStatus::Added, StageState::Staged)));
}

#[gpui::test]
fn the_changes_at_an_agents_diff_endpoint_are_shown_in_place_of_those_it_serves(cx: &mut TestAppContext) {
    let served = TestWorkspace::new().git_init().file("a.rs", "fn a() {}\n").file("b.rs", "fn b() {}\n");
    let diff = serve_diff(served.root());
    let project = tempfile::tempdir().unwrap();
    write_agents_file(
        project.path(),
        &json!({ "agents": { "aether": { "type": "stdio", "command": "aether", "diff": { "url": diff.url } } } }),
    );
    let (harness, thread) = DesktopHarnessBuilder::default()
        .agent(project.path(), replying("aether-session", "reply"))
        .git(project.path(), repository(&["src/main.rs"]))
        .start(cx, project.path());

    wait_for_changes(&harness, cx, &thread, 2);
}

#[gpui::test]
fn a_diff_endpoint_that_cannot_be_resolved_shows_why_in_place_of_the_changes(cx: &mut TestAppContext) {
    let project = tempfile::tempdir().unwrap();
    let diff = json!({ "url": "ws://${AETHER_DESKTOP_TEST_UNSET_HOST}/ws" });
    write_agents_file(
        project.path(),
        &json!({ "agents": { "aether": { "type": "stdio", "command": "aether", "diff": diff } } }),
    );
    let (harness, thread) = DesktopHarnessBuilder::default()
        .agent(project.path(), replying("aether-session", "reply"))
        .git(project.path(), FakeGit::new(project.path()))
        .start(cx, project.path());
    harness.run_until(cx, |cx| thread.read(cx).review().is_some());

    harness.simulate_keystrokes(cx, "secondary-2");

    let status = thread.read_with(cx, |thread, _| thread.review().and_then(|review| review.status()));
    assert!(status.is_some_and(|status| status.contains("AETHER_DESKTOP_TEST_UNSET_HOST")));
    assert!(harness.bounds(cx, "changes-status").is_some());
}

#[gpui::test]
fn a_diff_endpoint_still_resolving_holds_up_the_changes_but_not_the_chat(cx: &mut TestAppContext) {
    let project = tempfile::tempdir().unwrap();
    let diff = json!({ "endpoint": { "command": "./diff-endpoint.sh" } });
    write_agents_file(
        project.path(),
        &json!({ "agents": { "aether": { "type": "stdio", "command": "aether", "diff": diff } } }),
    );
    let (harness, thread) = DesktopHarnessBuilder::default()
        .agent(project.path(), replying("aether-session", "hello"))
        .git(project.path(), FakeGit::new(project.path()))
        .start(cx, project.path());

    harness.send(cx, &thread, "hi");

    assert_eq!(thread.read_with(cx, |thread, _| texts(thread)), ["hi", "hello"]);
    assert!(thread.read_with(cx, |thread, _| thread.review().is_none()), "the changes are still loading");
}

#[gpui::test]
fn a_stopped_thread_has_no_changes_to_show(cx: &mut TestAppContext) {
    let (harness, thread, _) = with_changes(cx, &["src/main.rs"]);

    choose(&harness, cx, "thread-row-0", "menu-stop");

    assert_eq!(thread.read_with(cx, |thread, _| thread.changed_files()), None);
}

fn with_changes(cx: &mut TestAppContext, paths: &[&str]) -> (DesktopHarness, Entity<Thread>, FakeGit) {
    let git = repository(paths);
    let (harness, thread) = DesktopHarnessBuilder::default()
        .agent("/alpha", replying("alpha-session", "reply"))
        .git("/alpha", git.clone())
        .start(cx, "/alpha");
    wait_for_changes(&harness, cx, &thread, paths.len());
    (harness, thread, git)
}

fn repository(paths: &[&str]) -> FakeGit {
    paths.iter().fold(FakeGit::new("/alpha"), |git, path| git.with_untracked(*path, "fn main() {}\n"))
}

fn comment(harness: &DesktopHarness, cx: &mut TestAppContext, text: &str) {
    harness.simulate_keystrokes(cx, "c");
    harness.simulate_input(cx, text);
    harness.simulate_keystrokes(cx, "secondary-enter");
}

fn assert_review_sent(harness: &DesktopHarness, cx: &mut TestAppContext, thread: &Entity<Thread>, comment: &str) {
    harness.run_until(cx, |cx| texts(thread.read(cx)).iter().any(|text| text == "reply"));
    let prompt = thread.read_with(cx, |thread, _| texts(thread)).into_iter().find(|text| text.contains("src/main.rs"));
    assert!(prompt.is_some_and(|prompt| prompt.contains(comment)), "the review names the file and the comment");
    assert!(harness.bounds(cx, "transcript").is_some(), "the chat is back");
}

fn wait_for_changes(harness: &DesktopHarness, cx: &mut TestAppContext, thread: &Entity<Thread>, files: usize) {
    harness.run_until(cx, |cx| thread.read(cx).changed_files() == Some(files));
}

fn scope(thread: &Thread) -> Option<DiffScope> {
    thread.review()?.snapshot.as_ref().and_then(|snapshot| snapshot.target.scope())
}
