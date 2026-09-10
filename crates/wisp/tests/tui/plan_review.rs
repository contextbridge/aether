use super::support::{
    CreateElicitationResponse, ElicitationAction, ElicitationSchema, accepted_content, assert_ctrl_c_exits,
    block_on_local, form_elicitation, make_app, with_elicitation,
};
use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::{buffer::Buffer, layout::Rect};
use std::{
    path::PathBuf,
    sync::{Arc, Mutex},
};
use utils::plan_review::PlanReviewElicitationMeta;
use wisp::testing::buffer_text;
use wisp::{
    renderer::DrawContext,
    screens::plan_review::PlanReviewScreen,
    surfaces::{elicitation::ElicitationResponder, input::PlanReviewOutput},
    theme::Theme,
    view::{generation::Generation, syntax::SyntaxHighlighter},
};

type Responses = Arc<Mutex<Vec<CreateElicitationResponse>>>;

fn screen(markdown: &str) -> (PlanReviewScreen, Responses) {
    let responses = Arc::new(Mutex::new(Vec::new()));
    let output = Arc::clone(&responses);
    let responder = ElicitationResponder::from_fn(move |response| output.lock().unwrap().push(response));
    (
        PlanReviewScreen::new(PlanReviewElicitationMeta::new(&PathBuf::from("/tmp/plan.md"), markdown), responder),
        responses,
    )
}

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}
fn type_text(screen: &mut PlanReviewScreen, text: &str) {
    for character in text.chars() {
        screen.on_key(key(KeyCode::Char(character)));
    }
}
fn render(screen: &mut PlanReviewScreen, width: u16, height: u16) -> Buffer {
    let area = Rect::new(3, 2, width, height);
    let mut buffer = Buffer::empty(area);
    let theme = Theme::default();
    let mut highlighter = SyntaxHighlighter::new();
    screen.render(
        area,
        &mut buffer,
        &mut DrawContext { theme: &theme, highlighter: &mut highlighter, theme_generation: Generation::default() },
    );
    buffer
}

#[test]
fn decisions_preserve_acp_payload_and_resolve_exactly_once() {
    for (binding, decision) in [('a', "approve"), ('r', "deny")] {
        let (mut screen, responses) = screen("# Plan\n\nImplement this.");
        assert!(
            screen
                .on_key(key(KeyCode::Char(binding)))
                .iter()
                .any(|output| matches!(output, PlanReviewOutput::Outcome(_)))
        );
        screen.on_key(key(KeyCode::Char(binding)));
        screen.cancel();
        drop(screen);
        let responses = responses.lock().unwrap();
        assert_eq!(responses.len(), 1);
        let content = accepted_content(&responses[0]);
        assert_eq!(content["decision"], decision);
        assert!(content["feedback"].is_string());
    }
}

#[test]
fn cancellation_and_route_destruction_resolve_exactly_once() {
    for explicit in [false, true] {
        let (mut screen, responses) = screen("# Plan\n\nbody");
        if explicit {
            screen.cancel();
            screen.cancel();
        }
        drop(screen);
        let responses = responses.lock().unwrap();
        assert_eq!(responses.len(), 1);
        assert!(matches!(responses[0].action, ElicitationAction::Cancel));
    }
}

#[test]
fn semantic_comment_submission_retains_path_source_and_heading_context() {
    let (mut screen, responses) = screen("# Plan\n\nBroken paragraph.\n\n```rust\nfn main() {}\n```\n");
    screen.on_key(key(KeyCode::Char('j')));
    screen.on_key(key(KeyCode::Char('c')));
    type_text(&mut screen, "Clarify this paragraph");
    screen.on_key(key(KeyCode::Enter));
    screen.on_key(key(KeyCode::Char('r')));
    let responses = responses.lock().unwrap();
    assert_eq!(responses.len(), 1);
    let content = accepted_content(&responses[0]);
    assert_eq!(content["decision"], "deny");
    let feedback = content["feedback"].as_str().unwrap();
    for expected in ["Clarify this paragraph", "plan.md", "Broken paragraph", "Plan"] {
        assert!(feedback.contains(expected), "missing {expected}: {feedback}");
    }
}

