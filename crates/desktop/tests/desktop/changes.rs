use crate::support::{choose, replying, start_thread, texts};
use acp_utils::testing::FakeAgent;
use aether_desktop::testing::{DesktopHarness, DesktopHarnessBuilder, FakeConnector};
use aether_desktop::threads::Thread;
use agent_client_protocol::schema::v2::NewSessionResponse;
use clankerdiff_core::{DiffScope, FileStatus, StageState};
use client_core::testing::FakeGit;
use gpui::{Entity, TestAppContext};

#[gpui::test]
fn the_changes_tab_counts_the_files_the_agent_reports(cx: &mut TestAppContext) {
    let harness = alpha(cx, repository(&["src/main.rs", "README.md"]));

    let thread = start_thread(&harness, cx, "project-alpha-new-thread");
    wait_for_changes(&harness, cx, &thread, 2);

    assert_eq!(harness.changes_label(cx).as_deref(), Some("Changes (2)"));
}

#[gpui::test]
fn the_changes_tab_shows_the_diff_in_place_of_the_chat(cx: &mut TestAppContext) {
    let harness = alpha(cx, repository(&["src/main.rs"]));
    let thread = start_thread(&harness, cx, "project-alpha-new-thread");
    wait_for_changes(&harness, cx, &thread, 1);

    harness.click(cx, "tab-changes").unwrap();
    assert!(harness.bounds(cx, "changes-viewer").is_some(), "the diff is painted");
    assert!(harness.bounds(cx, "composer").is_none(), "the chat is hidden");

    harness.click(cx, "tab-chat").unwrap();
    assert!(harness.bounds(cx, "composer").is_some(), "the chat is back");
    assert!(harness.bounds(cx, "changes-viewer").is_none(), "the diff is hidden");
}

#[gpui::test]
fn shortcuts_switch_between_chat_and_changes(cx: &mut TestAppContext) {
    let harness = alpha(cx, repository(&["src/main.rs"]));
    let thread = start_thread(&harness, cx, "project-alpha-new-thread");
    wait_for_changes(&harness, cx, &thread, 1);

    harness.simulate_keystrokes(cx, "secondary-2");
    assert!(harness.bounds(cx, "changes-viewer").is_some());

    harness.simulate_keystrokes(cx, "secondary-1");
    assert!(harness.bounds(cx, "composer").is_some());
}

#[gpui::test]
fn escape_in_the_diff_returns_to_the_chat(cx: &mut TestAppContext) {
    let harness = alpha(cx, repository(&["src/main.rs"]));
    let thread = start_thread(&harness, cx, "project-alpha-new-thread");
    wait_for_changes(&harness, cx, &thread, 1);
    harness.click(cx, "tab-changes").unwrap();

    harness.simulate_keystrokes(cx, "escape");

    assert!(harness.bounds(cx, "composer").is_some());
}

#[gpui::test]
fn a_clean_working_tree_shows_no_changes(cx: &mut TestAppContext) {
    let harness = alpha(cx, FakeGit::new("/alpha"));
    let thread = start_thread(&harness, cx, "project-alpha-new-thread");
    wait_for_changes(&harness, cx, &thread, 0);

    harness.click(cx, "tab-changes").unwrap();

    assert_eq!(harness.changes_label(cx).as_deref(), Some("Changes (0)"));
    assert!(harness.bounds(cx, "changes-clean").is_some());
}

#[gpui::test]
fn choosing_another_scope_on_a_clean_tree_reloads_the_diff_in_that_scope(cx: &mut TestAppContext) {
    let harness = alpha(cx, FakeGit::new("/alpha"));
    let thread = start_thread(&harness, cx, "project-alpha-new-thread");
    wait_for_changes(&harness, cx, &thread, 0);
    harness.click(cx, "tab-changes").unwrap();

    harness.click(cx, "scope-staged").unwrap();

    harness.run_until(cx, |cx| scope(thread.read(cx)) == Some(DiffScope::Staged));
}

#[gpui::test]
fn submitting_a_review_sends_its_comments_as_a_prompt_and_returns_to_the_chat(cx: &mut TestAppContext) {
    let connector = FakeConnector::default()
        .agent("/alpha", replying("alpha-session", "on it"))
        .git("/alpha", repository(&["src/main.rs"]));
    let harness = DesktopHarnessBuilder::default().connector(connector).project("/alpha").build(cx);
    let thread = start_thread(&harness, cx, "project-alpha-new-thread");
    wait_for_changes(&harness, cx, &thread, 1);
    harness.click(cx, "tab-changes").unwrap();

    harness.simulate_keystrokes(cx, "c");
    harness.simulate_input(cx, "rename this");
    harness.simulate_keystrokes(cx, "secondary-enter");
    harness.simulate_keystrokes(cx, "s");

    harness.run_until(cx, |cx| texts(thread.read(cx)).iter().any(|text| text == "on it"));
    let prompt = thread.read_with(cx, |thread, _| texts(thread)).into_iter().find(|text| text.contains("src/main.rs"));
    assert!(prompt.is_some_and(|prompt| prompt.contains("rename this")), "the review names the file and the comment");
    assert!(harness.bounds(cx, "composer").is_some(), "the chat is back");
}

