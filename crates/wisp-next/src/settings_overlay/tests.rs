use super::*;
use acp_utils::config_meta::ConfigOptionMeta;
use agent_client_protocol::schema::{SessionConfigOption, SessionConfigSelectOption};

fn sel(id: &str, name: &str, current: &str, values: &[(&str, &str)]) -> SessionConfigOption {
    let options: Vec<SessionConfigSelectOption> =
        values.iter().map(|(v, n)| SessionConfigSelectOption::new((*v).to_string(), (*n).to_string())).collect();
    SessionConfigOption::select(id.to_string(), name.to_string(), current.to_string(), options)
}

#[test]
fn menu_builds_entries_from_config_options() {
    let opts = vec![
        sel("model", "Model", "gpt-4o", &[("gpt-4o", "GPT-4o"), ("claude", "Claude")]),
        sel("mode", "Mode", "code", &[("code", "Code"), ("chat", "Chat")]),
    ];
    let overlay = SettingsOverlay::new(&opts, vec![], vec![]);
    assert_eq!(overlay.menu.entries.len(), 2);
    assert_eq!(overlay.menu.entries[0].config_id, "model");
    assert_eq!(overlay.menu.entries[0].current_value_index, 0);
    assert_eq!(overlay.menu.entries[1].config_id, "mode");
}

#[test]
fn menu_finds_current_value() {
    let opts = vec![sel("model", "Model", "claude", &[("gpt-4o", "GPT-4o"), ("claude", "Claude"), ("llama", "Llama")])];
    let overlay = SettingsOverlay::new(&opts, vec![], vec![]);
    assert_eq!(overlay.menu.entries[0].current_value_index, 1);
}

#[test]
fn menu_navigation_wraps() {
    let opts = vec![
        sel("a", "A", "v1", &[("v1", "V1")]),
        sel("b", "B", "v1", &[("v1", "V1")]),
        sel("c", "C", "v1", &[("v1", "V1")]),
    ];
    let mut overlay = SettingsOverlay::new(&opts, vec![], vec![]);
    assert_eq!(overlay.menu.selection.selected(), Some(0));

    overlay.on_key(KeyEvent::new(KeyCode::Up, crossterm::event::KeyModifiers::NONE));
    assert_eq!(overlay.menu.selection.selected(), Some(2));

    overlay.on_key(KeyEvent::new(KeyCode::Down, crossterm::event::KeyModifiers::NONE));
    assert_eq!(overlay.menu.selection.selected(), Some(0));
}

#[test]
fn menu_skips_empty_values() {
    let empty = SessionConfigOption::select("x", "X", "v", Vec::<SessionConfigSelectOption>::new());
    let opts = vec![empty, sel("model", "Model", "a", &[("a", "A")])];
    let overlay = SettingsOverlay::new(&opts, vec![], vec![]);
    assert_eq!(overlay.menu.entries.len(), 1);
    assert_eq!(overlay.menu.entries[0].config_id, "model");
}

#[test]
fn menu_excludes_reasoning_effort() {
    let opts = vec![
        sel("model", "Model", "gpt-4o", &[("gpt-4o", "GPT-4o")]),
        sel("reasoning_effort", "Reasoning", "high", &[("none", "None"), ("low", "Low"), ("high", "High")]),
    ];
    let overlay = SettingsOverlay::new(&opts, vec![], vec![]);
    assert!(overlay.menu.entries.iter().any(|e| e.config_id == "model"));
    assert!(!overlay.menu.entries.iter().any(|e| e.config_id == "reasoning_effort"));
}

#[test]
fn multi_select_detected_from_meta() {
    let mut opt = sel("model", "Model", "a", &[("a", "A"), ("b", "B")]);
    opt = opt.meta(ConfigOptionMeta { multi_select: true }.into_meta());
    let overlay = SettingsOverlay::new(&[opt], vec![], vec![]);
    assert!(overlay.menu.entries[0].multi_select);
}

#[test]
fn multi_select_with_comma_shows_model_names() {
    let mut opt = sel("model", "Model", "a,b", &[("a", "Alpha"), ("b", "Beta")]);
    opt = opt.meta(ConfigOptionMeta { multi_select: true }.into_meta());
    let overlay = SettingsOverlay::new(&[opt], vec![], vec![]);
    let display = overlay.menu.entries[0].display_name.as_deref().unwrap();
    assert!(display.contains("Alpha"), "display: {display}");
    assert!(display.contains("Beta"), "display: {display}");
}

