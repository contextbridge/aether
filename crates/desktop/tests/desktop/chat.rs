use crate::support::{replying, start_thread, start_thread_in, texts};
use acp_utils::testing::FakeAgent;
use aether_desktop::testing::{DesktopHarness, DesktopHarnessBuilder, FakeConnector};
use aether_desktop::threads::{Thread, ThreadStatus};
use agent_client_protocol::schema::v2::NewSessionResponse;
use gpui::{Entity, TestAppContext};

#[gpui::test]
fn enter_sends_the_prompt_and_clears_the_composer(cx: &mut TestAppContext) {
    let connector = FakeConnector::default().agent("/alpha", replying("alpha-session", "hello from alpha"));
    let harness = DesktopHarnessBuilder::default().connector(connector).project("/alpha").build(cx);
    let thread = start_thread(&harness, cx, "project-alpha-new-thread");
    assert!(harness.bounds(cx, "transcript-row-0").is_none(), "an empty thread shows no rows");

    harness.simulate_input(cx, "hi there");
    harness.simulate_keystrokes(cx, "enter");
    harness.run_until(cx, |cx| texts(thread.read(cx)).len() == 2 && thread.read(cx).status() == ThreadStatus::Idle);

    assert_eq!(thread.read_with(cx, |thread, _| texts(thread)), ["hi there", "hello from alpha"]);
    assert_eq!(harness.composer_text(cx).as_deref(), Some(""));
    assert!(harness.bounds(cx, "transcript-row-1").is_some(), "the reply is painted");
}

#[gpui::test]
fn a_second_exchange_follows_the_first_in_the_transcript(cx: &mut TestAppContext) {
    let connector = FakeConnector::default().agent("/alpha", replying("alpha-session", "reply"));
    let harness = DesktopHarnessBuilder::default().connector(connector).project("/alpha").build(cx);
    let thread = start_thread(&harness, cx, "project-alpha-new-thread");

    send(&harness, cx, &thread, "first");
    send(&harness, cx, &thread, "second");

    assert_eq!(thread.read_with(cx, |thread, _| texts(thread)), ["first", "reply", "second", "reply"]);
    assert!(harness.bounds(cx, "transcript-row-3").is_some(), "the second reply is painted");
}

#[gpui::test]
fn secondary_enter_also_sends(cx: &mut TestAppContext) {
    let connector = FakeConnector::default().agent("/alpha", replying("alpha-session", "hello from alpha"));
    let harness = DesktopHarnessBuilder::default().connector(connector).project("/alpha").build(cx);
    let thread = start_thread(&harness, cx, "project-alpha-new-thread");

    harness.simulate_input(cx, "hi");
    harness.simulate_keystrokes(cx, "secondary-enter");
    harness.run_until(cx, |cx| texts(thread.read(cx)).len() == 2);

    assert_eq!(harness.composer_text(cx).as_deref(), Some(""));
}

#[gpui::test]
fn shift_enter_starts_a_new_line_instead_of_sending(cx: &mut TestAppContext) {
    let connector = FakeConnector::default().agent("/alpha", replying("alpha-session", "hello from alpha"));
    let harness = DesktopHarnessBuilder::default().connector(connector).project("/alpha").build(cx);
    let thread = start_thread(&harness, cx, "project-alpha-new-thread");

    harness.simulate_input(cx, "line one");
    harness.simulate_keystrokes(cx, "shift-enter");
    harness.simulate_input(cx, "line two");

    assert_eq!(harness.composer_text(cx).as_deref(), Some("line one\nline two"));
    assert_eq!(thread.read_with(cx, |thread, _| texts(thread)), Vec::<String>::new());
}

#[gpui::test]
fn a_prompt_the_agent_never_received_is_given_back(cx: &mut TestAppContext) {
    let harness = DesktopHarnessBuilder::default().project("/nowhere").build(cx);
    let thread = start_thread_in(&harness, cx, "project-nowhere-new-thread");
    harness.run_until(cx, |cx| matches!(thread.read(cx).status(), ThreadStatus::Errored(_)));

    harness.simulate_input(cx, "hello?");
    harness.simulate_keystrokes(cx, "enter");

    assert_eq!(harness.composer_text(cx).as_deref(), Some("hello?"));
}

#[gpui::test]
fn stop_cancels_the_running_turn(cx: &mut TestAppContext) {
    let (agent, mut requests) =
        FakeAgent::default().new_session_response(NewSessionResponse::new("alpha-session")).capture();
    let connector = FakeConnector::default().agent("/alpha", agent);
    let harness = DesktopHarnessBuilder::default().connector(connector).project("/alpha").build(cx);
    let thread = start_thread(&harness, cx, "project-alpha-new-thread");
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
fn each_thread_keeps_its_own_transcript_across_selection(cx: &mut TestAppContext) {
    let connector = FakeConnector::default()
        .agent("/alpha", replying("alpha-session", "from alpha"))
        .agent("/beta", replying("beta-session", "from beta"));
    let harness = DesktopHarnessBuilder::default().connector(connector).project("/alpha").project("/beta").build(cx);
    let alpha = start_thread(&harness, cx, "project-alpha-new-thread");
    send(&harness, cx, &alpha, "one");
    let beta = start_thread(&harness, cx, "project-beta-new-thread");
    assert!(harness.bounds(cx, "transcript-row-0").is_none(), "beta's transcript is empty");

    harness.click(cx, "thread-row-0").unwrap();
    assert!(harness.bounds(cx, "transcript-row-1").is_some(), "alpha's exchange is painted");

    harness.click(cx, "thread-row-1").unwrap();
    send(&harness, cx, &beta, "two");
    assert_eq!(alpha.read_with(cx, |thread, _| texts(thread)), ["one", "from alpha"]);
    assert_eq!(beta.read_with(cx, |thread, _| texts(thread)), ["two", "from beta"]);
}

#[gpui::test]
fn a_draft_stays_with_its_thread(cx: &mut TestAppContext) {
    let connector = FakeConnector::default()
        .agent("/alpha", replying("alpha-session", "from alpha"))
        .agent("/beta", replying("beta-session", "from beta"));
    let harness = DesktopHarnessBuilder::default().connector(connector).project("/alpha").project("/beta").build(cx);
    start_thread(&harness, cx, "project-alpha-new-thread");
    harness.simulate_input(cx, "alpha draft");
    start_thread(&harness, cx, "project-beta-new-thread");
    assert_eq!(harness.composer_text(cx).as_deref(), Some(""));

    harness.click(cx, "thread-row-0").unwrap();

    assert_eq!(harness.composer_text(cx).as_deref(), Some("alpha draft"));
}

/// Sends `prompt` through the composer and waits for the agent's reply.
fn send(harness: &DesktopHarness, cx: &mut TestAppContext, thread: &Entity<Thread>, prompt: &str) {
    let replied = thread.read_with(cx, |thread, _| texts(thread).len()) + 2;
    harness.simulate_input(cx, prompt);
    harness.simulate_keystrokes(cx, "enter");
    harness
        .run_until(cx, |cx| texts(thread.read(cx)).len() == replied && thread.read(cx).status() == ThreadStatus::Idle);
}
