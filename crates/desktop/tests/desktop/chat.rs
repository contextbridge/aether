use crate::support::{replying, texts};
use acp_utils::testing::{FakeAgent, FakeAgentRequests};
use aether_desktop::testing::{DesktopHarness, DesktopHarnessBuilder};
use aether_desktop::threads::{Thread, ThreadStatus};
use agent_client_protocol as acp;
use agent_client_protocol::v2::schema::PromptRequest;
use std::path::PathBuf;
use utils::testing::TestWorkspace;

#[test]
fn enter_or_secondary_enter_sends_the_prompt_and_clears_the_composer_and_shift_enter_does_not() {
    let (mut harness, thread) = DesktopHarness::alpha(replying("alpha-session", "hello"));
    assert!(harness.bounds("transcript-row-0").is_none(), "an empty thread shows no rows");
    assert!(harness.bounds("stop").is_none(), "nothing runs yet");

    harness.send(thread, "hi there");
    harness.send(thread, "again");
    assert_eq!(texts(thread), ["hi there", "hello", "again", "hello"]);
    assert!(harness.shows("composer-text", ""));
    assert!(harness.bounds("transcript-row-3").is_some(), "the second reply is painted");

    harness.type_text("once more");
    harness.press("secondary-enter");
    harness.run_until(|_| texts(thread).len() == 6);
    assert!(harness.shows("composer-text", ""));

    harness.type_text("line one");
    harness.press("shift-enter");
    harness.type_text("line two");
    assert!(harness.shows("composer-text", "line one\nline two"));
    assert_eq!(texts(thread).len(), 6, "shift-enter sends nothing");
}

#[test]
fn stop_takes_the_place_of_send_while_the_turn_runs_and_cancels_it() {
    let (mut harness, thread, mut requests) = working_thread("/alpha");
    assert_eq!(thread.status(), ThreadStatus::Running);
    assert!(harness.bounds("stop").is_some());
    assert!(harness.bounds("send").is_none());

    harness.click("stop").unwrap();

    let cancel = harness.next(&mut requests.cancel);
    assert_eq!(cancel.session_id.0.as_ref(), "alpha-session");
}

#[test]
fn a_prompt_written_during_the_turn_brings_send_back_to_queue_it() {
    let (mut harness, thread, _requests) = working_thread("/alpha");

    harness.type_text("and then this");
    assert!(harness.bounds("stop").is_none());
    harness.click("send").unwrap();
    harness.run_until(|_| thread.with_core(|core| !core.queued_prompts().is_empty()).unwrap_or(false));

    assert!(harness.shows("composer-text", ""));
    assert!(harness.bounds("stop").is_some(), "the emptied composer offers stop again");
}

#[test]
fn every_prompt_the_agent_rejects_at_once_is_given_back() {
    let (mut harness, thread, mut requests) = working_thread("/alpha");
    harness.type_text("and then this");
    harness.click("send").unwrap();
    harness.next(&mut requests.prompt);

    for _ in 0..2 {
        let (_, responder) = requests.held::<PromptRequest>().try_recv().unwrap();
        responder.respond_with_error(acp::Error::internal_error()).unwrap();
    }
    harness.run_until(|_| thread.with_core(|core| core.queued_prompts().is_empty()).unwrap_or(false));

    assert!(harness.shows("composer-text", "work forever\nand then this"));
}

#[test]
fn escape_stops_the_running_turn_and_keeps_the_draft() {
    let (mut harness, _, mut requests) = working_thread("/alpha");
    harness.type_text("half a thought");

    harness.press("escape");

    let cancel = harness.next(&mut requests.cancel);
    assert_eq!(cancel.session_id.0.as_ref(), "alpha-session");
    assert!(harness.shows("composer-text", "half a thought"));
}

#[test]
fn escape_closes_the_file_menu_before_it_stops_the_turn() {
    let workspace = TestWorkspace::new().git_init().file("README.md", "# alpha\n");
    let (mut harness, _, mut requests) = working_thread(workspace.root());
    harness.type_text("@read");
    harness.run_until_painted("completion-item-0");

    harness.press("escape");
    assert!(harness.bounds("completion-menu").is_none());
    harness.press("escape");
    harness.next(&mut requests.cancel);

    assert!(harness.shows("composer-text", "@read"));
}

#[test]
fn each_thread_keeps_its_own_transcript_and_draft_across_selection() {
    let (mut harness, alpha) = DesktopHarnessBuilder::default()
        .agent("/alpha", replying("alpha-session", "from alpha"))
        .agent("/beta", replying("beta-session", "from beta"))
        .start("/alpha");
    harness.send(alpha, "one");
    harness.type_text("alpha draft");
    let beta = harness.start_thread("/beta");
    assert!(harness.bounds("transcript-row-0").is_none(), "beta's transcript is empty");
    assert!(harness.shows("composer-text", ""));

    harness.click("thread-row-0").unwrap();
    assert!(harness.bounds("transcript-row-1").is_some(), "alpha's exchange is painted");
    assert!(harness.shows("composer-text", "alpha draft"));

    harness.click("thread-row-1").unwrap();
    harness.send(beta, "two");
    assert_eq!(texts(alpha), ["one", "from alpha"]);
    assert_eq!(texts(beta), ["two", "from beta"]);
}

fn working_thread(cwd: impl Into<PathBuf>) -> (DesktopHarness, Thread, FakeAgentRequests) {
    let cwd = cwd.into();
    let (agent, mut requests) = FakeAgent::new("alpha-session").hold::<PromptRequest>().capture();
    let (mut harness, thread) = DesktopHarnessBuilder::default().agent(&cwd, agent).start(&cwd);
    harness.type_text("work forever");
    harness.press("enter");
    harness.next(&mut requests.prompt);
    (harness, thread, requests)
}
