use super::support::{
    CreateElicitationResponse, ElicitationSchema, accepted_content, assert_ctrl_c_exits, block_on_local,
    form_elicitation, make_app, with_elicitation,
};
use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use std::{
    path::PathBuf,
    sync::{Arc, Mutex},
};
use utils::artifact_review::ArtifactReviewElicitationMeta;
use wisp::{
    screens::artifact_review::ArtifactReviewScreen,
    surfaces::{elicitation::ElicitationResponder, input::UiEvent},
};

type Responses = Arc<Mutex<Vec<CreateElicitationResponse>>>;

fn screen(markdown: &str) -> (ArtifactReviewScreen, Responses) {
    let responses = Arc::new(Mutex::new(Vec::new()));
    let output = Arc::clone(&responses);
    let responder = ElicitationResponder::from_fn(move |response| output.lock().unwrap().push(response));
    (
        ArtifactReviewScreen::new(
            ArtifactReviewElicitationMeta::new(Some(PathBuf::from("/tmp/plan.md")), "Review /tmp/plan.md", markdown),
            responder,
        ),
        responses,
    )
}

fn key(code: KeyCode) -> UiEvent {
    UiEvent::Key(KeyEvent::new(code, KeyModifiers::NONE))
}
fn type_text(screen: &mut ArtifactReviewScreen, text: &str) {
    for character in text.chars() {
        screen.on_ui_event(key(KeyCode::Char(character)));
    }
}

#[test]
fn code_line_comments_preserve_source_context() {
    let (mut screen, responses) = screen("# Plan\n\n```rust\nfn first() {}\nfn last() {}\n```\n");
    screen.on_ui_event(key(KeyCode::Char('G')));
    screen.on_ui_event(key(KeyCode::Char('c')));
    type_text(&mut screen, "Rename this function");
    screen.on_ui_event(key(KeyCode::Enter));
    screen.on_ui_event(key(KeyCode::Char('r')));
    let responses = responses.lock().unwrap();
    let content = accepted_content(&responses[0]);
    assert_eq!(content["decision"], "feedback");
    let feedback = content["feedback"].as_str().unwrap();
    assert!(feedback.contains("Rename this function"), "{feedback}");
    assert!(feedback.contains("fn last()"), "{feedback}");
}

#[test]
fn approving_without_comments_requires_explicit_confirmation_and_answers_once() {
    let (mut screen, responses) = screen("# Plan\n\nbody");
    screen.on_ui_event(key(KeyCode::Char('r')));
    assert!(responses.lock().unwrap().is_empty());
    screen.on_ui_event(key(KeyCode::Enter));
    screen.on_ui_event(key(KeyCode::Enter));
    screen.cancel();
    drop(screen);
    let responses = responses.lock().unwrap();
    assert_eq!(responses.len(), 1);
    assert_eq!(accepted_content(&responses[0]), serde_json::json!({"decision": "approved"}));
}

#[test]
fn confirmation_escape_returns_to_review_without_answering() {
    let (mut screen, responses) = screen("# Plan\n\nbody");
    screen.on_ui_event(key(KeyCode::Char('r')));
    screen.on_ui_event(key(KeyCode::Esc));
    assert!(responses.lock().unwrap().is_empty());
    screen.on_ui_event(key(KeyCode::Char('c')));
    type_text(&mut screen, "Add tests");
    screen.on_ui_event(key(KeyCode::Enter));
    screen.on_ui_event(key(KeyCode::Char('r')));
    let responses = responses.lock().unwrap();
    let content = accepted_content(&responses[0]);
    assert_eq!(content["decision"], "feedback");
    assert!(content["feedback"].as_str().unwrap().contains("Add tests"));
}

#[test]
fn draft_and_help_escape_do_not_cancel_review() {
    let (mut screen, responses) = screen("# Plan\n\nbody");
    screen.on_ui_event(key(KeyCode::Char('?')));
    screen.on_ui_event(key(KeyCode::Esc));
    assert!(responses.lock().unwrap().is_empty());
    screen.on_ui_event(key(KeyCode::Char('c')));
    type_text(&mut screen, "discard me");
    screen.on_ui_event(key(KeyCode::Esc));
    assert!(responses.lock().unwrap().is_empty());
    screen.on_ui_event(key(KeyCode::Char('r')));
    assert!(responses.lock().unwrap().is_empty());
    screen.on_ui_event(key(KeyCode::Enter));
    let responses = responses.lock().unwrap();
    assert_eq!(accepted_content(&responses[0]), serde_json::json!({"decision": "approved"}));
}

