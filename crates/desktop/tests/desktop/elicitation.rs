use crate::support::texts;
use acp_utils::testing::FakeAgent;
use aether_desktop::testing::{DesktopHarness, DesktopHarnessBuilder, FakeConnector};
use aether_desktop::threads::{Thread, ThreadStatus};
use agent_client_protocol::schema::v2::{
    BooleanPropertySchema, ElicitationSchema, EnumOption, NewSessionResponse, StringPropertySchema,
};
use gpui::{Entity, TestAppContext};

#[gpui::test]
fn a_typed_answer_reaches_the_agent_and_its_turn_goes_on(cx: &mut TestAppContext) {
    let schema = ElicitationSchema::new().property("name", StringPropertySchema::new().title("Name"), true);
    let (harness, thread) = questioned(agent().prompt_form("Who are you?", schema), cx);
    assert!(harness.bounds(cx, "elicitation").is_some(), "the question shows as a card");

    harness.click(cx, "elicitation-field-name").unwrap();
    harness.simulate_input(cx, "Ada");
    harness.click(cx, "elicitation-submit").unwrap();
    let answer = turn_end(&harness, &thread, cx);

    assert!(answer.contains(r#""name":"Ada""#), "{answer}");
    assert!(harness.bounds(cx, "elicitation").is_none(), "an answered question leaves");
}

#[gpui::test]
fn a_required_field_left_empty_is_flagged_and_the_agent_keeps_waiting(cx: &mut TestAppContext) {
    let schema = ElicitationSchema::new().property("name", StringPropertySchema::new().title("Name"), true);
    let (harness, thread) = questioned(agent().prompt_form("Who are you?", schema), cx);

    harness.click(cx, "elicitation-submit").unwrap();

    assert!(harness.bounds(cx, "elicitation-error-name").is_some());
    assert_eq!(thread.read_with(cx, |thread, _| thread.status()), ThreadStatus::NeedsInput);
}

#[gpui::test]
fn chosen_options_reach_the_agent(cx: &mut TestAppContext) {
    let sizes = StringPropertySchema::new().one_of(vec![EnumOption::new("s", "Small"), EnumOption::new("l", "Large")]);
    let schema =
        ElicitationSchema::new().property("size", sizes, true).property("agree", BooleanPropertySchema::new(), true);
    let (harness, thread) = questioned(agent().prompt_form("Pick", schema), cx);

    harness.click(cx, "elicitation-option-size-1").unwrap();
    harness.click(cx, "elicitation-field-agree").unwrap();
    harness.click(cx, "elicitation-submit").unwrap();
    let answer = turn_end(&harness, &thread, cx);

    assert!(answer.contains(r#""size":"l""#), "{answer}");
    assert!(answer.contains(r#""agree":true"#), "{answer}");
}

#[gpui::test]
fn declining_tells_the_agent_no(cx: &mut TestAppContext) {
    let (harness, thread) = questioned(agent().prompt_elicitation("Proceed?"), cx);

    harness.click(cx, "elicitation-decline").unwrap();
    let answer = turn_end(&harness, &thread, cx);

    assert!(answer.contains("decline"), "{answer}");
}

#[gpui::test]
fn a_link_question_opens_its_link_and_accepts(cx: &mut TestAppContext) {
    let (harness, thread) = questioned(agent().prompt_url("Sign in", "https://example.com/login"), cx);

    harness.click(cx, "elicitation-open-url").unwrap();
    let answer = turn_end(&harness, &thread, cx);

    assert_eq!(cx.opened_url().as_deref(), Some("https://example.com/login"));
    assert!(answer.contains("accept"), "{answer}");
}

fn agent() -> FakeAgent {
    FakeAgent::default().new_session_response(NewSessionResponse::new("alpha-session"))
}

/// Opens a thread talking to `agent`, prompts it, and waits until it asks its question.
fn questioned(agent: FakeAgent, cx: &mut TestAppContext) -> (DesktopHarness, Entity<Thread>) {
    let harness = DesktopHarnessBuilder::default().connector(FakeConnector::default().agent("/alpha", agent)).build(cx);
    let thread = harness.threads(cx).update(cx, |threads, cx| threads.new_thread("/alpha".into(), cx));
    harness.run_until(cx, |cx| thread.read(cx).status() == ThreadStatus::Idle);
    thread.update(cx, |thread, cx| thread.submit("go", cx));
    harness.run_until(cx, |cx| thread.read(cx).status() == ThreadStatus::NeedsInput);
    (harness, thread)
}

/// Waits for the agent to finish its turn, and returns its last message: the answer it received.
fn turn_end(harness: &DesktopHarness, thread: &Entity<Thread>, cx: &mut TestAppContext) -> String {
    harness.run_until(cx, |cx| texts(thread.read(cx)).len() == 2 && thread.read(cx).status() == ThreadStatus::Idle);
    thread.read_with(cx, |thread, _| texts(thread).pop().unwrap_or_default())
}
