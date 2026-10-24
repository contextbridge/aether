use crate::support::{replying, texts};
use acp_utils::testing::{FakeAgent, FakeAgentRequests};
use aether_desktop::testing::{DesktopHarness, DesktopHarnessBuilder};
use aether_desktop::threads::{Thread, ThreadStatus};
use agent_client_protocol as acp;
use agent_client_protocol::schema::v2::NewSessionResponse;
use gpui::{Entity, TestAppContext};
use std::fs;
use std::path::PathBuf;

#[gpui::test]
fn enter_sends_the_prompt_and_clears_the_composer(cx: &mut TestAppContext) {
    let harness = DesktopHarnessBuilder::default().agent("/alpha", replying("alpha-session", "hello")).build(cx);
    let thread = harness.start_thread(cx, "/alpha");
    assert!(harness.bounds(cx, "transcript-row-0").is_none(), "an empty thread shows no rows");

    harness.send(cx, &thread, "hi there");
    harness.send(cx, &thread, "again");

    assert_eq!(thread.read_with(cx, |thread, _| texts(thread)), ["hi there", "hello", "again", "hello"]);
    assert_eq!(harness.composer_text(cx).as_deref(), Some(""));
    assert!(harness.bounds(cx, "transcript-row-3").is_some(), "the second reply is painted");
}

#[gpui::test]
fn secondary_enter_also_sends(cx: &mut TestAppContext) {
    let harness = DesktopHarnessBuilder::default().agent("/alpha", replying("alpha-session", "hello")).build(cx);
    let thread = harness.start_thread(cx, "/alpha");

    harness.simulate_input(cx, "hi");
    harness.simulate_keystrokes(cx, "secondary-enter");
    harness.run_until(cx, |cx| texts(thread.read(cx)).len() == 2);

    assert_eq!(harness.composer_text(cx).as_deref(), Some(""));
}

#[gpui::test]
fn shift_enter_starts_a_new_line_instead_of_sending(cx: &mut TestAppContext) {
    let harness = DesktopHarnessBuilder::default().agent("/alpha", replying("alpha-session", "hello")).build(cx);
    let thread = harness.start_thread(cx, "/alpha");

    harness.simulate_input(cx, "line one");
    harness.simulate_keystrokes(cx, "shift-enter");
    harness.simulate_input(cx, "line two");

    assert_eq!(harness.composer_text(cx).as_deref(), Some("line one\nline two"));
    assert_eq!(thread.read_with(cx, |thread, _| texts(thread)), Vec::<String>::new());
}

#[gpui::test]
fn stop_cancels_the_running_turn(cx: &mut TestAppContext) {
    let (agent, mut requests) =
        FakeAgent::default().new_session_response(NewSessionResponse::new("alpha-session")).capture();
    let harness = DesktopHarnessBuilder::default().agent("/alpha", agent).build(cx);
    let thread = harness.start_thread(cx, "/alpha");
    assert!(harness.bounds(cx, "stop").is_none(), "nothing runs yet");

    harness.simulate_input(cx, "work forever");
    harness.simulate_keystrokes(cx, "enter");
    harness.run_until(cx, |_| !requests.prompt.is_empty());
    assert_eq!(thread.read_with(cx, |thread, _| thread.status()), ThreadStatus::Running);

    harness.click(cx, "stop").unwrap();
    harness.run_until(cx, |_| !requests.cancel.is_empty());

    let cancel = requests.cancel.try_recv().unwrap();
    assert_eq!(cancel.session_id.0.as_ref(), "alpha-session");
}

#[gpui::test]
fn stop_takes_the_place_of_send_while_the_turn_runs(cx: &mut TestAppContext) {
    let (harness, _, _requests) = working_thread(cx, "/alpha");

    assert!(harness.bounds(cx, "stop").is_some());
    assert!(harness.bounds(cx, "send").is_none());
}

#[gpui::test]
fn a_prompt_written_during_the_turn_brings_send_back_to_queue_it(cx: &mut TestAppContext) {
    let (harness, thread, _requests) = working_thread(cx, "/alpha");

    harness.simulate_input(cx, "and then this");
    assert!(harness.bounds(cx, "stop").is_none());
    harness.click(cx, "send").unwrap();
    harness.run_until(cx, |cx| thread.read(cx).core().is_some_and(|core| !core.queued_prompts().is_empty()));

    assert_eq!(harness.composer_text(cx).as_deref(), Some(""));
    assert!(harness.bounds(cx, "stop").is_some(), "the emptied composer offers stop again");
}

