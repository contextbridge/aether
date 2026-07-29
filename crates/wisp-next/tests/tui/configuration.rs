use super::support::*;

#[test]
fn required_elicitation_field_must_be_completed() {
    let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    LocalSet::new().block_on(&runtime, async {
        let (mut app, _command_rx) = make_app();
        let (cx, mut peer) = test_connection().await;
        let (responder, mut response_rx) = peer.fake_elicitation(&cx).await;
        let schema: ElicitationSchema = serde_json::from_value(serde_json::json!({
            "type": "object",
            "properties": { "name": { "type": "string" } },
            "required": ["name"]
        }))
        .unwrap();

        app.on_acp_event(AcpEvent::ElicitationRequest {
            params: ElicitationParams {
                server_name: "test-server".to_string(),
                request: CreateElicitationRequestParams::FormElicitationParams {
                    meta: None,
                    message: String::new(),
                    requested_schema: schema,
                },
            },
            responder,
        });
        app.on_key(key(KeyCode::Enter));
        assert!(response_rx.try_recv().is_err());
        type_text(&mut app, "Ada");
        app.on_key(key(KeyCode::Enter));

        let response = response_rx.await.unwrap();
        assert_eq!(response.action, ElicitationAction::Accept);
        assert_eq!(response.content, Some(serde_json::json!({ "name": "Ada" })));
    });
}

#[test]
fn elicitation_request_is_accepted_interactively() {
    let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    LocalSet::new().block_on(&runtime, async {
        let (mut app, _command_rx) = make_app();
        let (cx, mut peer) = test_connection().await;
        let (responder, response_rx) = peer.fake_elicitation(&cx).await;

        app.on_acp_event(AcpEvent::ElicitationRequest {
            params: ElicitationParams {
                server_name: "test-server".to_string(),
                request: CreateElicitationRequestParams::FormElicitationParams {
                    meta: None,
                    message: "Confirm the action".to_string(),
                    requested_schema: ElicitationSchema::builder().build().unwrap(),
                },
            },
            responder,
        });
        assert!(app.has_modal());

        app.on_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

        let response = response_rx.await.unwrap();
        assert_eq!(response.action, ElicitationAction::Accept);
        assert_eq!(response.content, Some(serde_json::json!({})));
        assert!(!app.has_modal());
    });
}

#[test]
fn tab_cycles_reasoning_effort_through_advertised_levels() {
    let options = vec![reasoning_option("low", &["low", "medium", "high"])];
    let (mut app, mut command_rx) = AppBuilder::new().config_options(options).build();

    app.on_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
    let cmd = command_rx.try_recv().unwrap();
    assert!(
        matches!(cmd, PromptCommand::SetConfigOption { ref config_id, ref value, .. } if config_id == "reasoning_effort" && value == "medium")
    );

    app.on_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
    let cmd = command_rx.try_recv().unwrap();
    assert!(
        matches!(cmd, PromptCommand::SetConfigOption { ref config_id, ref value, .. } if config_id == "reasoning_effort" && value == "high")
    );

    app.on_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
    let cmd = command_rx.try_recv().unwrap();
    assert!(
        matches!(cmd, PromptCommand::SetConfigOption { ref config_id, ref value, .. } if config_id == "reasoning_effort" && value == "none")
    );

    app.on_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
    let cmd = command_rx.try_recv().unwrap();
    assert!(
        matches!(cmd, PromptCommand::SetConfigOption { ref config_id, ref value, .. } if config_id == "reasoning_effort" && value == "low")
    );
}

#[test]
fn shift_backtab_cycles_mode_option_and_wraps() {
    let options = vec![mode_option("code", &["code", "plan", "ask"])];
    let (mut app, mut command_rx) = AppBuilder::new().config_options(options).build();

    app.on_key(KeyEvent::new(KeyCode::BackTab, KeyModifiers::SHIFT));
    let cmd = command_rx.try_recv().unwrap();
    assert!(
        matches!(cmd, PromptCommand::SetConfigOption { ref config_id, ref value, .. } if config_id == "mode" && value == "plan")
    );

    app.on_key(KeyEvent::new(KeyCode::BackTab, KeyModifiers::SHIFT));
    let cmd = command_rx.try_recv().unwrap();
    assert!(
        matches!(cmd, PromptCommand::SetConfigOption { ref config_id, ref value, .. } if config_id == "mode" && value == "ask")
    );

    app.on_key(KeyEvent::new(KeyCode::BackTab, KeyModifiers::SHIFT));
    let cmd = command_rx.try_recv().unwrap();
    assert!(
        matches!(cmd, PromptCommand::SetConfigOption { ref config_id, ref value, .. } if config_id == "mode" && value == "code")
    );
}

