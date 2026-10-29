use super::support::*;
use crossterm::event::KeyEventKind;

#[test]
fn code_line_comments_preserve_source_context() {
    let mut ui = TestUi::new();
    let response = open_plan_review(&mut ui, "# Plan\n\n```rust\nfn first() {}\nfn last() {}\n```\n");

    ui.key(key(KeyCode::Char('G')));
    ui.key(key(KeyCode::Char('c')));
    ui.type_text("Rename this function");
    ui.key(key(KeyCode::Enter));
    ui.key(key(KeyCode::Char('r')));

    let content = accepted_content(&ui.answer_to(response));
    assert_eq!(content["decision"], "feedback");
    let feedback = content["feedback"].as_str().unwrap();
    assert!(feedback.contains("Rename this function"), "{feedback}");
    assert!(feedback.contains("fn last()"), "{feedback}");
}

#[test]
fn approving_without_comments_asks_for_confirmation_then_closes_the_review() {
    let mut ui = TestUi::new();
    let response = open_plan_review(&mut ui, "# Plan\n\nbody");

    ui.key(key(KeyCode::Char('r')));
    assert!(ui.app().full_screen_active(), "r only asks for confirmation");
    ui.assert_viewport_contains("Approve and continue");
    ui.key(key(KeyCode::Enter));

    assert_eq!(accepted_content(&ui.answer_to(response)), serde_json::json!({"decision": "approved"}));
    assert!(!ui.app().full_screen_active());
    ui.assert_viewport_contains("Artifact approved");
}

#[test]
fn confirmation_escape_returns_to_review_without_answering() {
    let mut ui = TestUi::new();
    let response = open_plan_review(&mut ui, "# Plan\n\nbody");

    ui.key(key(KeyCode::Char('r')));
    ui.key(key(KeyCode::Esc));
    assert!(ui.app().full_screen_active(), "Esc backs out of the confirmation, not the review");
    ui.key(key(KeyCode::Char('c')));
    ui.type_text("Add tests");
    ui.key(key(KeyCode::Enter));
    ui.key(key(KeyCode::Char('r')));

    let content = accepted_content(&ui.answer_to(response));
    assert_eq!(content["decision"], "feedback");
    assert!(content["feedback"].as_str().unwrap().contains("Add tests"));
}

#[test]
fn draft_and_help_escape_do_not_cancel_review() {
    let mut ui = TestUi::new();
    let response = open_plan_review(&mut ui, "# Plan\n\nbody");

    ui.key(key(KeyCode::Char('?')));
    ui.key(key(KeyCode::Esc));
    assert!(ui.app().full_screen_active(), "Esc closes help, not the review");
    ui.key(key(KeyCode::Char('c')));
    ui.type_text("discard me");
    ui.key(key(KeyCode::Esc));
    assert!(ui.app().full_screen_active(), "Esc discards the draft, not the review");
    ui.key(key(KeyCode::Char('r')));
    ui.key(key(KeyCode::Enter));

    assert_eq!(accepted_content(&ui.answer_to(response)), serde_json::json!({"decision": "approved"}));
}

#[test]
fn modified_and_released_submission_keys_do_not_submit() {
    let mut ui = TestUi::new();
    let _response = open_plan_review(&mut ui, "# Plan\n\nbody");

    for modifiers in
        [KeyModifiers::CONTROL, KeyModifiers::ALT, KeyModifiers::SUPER, KeyModifiers::HYPER, KeyModifiers::META]
    {
        ui.key(KeyEvent::new(KeyCode::Char('a'), modifiers));
        ui.key(KeyEvent::new(KeyCode::Char('r'), modifiers));
    }
    ui.key(KeyEvent::new_with_kind(KeyCode::Char('r'), KeyModifiers::NONE, KeyEventKind::Release));
    ui.key(key(KeyCode::Enter));

    assert!(ui.app().full_screen_active(), "no key submitted the review");
}

#[test]
fn confirmation_ignores_modified_and_widget_keys() {
    let mut ui = TestUi::new();
    let response = open_plan_review(&mut ui, "# Plan\n\nbody");
    ui.key(key(KeyCode::Char('r')));

    for modifiers in [
        KeyModifiers::SHIFT,
        KeyModifiers::CONTROL,
        KeyModifiers::ALT,
        KeyModifiers::SUPER,
        KeyModifiers::HYPER,
        KeyModifiers::META,
    ] {
        for code in [KeyCode::Enter, KeyCode::Esc] {
            ui.key(KeyEvent::new(code, modifiers));
        }
    }
    ui.key(key(KeyCode::Char('c')));
    ui.paste("do not add a comment");
    assert!(ui.app().full_screen_active(), "only a plain Enter confirms");
    ui.key(key(KeyCode::Enter));

    assert_eq!(accepted_content(&ui.answer_to(response)), serde_json::json!({"decision": "approved"}));
}

#[test]
fn closing_the_app_during_confirmation_cancels_the_review() {
    let mut ui = TestUi::new();
    let response = open_plan_review(&mut ui, "# Plan");
    ui.key(key(KeyCode::Char('r')));

    let answer = ui.quit_answering(response);

    assert_eq!(serde_json::to_value(answer).unwrap()["action"], "cancel");
}

#[test]
fn double_ctrl_c_exits_over_artifact_review() {
    let mut ui = TestUi::new();
    let _response = open_plan_review(&mut ui, "# Plan\nbody");
    assert!(ui.app().full_screen_active());
    assert_ctrl_c_exits(&mut ui);
}

fn open_plan_review(ui: &mut TestUi, markdown: &str) -> ElicitationReply {
    let request =
        form_elicitation("review", "Review artifact", ElicitationSchema::new()).meta(plan_meta(markdown).to_meta());
    ui.elicit(request)
}