#[gpui::test]
fn every_prompt_the_agent_rejects_at_once_is_given_back(cx: &mut TestAppContext) {
    let (harness, thread, mut requests) = working_thread(cx, "/alpha");
    harness.simulate_input(cx, "and then this");
    harness.click(cx, "send").unwrap();
    harness.run_until(cx, |_| requests.prompt.len() == 2);

    for _ in 0..2 {
        let (_, responder) = requests.prompt.try_recv().unwrap();
        responder.respond_with_error(acp::Error::internal_error()).unwrap();
    }
    harness.run_until(cx, |cx| thread.read(cx).core().is_some_and(|core| core.queued_prompts().is_empty()));

    assert_eq!(harness.composer_text(cx).as_deref(), Some("work forever\nand then this"));
}

#[gpui::test]
fn escape_stops_the_running_turn_and_keeps_the_draft(cx: &mut TestAppContext) {
    let (harness, _, mut requests) = working_thread(cx, "/alpha");
    harness.simulate_input(cx, "half a thought");

    harness.simulate_keystrokes(cx, "escape");
    harness.run_until(cx, |_| !requests.cancel.is_empty());

    let cancel = requests.cancel.try_recv().unwrap();
    assert_eq!(cancel.session_id.0.as_ref(), "alpha-session");
    assert_eq!(harness.composer_text(cx).as_deref(), Some("half a thought"));
}

#[gpui::test]
fn escape_closes_the_file_menu_before_it_stops_the_turn(cx: &mut TestAppContext) {
    let workspace = tempfile::tempdir().unwrap();
    let root = workspace.path().join("alpha");
    fs::create_dir_all(&root).unwrap();
    fs::write(root.join("README.md"), "# alpha\n").unwrap();
    let (harness, _, requests) = working_thread(cx, root);
    harness.simulate_input(cx, "@read");
    harness.run_until_painted(cx, "completion-item-0");

    harness.simulate_keystrokes(cx, "escape");
    assert!(harness.bounds(cx, "completion-menu").is_none());
    harness.simulate_keystrokes(cx, "escape");
    harness.run_until(cx, |_| !requests.cancel.is_empty());

    assert_eq!(harness.composer_text(cx).as_deref(), Some("@read"));
}

#[gpui::test]
fn each_thread_keeps_its_own_transcript_and_draft_across_selection(cx: &mut TestAppContext) {
    let harness = DesktopHarnessBuilder::default()
        .agent("/alpha", replying("alpha-session", "from alpha"))
        .agent("/beta", replying("beta-session", "from beta"))
        .build(cx);
    let alpha = harness.start_thread(cx, "/alpha");
    harness.send(cx, &alpha, "one");
    harness.simulate_input(cx, "alpha draft");
    let beta = harness.start_thread(cx, "/beta");
    assert!(harness.bounds(cx, "transcript-row-0").is_none(), "beta's transcript is empty");
    assert_eq!(harness.composer_text(cx).as_deref(), Some(""));

    harness.click(cx, "thread-row-0").unwrap();
    assert!(harness.bounds(cx, "transcript-row-1").is_some(), "alpha's exchange is painted");
    assert_eq!(harness.composer_text(cx).as_deref(), Some("alpha draft"));

    harness.click(cx, "thread-row-1").unwrap();
    harness.send(cx, &beta, "two");
    assert_eq!(alpha.read_with(cx, |thread, _| texts(thread)), ["one", "from alpha"]);
    assert_eq!(beta.read_with(cx, |thread, _| texts(thread)), ["two", "from beta"]);
}

fn working_thread(
    cx: &mut TestAppContext,
    cwd: impl Into<PathBuf>,
) -> (DesktopHarness, Entity<Thread>, FakeAgentRequests) {
    let cwd = cwd.into();
    let (agent, requests) =
        FakeAgent::default().new_session_response(NewSessionResponse::new("alpha-session")).capture();
    let harness = DesktopHarnessBuilder::default().agent(&cwd, agent).build(cx);
    let thread = harness.start_thread(cx, &cwd);
    harness.simulate_input(cx, "work forever");
    harness.simulate_keystrokes(cx, "enter");
    harness.run_until(cx, |_| !requests.prompt.is_empty());
    (harness, thread, requests)
}