#[test]
fn shift_backtab_cycles_grouped_mode_options() {
    let options = vec![grouped_mode_option("code", &[("built-in", &["code", "plan"]), ("custom", &["review"])])];
    let (mut app, mut command_rx) = AppBuilder::new().config_options(options).build();

    app.on_key(KeyEvent::new(KeyCode::BackTab, KeyModifiers::SHIFT));
    let cmd = command_rx.try_recv().unwrap();
    assert!(
        matches!(cmd, PromptCommand::SetConfigOption { ref config_id, ref value, .. } if config_id == "mode" && value == "plan")
    );

    app.on_key(KeyEvent::new(KeyCode::BackTab, KeyModifiers::SHIFT));
    let cmd = command_rx.try_recv().unwrap();
    assert!(
        matches!(cmd, PromptCommand::SetConfigOption { ref config_id, ref value, .. } if config_id == "mode" && value == "review"),
        "cycling must cross group boundaries, not stop at the first group"
    );
}

#[test]
fn tab_and_backtab_noop_without_cycleable_options() {
    let (mut app, mut command_rx) = make_app();

    app.on_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
    app.on_key(KeyEvent::new(KeyCode::BackTab, KeyModifiers::SHIFT));

    assert!(command_rx.try_recv().is_err());
}

#[test]
fn command_picker_consumes_tab_without_changing_config() {
    let options = vec![reasoning_option("low", &["low", "medium", "high"])];
    let (mut app, _command_rx) = AppBuilder::new().config_options(options).build();

    app.on_key(key(KeyCode::Char('/')));
    assert!(app.composer().has_completion());

    app.on_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));

    assert!(!app.composer().has_completion());
}

#[test]
fn failed_config_update_shows_error_and_does_not_corrupt_state() {
    let options = vec![reasoning_option("low", &["low", "medium", "high"])];
    let (mut app, mut command_rx) = AppBuilder::new().config_options(options).build();

    app.on_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
    let _ = command_rx.try_recv().unwrap();

    app.on_acp_event(AcpEvent::ConfigOptionUpdateFailed { error: "server error".to_string() });

    let items = app.drain_finalized();
    let has_error = items.iter().any(|item| matches!(item, HistoryItem::User(msg) if msg.contains("Failed to update")));
    assert!(has_error, "expected user-visible error, got {items:?}");

    let reasoning = app.config_options().iter().find(|o| o.id.0.as_ref() == "reasoning_effort");
    let acp::SessionConfigKind::Select(select) = &reasoning.unwrap().kind else {
        panic!("expected select");
    };
    assert_eq!(select.current_value.0.as_ref(), "medium");
}

#[test]
fn double_ctrl_c_exits_over_elicitation_modal() {
    let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    LocalSet::new().block_on(&runtime, async {
        let (mut app, _command_rx) = make_app();
        let (cx, mut peer) = test_connection().await;
        let (responder, _response_rx) = peer.fake_elicitation(&cx).await;

        app.on_acp_event(AcpEvent::ElicitationRequest {
            params: ElicitationParams {
                server_name: "test-server".to_string(),
                request: CreateElicitationRequestParams::FormElicitationParams {
                    meta: None,
                    message: String::new(),
                    requested_schema: ElicitationSchema::builder().build().unwrap(),
                },
            },
            responder,
        });
        assert!(app.has_modal());
        assert_ctrl_c_exits_over_open_layer(&mut app);
    });
}

#[test]
fn form_modal_composed_space_does_not_toggle() {
    let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    LocalSet::new().block_on(&runtime, async {
        let (mut app, _command_rx) = make_app();
        let (cx, mut peer) = test_connection().await;
        let (responder, response_rx) = peer.fake_elicitation(&cx).await;

        app.on_acp_event(AcpEvent::ElicitationRequest {
            params: ElicitationParams {
                server_name: "test-server".to_string(),
                request: CreateElicitationRequestParams::FormElicitationParams {
                    meta: None,
                    message: String::new(),
                    requested_schema: ElicitationSchema::builder().optional_bool("approved", false).build().unwrap(),
                },
            },
            responder,
        });
        assert!(app.has_modal());

        app.on_key(KeyEvent::new(KeyCode::Char(' '), KeyModifiers::CONTROL));
        app.on_key(KeyEvent::new(KeyCode::Char(' '), KeyModifiers::SUPER));
        app.on_key(key(KeyCode::Char(' ')));
        app.on_key(key(KeyCode::Enter));

        let response = response_rx.await.unwrap();
        assert_eq!(response.action, ElicitationAction::Accept);
        assert_eq!(response.content, Some(serde_json::json!({ "approved": true })));
    });
}
