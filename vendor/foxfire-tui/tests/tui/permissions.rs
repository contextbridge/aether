use super::support::*;
use agent_client_protocol::v2::schema::{
    PermissionOption, PermissionOptionKind, RequestPermissionOutcome, RequestPermissionRequest,
    SelectedPermissionOutcome,
};

#[test]
fn a_permission_request_offers_the_agents_choices_and_sends_the_one_picked() {
    let mut ui = TestUi::with_dimensions(80, 24);
    let reply = ui.elicit(run_rm());
    assert!(ui.app().has_modal());

    ui.draw();
    let screen = buffer_text(ui.backend().buffer());
    for shown in ["Run rm -rf build?", "Allow once", "Reject"] {
        assert!(screen.contains(shown), "{shown} is missing from:\n{screen}");
    }

    ui.key(key(KeyCode::Down));
    ui.key(key(KeyCode::Enter));

    let response = ui.answer_to(reply);
    assert_eq!(response.outcome, RequestPermissionOutcome::Selected(SelectedPermissionOutcome::new("reject")));
    assert!(!ui.app().has_modal());
}

#[test]
fn escape_cancels_a_permission_request() {
    let mut ui = TestUi::new();
    let reply = ui.elicit(run_rm());

    ui.key(key(KeyCode::Esc));
    ui.key(key(KeyCode::Char('y')));

    assert!(matches!(ui.answer_to(reply).outcome, RequestPermissionOutcome::Cancelled(_)));
}

fn run_rm() -> RequestPermissionRequest {
    let options = vec![
        PermissionOption::new("allow", "Allow once", PermissionOptionKind::AllowOnce),
        PermissionOption::new("reject", "Reject", PermissionOptionKind::RejectOnce),
    ];
    RequestPermissionRequest::new("test-session", "Run rm -rf build?", options)
}
