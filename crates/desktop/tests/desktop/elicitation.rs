use crate::support::{asked, asking_name, name_form, texts};
use acp_utils::testing::{FakeAgent, Question};
use aether_desktop::testing::DesktopHarness;
use aether_desktop::threads::{Thread, ThreadStatus};
use agent_client_protocol::v2::schema::{
    BooleanPropertySchema, CreateElicitationRequest, ElicitationFormMode, ElicitationSchema, ElicitationSessionScope,
    EnumOption, StringPropertySchema,
};
use std::path::PathBuf;
use utils::artifact_review::ArtifactReviewElicitationMeta;

#[test]
fn a_typed_answer_reaches_the_agent_once_every_required_field_is_filled() {
    let (mut harness, thread) = asked(asking_name());
    assert!(harness.bounds("elicitation").is_some(), "the question shows as a card");

    harness.click("elicitation-submit").unwrap();
    assert!(harness.bounds("elicitation-error-name").is_some(), "an empty required field is flagged");
    assert_eq!(thread.status(), ThreadStatus::NeedsInput);

    harness.click("elicitation-field-name").unwrap();
    harness.type_text("Ada");
    harness.click("elicitation-submit").unwrap();
    let answer = turn_end(&mut harness, thread);

    assert!(answer.contains(r#""name":"Ada""#), "{answer}");
    assert!(harness.bounds("elicitation").is_none(), "an answered question leaves");
}

#[test]
fn a_question_that_replaces_another_starts_with_a_fresh_answer() {
    let (agent, mut requests) = FakeAgent::new("alpha-session").capture();
    let (mut harness, thread) = DesktopHarness::alpha(agent);
    let turns = harness.start_turn(thread, &mut requests, "go");
    let _first = turns.ask(name_question());
    harness.run_until(|_| thread.status() == ThreadStatus::NeedsInput);
    let first = thread.elicitation().map(|(id, _)| id);
    harness.click("elicitation-field-name").unwrap();
    harness.type_text("Ada");

    let _second = turns.ask(name_question());
    harness.run_until(|_| thread.elicitation().map(|(id, _)| id) != first);
    harness.click("elicitation-submit").unwrap();

    assert!(harness.bounds("elicitation-error-name").is_some(), "the new question's field starts empty");
    assert_eq!(thread.status(), ThreadStatus::NeedsInput);
}

#[test]
fn chosen_options_reach_the_agent() {
    let sizes = StringPropertySchema::new().one_of(vec![EnumOption::new("s", "Small"), EnumOption::new("l", "Large")]);
    let schema =
        ElicitationSchema::new().property("size", sizes, true).property("agree", BooleanPropertySchema::new(), true);
    let (mut harness, thread) = asked(FakeAgent::new("alpha-session").prompt_asks("Pick", Question::Form(schema)));

    harness.click("elicitation-option-size-1").unwrap();
    harness.click("elicitation-field-agree").unwrap();
    harness.click("elicitation-submit").unwrap();
    let answer = turn_end(&mut harness, thread);

    assert!(answer.contains(r#""size":"l""#), "{answer}");
    assert!(answer.contains(r#""agree":true"#), "{answer}");
}

#[test]
fn declining_tells_the_agent_no() {
    let (mut harness, thread) =
        asked(FakeAgent::new("alpha-session").prompt_asks("Proceed?", Question::Form(ElicitationSchema::new())));

    harness.click("elicitation-decline").unwrap();
    let answer = turn_end(&mut harness, thread);

    assert!(answer.contains("decline"), "{answer}");
}

#[test]
fn a_link_question_opens_its_link_and_accepts() {
    let url = Question::Url("https://example.com/login".into());
    let (mut harness, thread) = asked(FakeAgent::new("alpha-session").prompt_asks("Sign in", url));

    harness.click("elicitation-open-url").unwrap();
    let answer = turn_end(&mut harness, thread);

    assert_eq!(harness.platform().opened(), ["https://example.com/login"]);
    assert!(answer.contains("accept"), "{answer}");
}

#[test]
fn an_artifact_review_opens_in_place_of_the_chat_and_approving_it_tells_the_agent() {
    let (mut harness, thread) = asked(reviewing_plan());
    assert!(harness.bounds("elicitation").is_some(), "the review is announced as a card");
    harness.click("elicitation-open-review").unwrap();
    assert!(harness.bounds("artifact-review").is_some(), "the artifact is shown for review");
    assert!(harness.bounds("transcript").is_none(), "the chat is hidden");

    harness.press("a");
    let answer = turn_end(&mut harness, thread);

    assert!(answer.contains(r#""decision":"approved""#), "{answer}");
    assert!(harness.bounds("transcript").is_some(), "the chat is back");
    assert!(harness.bounds("tab-review").is_none(), "an answered review leaves");
}

#[test]
fn comments_on_an_artifact_reach_the_agent_as_feedback() {
    let (mut harness, thread) = asked(reviewing_plan());
    harness.click("elicitation-open-review").unwrap();

    harness.press("c");
    harness.type_text("Too vague");
    harness.press("enter");
    harness.press("r");
    let answer = turn_end(&mut harness, thread);

    assert!(answer.contains(r#""decision":"feedback""#), "{answer}");
    assert!(answer.contains("Too vague"), "{answer}");
}

#[test]
fn cancelling_an_artifact_review_tells_the_agent_and_returns_to_the_chat() {
    let (mut harness, thread) = asked(reviewing_plan());
    harness.click("elicitation-open-review").unwrap();

    harness.press("escape");
    let answer = turn_end(&mut harness, thread);

    assert!(answer.contains("cancel"), "{answer}");
    assert!(harness.bounds("transcript").is_some(), "the chat is back");
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

fn turn_end(harness: &mut DesktopHarness, thread: Thread) -> String {
    harness.run_until(|_| texts(thread).len() == 2 && thread.status() == ThreadStatus::Idle);
    texts(thread).pop().unwrap_or_default()
}
