use acp_utils::testing::FakeAgent;
use agent_client_protocol::schema::v2::PromptRequest;
use client_core::testing::SessionCoreConfigBuilder;
use wisp::error::AppError;
use wisp::testing::TestUiBuilder;

#[test]
fn remote_connection_loss_exits_with_error_without_resending() {
    let mut ui = TestUiBuilder::new()
        .session(SessionCoreConfigBuilder::remote_workspace)
        .agent(FakeAgent::hold::<PromptRequest>)
        .build();
    ui.submit("acceptance is uncertain");
    let _unanswered = ui.held::<PromptRequest>();

    ui.disconnect_agent();

    let error = ui.app().exit_result().unwrap().unwrap_err();
    assert!(matches!(error, AppError::ConnectionLost));
    assert!(error.to_string().contains("agent may still be running"));
    assert!(error.to_string().contains("before resubmitting"));
    assert!(!ui.app().has_modal());
    assert!(!ui.app().needs_mouse_capture());
    assert_eq!(std::iter::from_fn(|| ui.agent().prompt.try_recv().ok()).count(), 1, "the prompt is sent once");
    assert!(ui.agent().cancel.try_recv().is_err());
    assert!(ui.agent().close_session.try_recv().is_err());
}
