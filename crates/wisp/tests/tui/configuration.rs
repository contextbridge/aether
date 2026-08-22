use super::support::*;

#[test]
fn required_elicitation_field_must_be_completed() {
    block_on_local(async {
        let mut app = make_app();
        let schema: ElicitationSchema = serde_json::from_value(serde_json::json!({
            "type": "object",
            "properties": { "name": { "type": "string" } },
            "required": ["name"]
        }))
        .unwrap();

        let mut response_rx = with_elicitation(&mut app, form_elicitation("", schema)).await;
        app.key(key(KeyCode::Enter));
        assert!(response_rx.try_recv().is_err());
        app.type_text("Ada");
        app.key(key(KeyCode::Enter));

        let response = response_rx.await.unwrap();
        assert_eq!(response.action, ElicitationAction::Accept);
        assert_eq!(response.content, Some(serde_json::json!({ "name": "Ada" })));
    });
}

#[test]
fn elicitation_request_is_accepted_interactively() {
    block_on_local(async {
        let mut app = make_app();

        let response_rx = with_elicitation(
            &mut app,
            ElicitationParams {
                server_name: "test-server".to_string(),
                request: ElicitRequestParams::FormElicitationParams {
                    meta: None,
                    message: "Confirm the action".to_string(),
                    requested_schema: ElicitationSchema::builder().build().unwrap(),
                },
            },
        )
        .await;
        assert!(app.app().has_modal());

        app.key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

        let response = response_rx.await.unwrap();
        assert_eq!(response.action, ElicitationAction::Accept);
        assert_eq!(response.content, Some(serde_json::json!({})));
        assert!(!app.app().has_modal());
    });
}

#[test]
fn tab_cycles_reasoning_effort_through_advertised_levels() {
    let options = vec![reasoning_option("low", &["low", "medium", "high"])];
    let mut app = TestUiBuilder::new().config_options(options).build();

    app.key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
    let cmd = app.next_agent_command().unwrap();
    assert!(
        matches!(cmd, AgentCommand::SetConfigOption { ref config_id, ref value, .. } if config_id == "reasoning_effort" && value == "medium")
    );

    app.key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
    let cmd = app.next_agent_command().unwrap();
    assert!(
        matches!(cmd, AgentCommand::SetConfigOption { ref config_id, ref value, .. } if config_id == "reasoning_effort" && value == "high")
    );

    app.key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
    let cmd = app.next_agent_command().unwrap();
    assert!(
        matches!(cmd, AgentCommand::SetConfigOption { ref config_id, ref value, .. } if config_id == "reasoning_effort" && value == "none")
    );

    app.key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
    let cmd = app.next_agent_command().unwrap();
    assert!(
        matches!(cmd, AgentCommand::SetConfigOption { ref config_id, ref value, .. } if config_id == "reasoning_effort" && value == "low")
    );
}

#[test]
fn shift_backtab_cycles_mode_option_and_wraps() {
    let options = vec![mode_option("code", &["code", "plan", "ask"])];
    let mut app = TestUiBuilder::new().config_options(options).build();

    app.key(KeyEvent::new(KeyCode::BackTab, KeyModifiers::SHIFT));
    let cmd = app.next_agent_command().unwrap();
    assert!(
        matches!(cmd, AgentCommand::SetConfigOption { ref config_id, ref value, .. } if config_id == "mode" && value == "plan")
    );

    app.key(KeyEvent::new(KeyCode::BackTab, KeyModifiers::SHIFT));
    let cmd = app.next_agent_command().unwrap();
    assert!(
        matches!(cmd, AgentCommand::SetConfigOption { ref config_id, ref value, .. } if config_id == "mode" && value == "ask")
    );

    app.key(KeyEvent::new(KeyCode::BackTab, KeyModifiers::SHIFT));
    let cmd = app.next_agent_command().unwrap();
    assert!(
        matches!(cmd, AgentCommand::SetConfigOption { ref config_id, ref value, .. } if config_id == "mode" && value == "code")
    );
}

