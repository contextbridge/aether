use crate::support::{replying, texts};
use acp_utils::testing::{FakeAgent, FakeAgentRequests};
use aether_desktop::testing::{DesktopHarness, DesktopHarnessBuilder};
use aether_desktop::threads::{Thread, ThreadStatus};
use agent_client_protocol as acp;
use agent_client_protocol::schema::v2::PromptRequest;
use gpui::{Entity, TestAppContext};
use std::path::PathBuf;
use utils::testing::TestWorkspace;

#[gpui::test]
fn enter_or_secondary_enter_sends_the_prompt_and_clears_the_composer_and_shift_enter_does_not(cx: &mut TestAppContext) {
    let (harness, thread) = DesktopHarness::alpha(cx, replying("alpha-session", "hello"));
    assert!(harness.bounds(cx, "transcript-row-0").is_none(), "an empty thread shows no rows");
    assert!(harness.bounds(cx, "stop").is_none(), "nothing runs yet");

    harness.send(cx, &thread, "hi there");
    harness.send(cx, &thread, "again");
    assert_eq!(thread.read_with(cx, |thread, _| texts(thread)), ["hi there", "hello", "again", "hello"]);
    assert!(harness.shows(cx, "composer-text", ""));
    assert!(harness.bounds(cx, "transcript-row-3").is_some(), "the second reply is painted");

    harness.simulate_input(cx, "once more");
    harness.simulate_keystrokes(cx, "secondary-enter");
    harness.run_until(cx, |cx| texts(thread.read(cx)).len() == 6);
    assert!(harness.shows(cx, "composer-text", ""));

    harness.simulate_input(cx, "line one");
    harness.simulate_keystrokes(cx, "shift-enter");
    harness.simulate_input(cx, "line two");
    assert!(harness.shows(cx, "composer-text", "line one\nline two"));
    assert_eq!(thread.read_with(cx, |thread, _| texts(thread)).len(), 6, "shift-enter sends nothing");
}

#[gpui::test]
fn stop_takes_the_place_of_send_while_the_turn_runs_and_cancels_it(cx: &mut TestAppContext) {
    let (harness, thread, mut requests) = working_thread(cx, "/alpha");
    assert_eq!(thread.read_with(cx, |thread, _| thread.status()), ThreadStatus::Running);
    assert!(harness.bounds(cx, "stop").is_some());
    assert!(harness.bounds(cx, "send").is_none());

    harness.click(cx, "stop").unwrap();

    let cancel = harness.next(cx, &mut requests.cancel);
    assert_eq!(cancel.session_id.0.as_ref(), "alpha-session");
}

#[gpui::test]
fn a_prompt_written_during_the_turn_brings_send_back_to_queue_it(cx: &mut TestAppContext) {
    let (harness, thread, _requests) = working_thread(cx, "/alpha");

    harness.simulate_input(cx, "and then this");
    assert!(harness.bounds(cx, "stop").is_none());
    harness.click(cx, "send").unwrap();
    harness.run_until(cx, |cx| thread.read(cx).core().is_some_and(|core| !core.queued_prompts().is_empty()));

    assert!(harness.shows(cx, "composer-text", ""));
    assert!(harness.bounds(cx, "stop").is_some(), "the emptied composer offers stop again");
}

#[gpui::test]
fn every_prompt_the_agent_rejects_at_once_is_given_back(cx: &mut TestAppContext) {
    let (harness, thread, mut requests) = working_thread(cx, "/alpha");
    harness.simulate_input(cx, "and then this");
    harness.click(cx, "send").unwrap();
    harness.run_until(cx, |_| requests.prompt.len() == 2);

    for _ in 0..2 {
        let (_, responder) = requests.held::<PromptRequest>().try_recv().unwrap();
        responder.respond_with_error(acp::Error::internal_error()).unwrap();
    }
    harness.run_until(cx, |cx| thread.read(cx).core().is_some_and(|core| core.queued_prompts().is_empty()));

    assert!(harness.shows(cx, "composer-text", "work forever\nand then this"));
}

#[gpui::test]
fn escape_stops_the_running_turn_and_keeps_the_draft(cx: &mut TestAppContext) {
    let (harness, _, mut requests) = working_thread(cx, "/alpha");
    harness.simulate_input(cx, "half a thought");

    harness.simulate_keystrokes(cx, "escape");

    let cancel = harness.next(cx, &mut requests.cancel);
    assert_eq!(cancel.session_id.0.as_ref(), "alpha-session");
    assert!(harness.shows(cx, "composer-text", "half a thought"));
}

#[gpui::test]
fn escape_closes_the_file_menu_before_it_stops_the_turn(cx: &mut TestAppContext) {
    let workspace = TestWorkspace::new().git_init().file("README.md", "# alpha\n");
    let (harness, _, requests) = working_thread(cx, workspace.root());
    harness.simulate_input(cx, "@read");
    harness.run_until_painted(cx, "completion-item-0");

    harness.simulate_keystrokes(cx, "escape");
    assert!(harness.bounds(cx, "completion-menu").is_none());
    harness.simulate_keystrokes(cx, "escape");
    harness.run_until(cx, |_| !requests.cancel.is_empty());

    assert!(harness.shows(cx, "composer-text", "@read"));
}

#[gpui::test]
fn each_thread_keeps_its_own_transcript_and_draft_across_selection(cx: &mut TestAppContext) {
    let (harness, alpha) = DesktopHarnessBuilder::default()
        .agent("/alpha", replying("alpha-session", "from alpha"))
        .agent("/beta", replying("beta-session", "from beta"))
        .start(cx, "/alpha");
    harness.send(cx, &alpha, "one");
    harness.simulate_input(cx, "alpha draft");
    let beta = harness.start_thread(cx, "/beta");
    assert!(harness.bounds(cx, "transcript-row-0").is_none(), "beta's transcript is empty");
    assert!(harness.shows(cx, "composer-text", ""));

    harness.click(cx, "thread-row-0").unwrap();
    assert!(harness.bounds(cx, "transcript-row-1").is_some(), "alpha's exchange is painted");
    assert!(harness.shows(cx, "composer-text", "alpha draft"));

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
    let (agent, requests) = FakeAgent::new("alpha-session").hold::<PromptRequest>().capture();
    let (harness, thread) = DesktopHarnessBuilder::default().agent(&cwd, agent).start(cx, &cwd);
    harness.simulate_input(cx, "work forever");
    harness.simulate_keystrokes(cx, "enter");
    harness.run_until(cx, |_| !requests.prompt.is_empty());
    (harness, thread, requests)
}
