//! Most agents speak only standard ACP. Foxfire must drive one without touching Aether's extensions.

use super::support::*;
use agent_client_protocol::v2::schema::{
    PermissionOption, PermissionOptionKind, RequestPermissionOutcome, RequestPermissionRequest,
    SelectedPermissionOutcome,
};

#[test]
fn a_plain_acp_agent_drives_foxfire_without_any_aether_extension() {
    let model = select("llm", "LLM", "fast", &[("fast", "Fast"), ("slow", "Slow")])
        .category(acp::SessionConfigOptionCategory::Model);
    let mut ui = TestUiBuilder::new()
        .plain_acp()
        .session(|session| session.config_options(vec![model]))
        .dimensions(100, 24)
        .build();
    ui.assert_viewport_contains("Fast");

    ui.submit("run the tests");
    let options = vec![
        PermissionOption::new("allow", "Allow once", PermissionOptionKind::AllowOnce),
        PermissionOption::new("reject", "Reject", PermissionOptionKind::RejectOnce),
    ];
    let reply = ui.elicit(RequestPermissionRequest::new("test-session", "Run the tests?", options));
    ui.key(key(KeyCode::Enter));
    let allowed = RequestPermissionOutcome::Selected(SelectedPermissionOutcome::new("allow"));
    assert_eq!(ui.answer_to(reply).outcome, allowed);
    ui.acp_event(text_chunk("All green."));
    ui.complete_prompt(acp::StopReason::EndTurn);

    ui.assert_viewport_contains("All green.");
    assert_eq!(ui.agent().extensions.try_recv().ok(), None, "no Aether extension is asked for");
}