#[test]
fn esc_closes_overlay_from_menu() {
    let opts = vec![sel("model", "Model", "a", &[("a", "A")])];
    let mut overlay = SettingsOverlay::new(&opts, vec![], vec![]);
    let msgs = overlay.on_key(KeyEvent::new(KeyCode::Esc, crossterm::event::KeyModifiers::NONE));
    assert!(matches!(msgs.as_slice(), [SettingsOverlayMessage::Close]));
}

#[test]
fn enter_opens_picker_for_single_select() {
    let opts = vec![sel("model", "Model", "a", &[("a", "A"), ("b", "B")])];
    let mut overlay = SettingsOverlay::new(&opts, vec![], vec![]);
    overlay.on_key(KeyEvent::new(KeyCode::Enter, crossterm::event::KeyModifiers::NONE));
    assert!(matches!(overlay.active_pane, ActivePane::Picker(_)));
}

#[test]
fn enter_opens_model_selector_for_multi_select() {
    let mut opt = sel("model", "Model", "a", &[("a", "A"), ("b", "B")]);
    opt = opt.meta(ConfigOptionMeta { multi_select: true }.into_meta());
    let mut overlay = SettingsOverlay::new(&[opt], vec![], vec![]);
    overlay.on_key(KeyEvent::new(KeyCode::Enter, crossterm::event::KeyModifiers::NONE));
    assert!(matches!(overlay.active_pane, ActivePane::ModelSelector(_)));
}

#[test]
fn picker_esc_returns_to_menu() {
    let opts = vec![sel("model", "Model", "a", &[("a", "A"), ("b", "B")])];
    let mut overlay = SettingsOverlay::new(&opts, vec![], vec![]);
    overlay.on_key(KeyEvent::new(KeyCode::Enter, crossterm::event::KeyModifiers::NONE));
    assert!(matches!(overlay.active_pane, ActivePane::Picker(_)));
    overlay.on_key(KeyEvent::new(KeyCode::Esc, crossterm::event::KeyModifiers::NONE));
    assert!(matches!(overlay.active_pane, ActivePane::Menu));
}

#[test]
fn picker_confirm_returns_set_config_option() {
    let opts = vec![sel("model", "Model", "a", &[("a", "A"), ("b", "B")])];
    let mut overlay = SettingsOverlay::new(&opts, vec![], vec![]);
    overlay.on_key(KeyEvent::new(KeyCode::Enter, crossterm::event::KeyModifiers::NONE));
    // Navigate down to different value
    overlay.on_key(KeyEvent::new(KeyCode::Down, crossterm::event::KeyModifiers::NONE));
    let msgs = overlay.on_key(KeyEvent::new(KeyCode::Enter, crossterm::event::KeyModifiers::NONE));
    match msgs.as_slice() {
        [SettingsOverlayMessage::SetConfigOption { config_id, value }] => {
            assert_eq!(config_id, "model");
            assert_eq!(value, "b");
        }
        other => panic!("expected SetConfigOption, got: {other:?}"),
    }
}

#[test]
fn picker_confirm_applies_change_to_menu() {
    let opts = vec![sel("model", "Model", "a", &[("a", "A"), ("b", "B")])];
    let mut overlay = SettingsOverlay::new(&opts, vec![], vec![]);
    overlay.on_key(KeyEvent::new(KeyCode::Enter, crossterm::event::KeyModifiers::NONE));
    overlay.on_key(KeyEvent::new(KeyCode::Down, crossterm::event::KeyModifiers::NONE));
    overlay.on_key(KeyEvent::new(KeyCode::Enter, crossterm::event::KeyModifiers::NONE));
    assert_eq!(overlay.menu.entries[0].current_raw_value, "b");
    assert_eq!(overlay.menu.entries[0].current_value_index, 1);
}

#[test]
fn picker_confirm_no_change_returns_empty() {
    let opts = vec![sel("model", "Model", "a", &[("a", "A"), ("b", "B")])];
    let mut overlay = SettingsOverlay::new(&opts, vec![], vec![]);
    overlay.on_key(KeyEvent::new(KeyCode::Enter, crossterm::event::KeyModifiers::NONE));
    let msgs = overlay.on_key(KeyEvent::new(KeyCode::Enter, crossterm::event::KeyModifiers::NONE));
    assert!(msgs.is_empty());
}