#[test]
fn code_line_comments_preserve_source_context() {
    let (mut screen, responses) = screen("# Plan\n\n```rust\nfn first() {}\nfn last() {}\n```\n");
    screen.on_key(key(KeyCode::Char('G')));
    screen.on_key(key(KeyCode::Char('c')));
    type_text(&mut screen, "Rename this function");
    screen.on_key(key(KeyCode::Enter));
    screen.on_key(key(KeyCode::Char('r')));
    let responses = responses.lock().unwrap();
    let content = accepted_content(&responses[0]);
    let feedback = content["feedback"].as_str().unwrap();
    assert!(feedback.contains("Rename this function"), "{feedback}");
    assert!(feedback.contains("fn last()"), "{feedback}");
}

#[test]
fn draft_and_help_escape_do_not_cancel_review() {
    let (mut screen, responses) = screen("# Plan\n\nbody");
    screen.on_key(key(KeyCode::Char('?')));
    screen.on_key(key(KeyCode::Esc));
    assert!(responses.lock().unwrap().is_empty());
    screen.on_key(key(KeyCode::Char('c')));
    type_text(&mut screen, "discard me");
    screen.on_key(key(KeyCode::Esc));
    assert!(responses.lock().unwrap().is_empty());
    screen.on_key(key(KeyCode::Char('r')));
    let responses = responses.lock().unwrap();
    assert!(!accepted_content(&responses[0])["feedback"].as_str().unwrap().contains("discard me"));
}

#[test]
fn modified_and_released_decision_keys_do_not_submit() {
    let (mut screen, responses) = screen("# Plan\n\nbody");
    for modifiers in
        [KeyModifiers::CONTROL, KeyModifiers::ALT, KeyModifiers::SUPER, KeyModifiers::HYPER, KeyModifiers::META]
    {
        screen.on_key(KeyEvent::new(KeyCode::Char('a'), modifiers));
        screen.on_key(KeyEvent::new(KeyCode::Char('r'), modifiers));
    }
    screen.on_key(KeyEvent::new_with_kind(KeyCode::Char('a'), KeyModifiers::NONE, KeyEventKind::Release));
    assert!(responses.lock().unwrap().is_empty());
}

#[test]
fn widget_paints_each_host_buffer_and_renders_tables() {
    let (mut screen, _responses) = screen("# Plan\n\n| Task | Status |\n| --- | --- |\n| Implement | Ready |\n");
    for width in [35, 100] {
        let first = render(&mut screen, width, 24);
        let second = render(&mut screen, width, 24);
        assert_eq!(first, second);
        let text = buffer_text(&first);
        assert!(text.contains("Implement"), "{text}");
        assert!(text.contains("Ready"), "{text}");
    }
}

#[test]
fn theme_picker_returns_a_stable_selection_without_resolving_review() {
    let (mut screen, responses) = screen("# Plan");
    render(&mut screen, 100, 24);
    screen.on_key(key(KeyCode::Char('t')));
    render(&mut screen, 100, 24);
    let outputs = screen.on_key(key(KeyCode::Enter));
    assert!(
        outputs.iter().any(|output| matches!(output, PlanReviewOutput::SetTheme(id) if id == "builtin:sage")),
        "{outputs:?}"
    );
    assert!(responses.lock().unwrap().is_empty());
}

#[test]
fn double_ctrl_c_exits_over_plan_review() {
    block_on_local(async {
        let mut app = make_app();
        let meta = PlanReviewElicitationMeta::new(&PathBuf::from("/tmp/plan.md"), "# Plan\nbody").to_json().unwrap();
        with_elicitation(&mut app, form_elicitation("plan", "Approve plan?", ElicitationSchema::new()).meta(meta))
            .await;
        assert!(app.app().full_screen_active());
        assert_ctrl_c_exits(&mut app);
    });
}
