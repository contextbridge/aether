use crate::support::{asked, asking_name, name_form, texts};
use acp_utils::testing::{FakeAgent, Question};
use aether_desktop::testing::DesktopHarness;
use aether_desktop::threads::{Thread, ThreadStatus};
use agent_client_protocol::schema::v2::{
    BooleanPropertySchema, CreateElicitationRequest, ElicitationFormMode, ElicitationSchema, ElicitationSessionScope,
    EnumOption, StringPropertySchema,
};
use gpui::{Entity, TestAppContext};
use std::path::PathBuf;
use utils::artifact_review::ArtifactReviewElicitationMeta;

#[gpui::test]
fn a_typed_answer_reaches_the_agent_once_every_required_field_is_filled(cx: &mut TestAppContext) {
    let (harness, thread) = asked(cx, asking_name());
    assert!(harness.bounds(cx, "elicitation").is_some(), "the question shows as a card");

    harness.click(cx, "elicitation-submit").unwrap();
    assert!(harness.bounds(cx, "elicitation-error-name").is_some(), "an empty required field is flagged");
    assert_eq!(thread.read_with(cx, |thread, _| thread.status()), ThreadStatus::NeedsInput);

    harness.click(cx, "elicitation-field-name").unwrap();
    harness.simulate_input(cx, "Ada");
    harness.click(cx, "elicitation-submit").unwrap();
    let answer = turn_end(&harness, &thread, cx);

    assert!(answer.contains(r#""name":"Ada""#), "{answer}");
    assert!(harness.bounds(cx, "elicitation").is_none(), "an answered question leaves");
}

#[gpui::test]
fn a_question_that_replaces_another_starts_with_a_fresh_answer(cx: &mut TestAppContext) {
    let (agent, mut requests) = FakeAgent::new("alpha-session").capture();
    let (harness, thread) = DesktopHarness::alpha(cx, agent);
    let turns = harness.start_turn(cx, &thread, &mut requests, "go");
    let _first = turns.ask(name_question());
    harness.run_until(cx, |cx| thread.read(cx).status() == ThreadStatus::NeedsInput);
    let first = thread.read_with(cx, |thread, _| thread.elicitation().map(|(id, _)| id));
    harness.click(cx, "elicitation-field-name").unwrap();
    harness.simulate_input(cx, "Ada");

    let _second = turns.ask(name_question());
    harness.run_until(cx, |cx| thread.read(cx).elicitation().map(|(id, _)| id) != first);
    harness.click(cx, "elicitation-submit").unwrap();

    assert!(harness.bounds(cx, "elicitation-error-name").is_some(), "the new question's field starts empty");
    assert_eq!(thread.read_with(cx, |thread, _| thread.status()), ThreadStatus::NeedsInput);
}

#[gpui::test]
fn chosen_options_reach_the_agent(cx: &mut TestAppContext) {
    let sizes = StringPropertySchema::new().one_of(vec![EnumOption::new("s", "Small"), EnumOption::new("l", "Large")]);
    let schema =
        ElicitationSchema::new().property("size", sizes, true).property("agree", BooleanPropertySchema::new(), true);
    let (harness, thread) = asked(cx, FakeAgent::new("alpha-session").prompt_asks("Pick", Question::Form(schema)));

    harness.click(cx, "elicitation-option-size-1").unwrap();
    harness.click(cx, "elicitation-field-agree").unwrap();
    harness.click(cx, "elicitation-submit").unwrap();
    let answer = turn_end(&harness, &thread, cx);

    assert!(answer.contains(r#""size":"l""#), "{answer}");
    assert!(answer.contains(r#""agree":true"#), "{answer}");
}

#[gpui::test]
fn declining_tells_the_agent_no(cx: &mut TestAppContext) {
    let (harness, thread) =
        asked(cx, FakeAgent::new("alpha-session").prompt_asks("Proceed?", Question::Form(ElicitationSchema::new())));

    harness.click(cx, "elicitation-decline").unwrap();
    let answer = turn_end(&harness, &thread, cx);

    assert!(answer.contains("decline"), "{answer}");
}

#[gpui::test]
fn a_link_question_opens_its_link_and_accepts(cx: &mut TestAppContext) {
    let url = Question::Url("https://example.com/login".into());
    let (harness, thread) = asked(cx, FakeAgent::new("alpha-session").prompt_asks("Sign in", url));

    harness.click(cx, "elicitation-open-url").unwrap();
    let answer = turn_end(&harness, &thread, cx);

    assert_eq!(cx.opened_url().as_deref(), Some("https://example.com/login"));
    assert!(answer.contains("accept"), "{answer}");
}

#[gpui::test]
fn an_artifact_review_opens_in_place_of_the_chat_and_approving_it_tells_the_agent(cx: &mut TestAppContext) {
    let (harness, thread) = asked(cx, reviewing_plan());
    assert!(harness.bounds(cx, "elicitation").is_some(), "the review is announced as a card");
    harness.click(cx, "elicitation-open-review").unwrap();
    assert!(harness.bounds(cx, "artifact-review").is_some(), "the artifact is shown for review");
    assert!(harness.bounds(cx, "transcript").is_none(), "the chat is hidden");

    harness.simulate_keystrokes(cx, "a");
    let answer = turn_end(&harness, &thread, cx);

    assert!(answer.contains(r#""decision":"approved""#), "{answer}");
    assert!(harness.bounds(cx, "transcript").is_some(), "the chat is back");
    assert!(harness.bounds(cx, "tab-review").is_none(), "an answered review leaves");
}

#[gpui::test]
fn comments_on_an_artifact_reach_the_agent_as_feedback(cx: &mut TestAppContext) {
    let (harness, thread) = asked(cx, reviewing_plan());
    harness.click(cx, "elicitation-open-review").unwrap();

    harness.simulate_keystrokes(cx, "c");
    harness.simulate_input(cx, "Too vague");
    harness.press_key(cx, "enter");
    harness.simulate_keystrokes(cx, "r");
    let answer = turn_end(&harness, &thread, cx);

    assert!(answer.contains(r#""decision":"feedback""#), "{answer}");
    assert!(answer.contains("Too vague"), "{answer}");
}

#[gpui::test]
fn cancelling_an_artifact_review_tells_the_agent_and_returns_to_the_chat(cx: &mut TestAppContext) {
    let (harness, thread) = asked(cx, reviewing_plan());
    harness.click(cx, "elicitation-open-review").unwrap();

    harness.simulate_keystrokes(cx, "escape");
    let answer = turn_end(&harness, &thread, cx);

    assert!(answer.contains("cancel"), "{answer}");
    assert!(harness.bounds(cx, "transcript").is_some(), "the chat is back");
}

fn name_question() -> CreateElicitationRequest {
    let form = ElicitationFormMode::new(ElicitationSessionScope::new("alpha-session"), name_form());
    CreateElicitationRequest::new(form, "Who are you?")
}

fn reviewing_plan() -> FakeAgent {
    let plan = ArtifactReviewElicitationMeta::new(
        Some(PathBuf::from("/alpha/plan.md")),
        "Review plan.md",
        "# Plan\n\nShip it.\n",
    );
    FakeAgent::new("alpha-session").prompt_asks("Review plan.md", Question::ArtifactReview(plan))
}

fn turn_end(harness: &DesktopHarness, thread: &Entity<Thread>, cx: &mut TestAppContext) -> String {
    harness.run_until(cx, |cx| texts(thread.read(cx)).len() == 2 && thread.read(cx).status() == ThreadStatus::Idle);
    thread.read_with(cx, |thread, _| texts(thread).pop().unwrap_or_default())
}