#[test]
fn picker_query_filters_by_name() {
    let opts = vec![sel(
        "model",
        "Model",
        "gpt",
        &[("openrouter:gpt-4o", "GPT-4o"), ("openrouter:claude", "Claude Sonnet"), ("openrouter:gemini", "Gemini Pro")],
    )];
    let mut overlay = SettingsOverlay::new(&opts, vec![], vec![]);
    overlay.on_key(KeyEvent::new(KeyCode::Enter, crossterm::event::KeyModifiers::NONE));
    for c in "gem".chars() {
        overlay.on_key(KeyEvent::new(KeyCode::Char(c), crossterm::event::KeyModifiers::NONE));
    }
    if let ActivePane::Picker(ref picker) = overlay.active_pane {
        assert_eq!(picker.values.filtered_len(), 1);
        assert!(picker.values.selected_entry().is_some_and(|value| value.name.contains("Gemini")));
    } else {
        panic!("expected picker");
    }
}

#[test]
fn picker_disabled_option_not_selectable() {
    let opt = SessionConfigOption::select(
        "model",
        "Model",
        "a",
        vec![SessionConfigSelectOption::new("a", "A"), SessionConfigSelectOption::new("b", "B")],
    );
    let mut values = opt.clone();
    // set b as disabled
    if let SessionConfigKind::Select(ref mut select) = values.kind {
        select.options = SessionConfigSelectOptions::Ungrouped(vec![
            SessionConfigSelectOption::new("a", "A"),
            SessionConfigSelectOption::new("b".to_string(), "B".to_string())
                .description("Unavailable: need key".to_string()),
        ]);
    }
    let overlay = SettingsOverlay::new(&[values], vec![], vec![]);
    // Just check the entry has the disabled flag
    assert!(overlay.menu.entries[0].values[1].is_disabled);
}

#[test]
fn model_selector_enter_toggles() {
    let mut opt = sel(
        "model",
        "Model",
        "",
        &[("anthropic:opus", "Anthropic / Opus"), ("anthropic:sonnet", "Anthropic / Sonnet")],
    );
    opt = opt.meta(ConfigOptionMeta { multi_select: true }.into_meta());
    let mut overlay = SettingsOverlay::new(&[opt], vec![], vec![]);
    overlay.on_key(KeyEvent::new(KeyCode::Enter, crossterm::event::KeyModifiers::NONE));
    assert!(matches!(overlay.active_pane, ActivePane::ModelSelector(_)));

    // Toggle first model
    overlay.on_key(KeyEvent::new(KeyCode::Enter, crossterm::event::KeyModifiers::NONE));
    if let ActivePane::ModelSelector(ref selector) = overlay.active_pane {
        assert_eq!(selector.selected_models.len(), 1);
    }

    // Toggle again
    overlay.on_key(KeyEvent::new(KeyCode::Enter, crossterm::event::KeyModifiers::NONE));
    if let ActivePane::ModelSelector(ref selector) = overlay.active_pane {
        assert!(selector.selected_models.is_empty());
    }
}

#[test]
fn model_selector_esc_returns_to_menu() {
    let mut opt = sel("model", "Model", "", &[("a:m1", "A / M1")]);
    opt = opt.meta(ConfigOptionMeta { multi_select: true }.into_meta());
    let mut overlay = SettingsOverlay::new(&[opt], vec![], vec![]);
    overlay.on_key(KeyEvent::new(KeyCode::Enter, crossterm::event::KeyModifiers::NONE));
    let msgs = overlay.on_key(KeyEvent::new(KeyCode::Esc, crossterm::event::KeyModifiers::NONE));
    assert!(matches!(overlay.active_pane, ActivePane::Menu));
    assert!(msgs.is_empty());
}

#[test]
fn model_selector_returns_changes_on_esc() {
    let mut opt = sel(
        "model",
        "Model",
        "",
        &[("anthropic:opus", "Anthropic / Opus"), ("anthropic:sonnet", "Anthropic / Sonnet")],
    );
    opt = opt.meta(ConfigOptionMeta { multi_select: true }.into_meta());
    let mut overlay = SettingsOverlay::new(&[opt], vec![], vec![]);
    overlay.on_key(KeyEvent::new(KeyCode::Enter, crossterm::event::KeyModifiers::NONE));
    overlay.on_key(KeyEvent::new(KeyCode::Enter, crossterm::event::KeyModifiers::NONE));

    let msgs = overlay.on_key(KeyEvent::new(KeyCode::Esc, crossterm::event::KeyModifiers::NONE));
    match msgs.as_slice() {
        [SettingsOverlayMessage::SetConfigOption { config_id, value }] => {
            assert_eq!(config_id, "model");
            assert!(value.contains("anthropic:opus"));
        }
        other => panic!("expected SetConfigOption, got: {other:?}"),
    }
}

