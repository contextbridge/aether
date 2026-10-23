use crate::support::{replying, texts};
use acp_utils::testing::FakeAgent;
use aether_desktop::testing::DesktopHarnessBuilder;
use aether_desktop::threads::ThreadStatus;
use agent_client_protocol::schema::v2::NewSessionResponse;
use gpui::TestAppContext;

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
