use super::support::*;

#[test]
fn elicitation_request_is_accepted_interactively() {
    let mut app = TestUi::new();

    let response_rx = app.elicit(form_elicitation("test-server", "Confirm the action", ElicitationSchema::new()));
    assert!(app.app().has_modal());

    app.key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

    let response = app.answer_to(response_rx);
    assert_eq!(accepted_content(&response), serde_json::json!({}));
    assert!(!app.app().has_modal());
}

#[test]
fn tab_cycles_reasoning_effort_through_advertised_levels() {
    for (current, levels, expected_values) in [
        ("default", ["default", "disabled", "low"], ["disabled", "low", "default", "disabled"]),
        ("low", ["low", "medium", "high"], ["medium", "high", "default", "low"]),
    ] {
        let mut app = TestUiBuilder::new()
            .session(|session| session.config_options(vec![reasoning_option(current, &levels)]))
            .build();
        for expected in expected_values {
            app.key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
            assert_sets_config_option(&mut app, "reasoning_effort", expected);
        }
    }
}

#[test]
fn shift_backtab_cycles_mode_option_and_wraps() {
    let options = vec![mode_option("code", &["code", "plan", "ask"])];
    let mut app = TestUiBuilder::new().session(|session| session.config_options(options)).build();

    app.key(KeyEvent::new(KeyCode::BackTab, KeyModifiers::SHIFT));
    assert_sets_config_option(&mut app, "mode", "plan");

    app.key(KeyEvent::new(KeyCode::BackTab, KeyModifiers::SHIFT));
    assert_sets_config_option(&mut app, "mode", "ask");

    app.key(KeyEvent::new(KeyCode::BackTab, KeyModifiers::SHIFT));
    assert_sets_config_option(&mut app, "mode", "code");
}

#[test]
fn shift_backtab_cycles_grouped_mode_options() {
    let options = vec![grouped_mode_option("code", &[("built-in", &["code", "plan"]), ("custom", &["review"])])];
    let mut app = TestUiBuilder::new().session(|session| session.config_options(options)).build();

    app.key(KeyEvent::new(KeyCode::BackTab, KeyModifiers::SHIFT));
    assert_sets_config_option(&mut app, "mode", "plan");

    app.key(KeyEvent::new(KeyCode::BackTab, KeyModifiers::SHIFT));
    assert_sets_config_option(&mut app, "mode", "review");
}

#[test]
fn tab_and_backtab_noop_without_cycleable_options() {
    let mut app = TestUi::new();

    app.key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
    app.key(KeyEvent::new(KeyCode::BackTab, KeyModifiers::SHIFT));

    assert!(app.agent().config.try_recv().is_err());
}

#[test]
fn command_picker_consumes_tab_without_changing_config() {
    let options = vec![reasoning_option("low", &["low", "medium", "high"])];
    let mut app = TestUiBuilder::new().session(|session| session.config_options(options)).build();

    app.key(key(KeyCode::Char('/')));
    assert!(app.app().composer().has_completion());

    app.key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));

    assert!(!app.app().composer().has_completion());
}

#[test]
fn failed_config_update_shows_error_and_does_not_corrupt_state() {
    let options = vec![reasoning_option("low", &["low", "medium", "high"])];
    let mut app = TestUiBuilder::new()
        .session(|session| session.config_options(options))
        .agent(FakeAgent::hold::<acp::SetSessionConfigOptionRequest>)
        .build();

    app.key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
    app.run_command("/settings");
    let (_, responder) = app.held::<acp::SetSessionConfigOptionRequest>();
    responder.respond_with_error(agent_error("server error")).unwrap();
    app.settle_tasks();

    assert!(app.app().has_modal(), "the settings overlay stays open");
    let messages: Vec<_> = message_texts(&app).collect();
    let has_error =
        messages.iter().any(|message| message.contains("Failed to update") && message.contains("server error"));
    assert!(has_error, "expected user-visible error, got {messages:?}");

    let reasoning = app.app().core().session().config_options().iter().find(|option| option.id == "reasoning_effort");
    assert_eq!(reasoning.unwrap().current_value(), Some("medium"));
}

#[test]
fn form_modal_composed_space_does_not_answer() {
    let mut app = TestUi::new();

    let response_rx = app.elicit(form_elicitation(
        "test-server",
        "",
        ElicitationSchema::new().property("approved", BooleanPropertySchema::new().default_value(false), false),
    ));
    assert!(app.app().has_modal());

    app.key(KeyEvent::new(KeyCode::Char(' '), KeyModifiers::CONTROL));
    app.key(KeyEvent::new(KeyCode::Char(' '), KeyModifiers::SUPER));
    app.key(key(KeyCode::Char(' ')));
    app.key(key(KeyCode::Enter));

    let response = app.answer_to(response_rx);
    assert_eq!(
        accepted_content(&response),
        serde_json::json!({ "approved": false }),
        "composed spaces must not answer a choice page; only the arrows do"
    );
}