#[test]
fn model_selector_preselects_from_current_value() {
    let mut opt = sel(
        "model",
        "Model",
        "anthropic:opus,anthropic:sonnet",
        &[("anthropic:opus", "Anthropic / Opus"), ("anthropic:sonnet", "Anthropic / Sonnet")],
    );
    opt = opt.meta(ConfigOptionMeta { multi_select: true }.into_meta());
    let mut overlay = SettingsOverlay::new(&[opt], vec![], vec![]);
    overlay.on_key(KeyEvent::new(KeyCode::Enter, crossterm::event::KeyModifiers::NONE));
    if let ActivePane::ModelSelector(ref selector) = overlay.active_pane {
        assert_eq!(selector.selected_models.len(), 2);
    }
}

#[test]
fn update_config_options_refreshes_menu() {
    let opts = vec![sel("model", "Model", "a", &[("a", "A"), ("b", "B")])];
    let mut overlay = SettingsOverlay::new(&opts, vec![], vec![]);
    overlay.on_key(KeyEvent::new(KeyCode::Down, crossterm::event::KeyModifiers::NONE));
    overlay.on_key(KeyEvent::new(KeyCode::Down, crossterm::event::KeyModifiers::NONE));

    let new_opts = vec![sel("model", "Model", "b", &[("a", "A"), ("b", "B")])];
    overlay.update_config_options(&new_opts);
    assert_eq!(overlay.menu.entries[0].current_value_index, 1);
    assert_eq!(overlay.menu.entries[0].current_raw_value, "b");
}

#[test]
fn reasoning_effort_extracted_from_options() {
    let opts = vec![
        sel("model", "Model", "gpt-4o", &[("gpt-4o", "GPT-4o")]),
        sel("reasoning_effort", "Reasoning", "high", &[("none", "None"), ("low", "Low"), ("high", "High")]),
    ];
    let overlay = SettingsOverlay::new(&opts, vec![], vec![]);
    assert_eq!(overlay.current_reasoning_effort.as_deref(), Some("high"));
}

#[test]
fn reasoning_effort_none_filtered_out() {
    let opts = vec![
        sel("model", "Model", "gpt-4o", &[("gpt-4o", "GPT-4o")]),
        sel("reasoning_effort", "Reasoning", "none", &[("none", "None"), ("low", "Low")]),
    ];
    let overlay = SettingsOverlay::new(&opts, vec![], vec![]);
    assert_eq!(overlay.current_reasoning_effort, None);
}

#[test]
fn empty_config_options_creates_empty_menu() {
    let overlay = SettingsOverlay::new(&[], vec![], vec![]);
    assert!(overlay.menu.entries.is_empty());
}

#[test]
fn small_terminal_renders_placeholder() {
    let opts = vec![sel("model", "Model", "a", &[("a", "A")])];
    let mut overlay = SettingsOverlay::new(&opts, vec![], vec![]);
    let theme = Theme::default();
    let area = Rect::new(0, 0, 5, 2);
    let mut buffer = Buffer::empty(area);
    overlay.render(area, &mut buffer, &theme);
    let mut text = String::new();
    for y in 0..area.height {
        for x in 0..area.width {
            text.push(buffer.cell((x, y)).map_or(' ', |c| c.symbol().chars().next().unwrap_or(' ')));
        }
    }
    assert!(text.contains("term"), "text: {text}");
}

#[test]
fn truncate_to_width_handles_unicode() {
    assert_eq!(truncate_to_width("hello", 3), "he…");
    assert_eq!(truncate_to_width("a界b", 3), "a…");
    assert_eq!(truncate_to_width("hello", 10), "hello");
    assert_eq!(truncate_to_width("", 5), "");
    assert_eq!(truncate_to_width("abc", 0), "");
}

#[test]
fn provider_label_handles_slash_format() {
    assert_eq!(provider_label("Anthropic / Claude Sonnet", "anthropic"), "Anthropic");
}

#[test]
fn provider_label_capitalizes_key() {
    assert_eq!(provider_label("OpenRouter / GPT-4o", "openrouter"), "OpenRouter");
}

#[test]
fn model_label_extracts_after_slash() {
    assert_eq!(model_label("Anthropic / Claude Sonnet"), "Claude Sonnet");
    assert_eq!(model_label("GPT-4o"), "GPT-4o");
}