#[test]
fn modified_and_released_submission_keys_do_not_submit() {
    let (mut screen, responses) = screen("# Plan\n\nbody");
    for modifiers in
        [KeyModifiers::CONTROL, KeyModifiers::ALT, KeyModifiers::SUPER, KeyModifiers::HYPER, KeyModifiers::META]
    {
        screen.on_ui_event(UiEvent::Key(KeyEvent::new(KeyCode::Char('a'), modifiers)));
        screen.on_ui_event(UiEvent::Key(KeyEvent::new(KeyCode::Char('r'), modifiers)));
    }
    screen.on_ui_event(UiEvent::Key(KeyEvent::new_with_kind(
        KeyCode::Char('r'),
        KeyModifiers::NONE,
        KeyEventKind::Release,
    )));
    screen.on_ui_event(key(KeyCode::Enter));
    assert!(responses.lock().unwrap().is_empty());
}

#[test]
fn confirmation_ignores_modified_released_repeated_and_widget_keys() {
    let (mut screen, responses) = screen("# Plan\n\nbody");
    screen.on_ui_event(key(KeyCode::Char('r')));
    for modifiers in [
        KeyModifiers::SHIFT,
        KeyModifiers::CONTROL,
        KeyModifiers::ALT,
        KeyModifiers::SUPER,
        KeyModifiers::HYPER,
        KeyModifiers::META,
    ] {
        for code in [KeyCode::Enter, KeyCode::Esc] {
            screen.on_ui_event(UiEvent::Key(KeyEvent::new(code, modifiers)));
        }
    }
    for kind in [KeyEventKind::Release, KeyEventKind::Repeat] {
        screen.on_ui_event(UiEvent::Key(KeyEvent::new_with_kind(KeyCode::Enter, KeyModifiers::NONE, kind)));
    }
    screen.on_ui_event(key(KeyCode::Char('c')));
    screen.on_ui_event(UiEvent::Paste("do not add a comment".into()));
    assert!(responses.lock().unwrap().is_empty());
    screen.on_ui_event(key(KeyCode::Enter));
    assert_eq!(accepted_content(&responses.lock().unwrap()[0]), serde_json::json!({"decision": "approved"}));
}

#[test]
fn dropping_confirmation_cancels_once() {
    let (mut screen, responses) = screen("# Plan");
    screen.on_ui_event(key(KeyCode::Char('r')));
    drop(screen);
    let responses = responses.lock().unwrap();
    assert_eq!(responses.len(), 1);
    assert_eq!(serde_json::to_value(&responses[0]).unwrap()["action"], "cancel");
}

#[test]
fn approval_closes_the_route_and_notifies_the_user() {
    block_on_local(async {
        let mut app = make_app();
        let meta =
            ArtifactReviewElicitationMeta::new(Some(PathBuf::from("/tmp/plan.md")), "Review /tmp/plan.md", "# Plan")
                .to_json()
                .unwrap();
        with_elicitation(&mut app, form_elicitation("review", "Review artifact", ElicitationSchema::new()).meta(meta))
            .await;
        app.key(KeyCode::Char('r').into());
        assert!(app.app().full_screen_active());
        assert!(app.viewport_text().contains("Approve and continue"));
        app.key(KeyCode::Enter.into());
        assert!(!app.app().full_screen_active());
        assert!(app.viewport_text().contains("Artifact approved"));
    });
}

#[test]
fn double_ctrl_c_exits_over_artifact_review() {
    block_on_local(async {
        let mut app = make_app();
        let meta = ArtifactReviewElicitationMeta::new(
            Some(PathBuf::from("/tmp/plan.md")),
            "Review /tmp/plan.md",
            "# Plan\nbody",
        )
        .to_json()
        .unwrap();
        with_elicitation(&mut app, form_elicitation("review", "Review artifact", ElicitationSchema::new()).meta(meta))
            .await;
        assert!(app.app().full_screen_active());
        assert_ctrl_c_exits(&mut app);
    });
}