#[test]
fn shift_backtab_cycles_grouped_mode_options() {
    let options = vec![grouped_mode_option("code", &[("built-in", &["code", "plan"]), ("custom", &["review"])])];
    let mut app = TestUiBuilder::new().config_options(options).build();

    app.key(KeyEvent::new(KeyCode::BackTab, KeyModifiers::SHIFT));
    let cmd = app.next_agent_command().unwrap();
    assert!(
        matches!(cmd, AgentCommand::SetConfigOption { ref config_id, ref value, .. } if config_id == "mode" && value == "plan")
    );

    app.key(KeyEvent::new(KeyCode::BackTab, KeyModifiers::SHIFT));
    let cmd = app.next_agent_command().unwrap();
    assert!(
        matches!(cmd, AgentCommand::SetConfigOption { ref config_id, ref value, .. } if config_id == "mode" && value == "review"),
        "cycling must cross group boundaries, not stop at the first group"
    );
}

#[test]
fn tab_and_backtab_noop_without_cycleable_options() {
    let mut app = make_app();

    app.key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
    app.key(KeyEvent::new(KeyCode::BackTab, KeyModifiers::SHIFT));

    assert!(app.next_command().is_none());
}

#[test]
fn command_picker_consumes_tab_without_changing_config() {
    let options = vec![reasoning_option("low", &["low", "medium", "high"])];
    let mut app = TestUiBuilder::new().config_options(options).build();

    app.key(key(KeyCode::Char('/')));
    assert!(app.app().composer().has_completion());

    app.key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));

    assert!(!app.app().composer().has_completion());
}

#[test]
fn failed_config_update_shows_error_and_does_not_corrupt_state() {
    let options = vec![reasoning_option("low", &["low", "medium", "high"])];
    let mut app = TestUiBuilder::new().config_options(options).build();

    app.key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
    let _ = app.next_agent_command().unwrap();

    app.acp_event(AcpEvent::ConfigOptionUpdateFailed { error: "server error".to_string() });

    let messages: Vec<_> = message_texts(&app).collect();
    let has_error = messages.iter().any(|message| message.contains("Failed to update"));
    assert!(has_error, "expected user-visible error, got {messages:?}");

    let reasoning = app.app().config_options().iter().find(|option| option.id == "reasoning_effort");
    assert_eq!(reasoning.unwrap().current_value(), Some("medium"));
}

#[test]
fn double_ctrl_c_exits_over_elicitation_modal() {
    block_on_local(async {
        let mut app = make_app();

        with_elicitation(
            &mut app,
            ElicitationParams {
                server_name: "test-server".to_string(),
                request: ElicitRequestParams::FormElicitationParams {
                    meta: None,
                    message: String::new(),
                    requested_schema: ElicitationSchema::builder().build().unwrap(),
                },
            },
        )
        .await;
        assert!(app.app().has_modal());
        assert_ctrl_c_exits(&mut app);
    });
}

#[test]
fn form_modal_composed_space_does_not_answer() {
    block_on_local(async {
        let mut app = make_app();

        let response_rx = with_elicitation(
            &mut app,
            ElicitationParams {
                server_name: "test-server".to_string(),
                request: ElicitRequestParams::FormElicitationParams {
                    meta: None,
                    message: String::new(),
                    requested_schema: ElicitationSchema::builder().optional_bool("approved", false).build().unwrap(),
                },
            },
        )
        .await;
        assert!(app.app().has_modal());

        app.key(KeyEvent::new(KeyCode::Char(' '), KeyModifiers::CONTROL));
        app.key(KeyEvent::new(KeyCode::Char(' '), KeyModifiers::SUPER));
        app.key(key(KeyCode::Char(' ')));
        app.key(key(KeyCode::Enter));

        let response = response_rx.await.unwrap();
        assert_eq!(response.action, ElicitationAction::Accept);
        assert_eq!(
            response.content,
            Some(serde_json::json!({ "approved": false })),
            "composed spaces must not answer a choice page; only the arrows do"
        );
    });
}