#[gpui::test]
fn submitting_a_review_during_a_turn_queues_it_behind_the_turn(cx: &mut TestAppContext) {
    let (agent, requests) =
        FakeAgent::default().new_session_response(NewSessionResponse::new("alpha-session")).capture();
    let connector = FakeConnector::default().agent("/alpha", agent).git("/alpha", repository(&["src/main.rs"]));
    let harness = DesktopHarnessBuilder::default().connector(connector).project("/alpha").build(cx);
    let thread = start_thread(&harness, cx, "project-alpha-new-thread");
    wait_for_changes(&harness, cx, &thread, 1);
    harness.simulate_input(cx, "work forever");
    harness.simulate_keystrokes(cx, "enter");
    harness.run_until(cx, |_| !requests.prompt.is_empty());
    harness.click(cx, "tab-changes").unwrap();

    harness.simulate_keystrokes(cx, "s");

    harness.run_until(cx, |cx| thread.read(cx).core().is_some_and(|core| core.queued_prompts().len() == 2));
}

#[gpui::test]
fn copying_the_review_puts_it_on_the_clipboard(cx: &mut TestAppContext) {
    let harness = alpha(cx, repository(&["src/main.rs"]));
    let thread = start_thread(&harness, cx, "project-alpha-new-thread");
    wait_for_changes(&harness, cx, &thread, 1);
    harness.click(cx, "tab-changes").unwrap();

    harness.simulate_keystrokes(cx, "c");
    harness.simulate_input(cx, "rename this");
    harness.simulate_keystrokes(cx, "secondary-enter");
    harness.simulate_keystrokes(cx, "y");

    let copied = cx.read_from_clipboard().and_then(|item| item.text());
    assert!(copied.is_some_and(|text| text.contains("rename this")));
}

#[gpui::test]
fn refreshing_picks_up_files_changed_since(cx: &mut TestAppContext) {
    let mut git = repository(&["src/main.rs"]);
    let harness = alpha(cx, git.clone());
    let thread = start_thread(&harness, cx, "project-alpha-new-thread");
    wait_for_changes(&harness, cx, &thread, 1);
    harness.click(cx, "tab-changes").unwrap();

    git.add_file("src/lib.rs", "pub fn lib() {}\n");
    harness.simulate_keystrokes(cx, "ctrl-r");

    wait_for_changes(&harness, cx, &thread, 2);
}

#[gpui::test]
fn staging_everything_from_the_file_list_stages_the_repository(cx: &mut TestAppContext) {
    let git = repository(&["src/main.rs", "README.md"]);
    let harness = alpha(cx, git.clone());
    let thread = start_thread(&harness, cx, "project-alpha-new-thread");
    wait_for_changes(&harness, cx, &thread, 2);
    harness.click(cx, "tab-changes").unwrap();

    harness.simulate_keystrokes(cx, "tab a");

    harness.run_until(cx, |_| git.status("src/main.rs") == Some((FileStatus::Untracked, StageState::Staged)));
    assert_eq!(git.status("README.md"), Some((FileStatus::Untracked, StageState::Staged)));
}

#[gpui::test]
fn a_stopped_thread_has_no_changes_to_show(cx: &mut TestAppContext) {
    let harness = alpha(cx, repository(&["src/main.rs"]));
    let thread = start_thread(&harness, cx, "project-alpha-new-thread");
    wait_for_changes(&harness, cx, &thread, 1);

    choose(&harness, cx, "thread-row-0", "menu-stop");

    assert_eq!(thread.read_with(cx, |thread, _| thread.changed_files()), None);
}

/// A harness whose only project, `/alpha`, has an agent that serves `git`.
fn alpha(cx: &mut TestAppContext, git: FakeGit) -> DesktopHarness {
    let connector = FakeConnector::default().agent("/alpha", replying("alpha-session", "reply")).git("/alpha", git);
    DesktopHarnessBuilder::default().connector(connector).project("/alpha").build(cx)
}

/// A repository at `/alpha` with each of `paths` added and not yet committed.
fn repository(paths: &[&str]) -> FakeGit {
    let mut git = FakeGit::new("/alpha");
    for path in paths {
        git.add_file(*path, "fn main() {}\n");
    }
    git
}

fn wait_for_changes(harness: &DesktopHarness, cx: &mut TestAppContext, thread: &Entity<Thread>, files: usize) {
    harness.run_until(cx, |cx| thread.read(cx).changed_files() == Some(files));
}

fn scope(thread: &Thread) -> Option<DiffScope> {
    thread.review()?.snapshot.as_ref().map(|snapshot| snapshot.scope)
}