#[test]
fn reasoning_bar_shows_correct_indicators() {
    use utils::ReasoningEffort::*;
    let levels = &[Low, Medium, High];
    let bar = reasoning_bar(Some(Medium), levels);
    assert!(bar.contains("■"), "bar: {bar}");
    assert!(bar.contains("·"), "bar: {bar}");
    assert!(bar.contains("medium"), "bar: {bar}");

    let bar_none = reasoning_bar(None, levels);
    assert!(!bar_none.contains("■"), "bar_none: {bar_none}");
    assert!(bar_none.contains("none"), "bar_none: {bar_none}");
}

#[test]
fn capability_tags_all_combinations() {
    assert_eq!(capability_tags(false, false), "");
    assert_eq!(capability_tags(true, false), "img");
    assert_eq!(capability_tags(false, true), "audio");
    assert_eq!(capability_tags(true, true), "img  audio");
}

#[test]
fn provider_key_handles_unavailable_prefix() {
    assert_eq!(provider_key("__unavailable:moonshot"), "moonshot");
}

#[test]
fn update_options_with_mcp_and_theme_entries() {
    // Theme and MCP entries are added by App, not the overlay itself
    let opts = vec![sel("model", "Model", "a", &[("a", "A")])];
    let mut overlay = SettingsOverlay::new(&opts, vec![], vec![]);
    let new_opts =
        vec![sel("model", "Model", "b", &[("a", "A"), ("b", "B")]), sel("mode", "Mode", "code", &[("code", "Code")])];
    overlay.update_config_options(&new_opts);
    assert_eq!(overlay.menu.entries.len(), 2);
    assert_eq!(overlay.menu.entries[0].current_raw_value, "b");
}

#[test]
fn apply_change_updates_menu_entry() {
    let opts = vec![sel("model", "Model", "a", &[("a", "A"), ("b", "B")])];
    let mut overlay = SettingsOverlay::new(&opts, vec![], vec![]);
    overlay.apply_change(&SettingsChange { config_id: "model".to_string(), new_value: "b".to_string() });
    assert_eq!(overlay.menu.entries[0].current_raw_value, "b");
    assert_eq!(overlay.menu.entries[0].current_value_index, 1);
}

#[test]
fn model_selector_query_filters() {
    let mut opt = sel(
        "model",
        "Model",
        "",
        &[
            ("anthropic:opus", "Anthropic / Opus"),
            ("openai:gpt-4o", "OpenAI / GPT-4o"),
            ("google:gemini", "Google / Gemini"),
        ],
    );
    opt = opt.meta(ConfigOptionMeta { multi_select: true }.into_meta());
    let mut overlay = SettingsOverlay::new(&[opt], vec![], vec![]);
    overlay.on_key(KeyEvent::new(KeyCode::Enter, crossterm::event::KeyModifiers::NONE));

    for c in "gpt".chars() {
        overlay.on_key(KeyEvent::new(KeyCode::Char(c), crossterm::event::KeyModifiers::NONE));
    }
    if let ActivePane::ModelSelector(ref selector) = overlay.active_pane {
        assert_eq!(selector.filtered.len(), 1);
        let idx = selector.filtered[0];
        assert!(selector.all_items[idx].name.contains("GPT"), "name: {}", selector.all_items[idx].name);
    } else {
        panic!("expected model selector");
    }
}

#[tokio::test(flavor = "current_thread")]
async fn dropping_overlay_cancels_pending_elicitation() {
    tokio::task::LocalSet::new()
        .run_until(async {
            use acp_utils::notifications::{CreateElicitationRequestParams, ElicitationAction, ElicitationParams};
            use acp_utils::testing::test_connection;

            let opts = vec![sel("model", "Model", "a", &[("a", "A")])];
            let mut overlay = SettingsOverlay::new(&opts, vec![], vec![]);

            let (cx, mut peer) = test_connection().await;
            let (responder, response_rx) = peer.fake_elicitation(&cx).await;
            overlay.on_elicitation_request(
                ElicitationParams {
                    server_name: "test".into(),
                    request: CreateElicitationRequestParams::UrlElicitationParams {
                        meta: None,
                        message: String::new(),
                        url: "https://example.com".into(),
                        elicitation_id: "el-1".into(),
                    },
                },
                responder,
            );

            drop(overlay);

            let response = response_rx.await.unwrap();
            assert_eq!(response.action, ElicitationAction::Cancel);
        })
        .await;
}
