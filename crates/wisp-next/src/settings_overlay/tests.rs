use super::model_selector::{capability_tags, model_label};
use super::*;
use acp_utils::config_meta::ConfigOptionMeta;
use agent_client_protocol::schema::{
    SessionConfigKind, SessionConfigOption, SessionConfigSelectOption, SessionConfigSelectOptions,
};
use crossterm::event::KeyModifiers;

fn sel(id: &str, name: &str, current: &str, values: &[(&str, &str)]) -> SessionConfigOption {
    let options: Vec<SessionConfigSelectOption> =
        values.iter().map(|(v, n)| SessionConfigSelectOption::new((*v).to_string(), (*n).to_string())).collect();
    SessionConfigOption::select(id.to_string(), name.to_string(), current.to_string(), options)
}

fn multi_select(id: &str, name: &str, current: &str, values: &[(&str, &str)]) -> SessionConfigOption {
    sel(id, name, current, values).meta(ConfigOptionMeta { multi_select: true }.into_meta())
}

fn press(overlay: &mut SettingsOverlay, code: KeyCode) -> Vec<SurfaceMessage> {
    overlay.on_key(KeyEvent::new(code, KeyModifiers::NONE))
}

fn type_query(overlay: &mut SettingsOverlay, query: &str) {
    for character in query.chars() {
        press(overlay, KeyCode::Char(character));
    }
}

/// The footer identifies which pane has focus, and is what the user sees.
fn footer(overlay: &SettingsOverlay) -> String {
    overlay.footer_text()
}

// ── Menu construction ──

#[test]
fn menu_builds_entries_from_config_options() {
    let opts = vec![
        sel("model", "Model", "gpt-4o", &[("gpt-4o", "GPT-4o"), ("claude", "Claude")]),
        sel("mode", "Mode", "code", &[("code", "Code"), ("chat", "Chat")]),
    ];
    let overlay = SettingsOverlay::new(&opts, vec![], &[]);
    assert_eq!(overlay.menu.entries.len(), 2);
    assert_eq!(overlay.menu.entries[0].config_id, "model");
    assert_eq!(overlay.menu.entries[0].current_value_index, 0);
    assert_eq!(overlay.menu.entries[1].config_id, "mode");
}

#[test]
fn menu_finds_current_value() {
    let opts = vec![sel("model", "Model", "claude", &[("gpt-4o", "GPT-4o"), ("claude", "Claude"), ("llama", "Llama")])];
    let overlay = SettingsOverlay::new(&opts, vec![], &[]);
    assert_eq!(overlay.menu.entries[0].current_value_index, 1);
}

#[test]
fn menu_navigation_wraps() {
    let opts = vec![
        sel("a", "A", "v1", &[("v1", "V1")]),
        sel("b", "B", "v1", &[("v1", "V1")]),
        sel("c", "C", "v1", &[("v1", "V1")]),
    ];
    let mut overlay = SettingsOverlay::new(&opts, vec![], &[]);
    assert_eq!(overlay.menu.selection.selected(), Some(0));

    press(&mut overlay, KeyCode::Up);
    assert_eq!(overlay.menu.selection.selected(), Some(2));

    press(&mut overlay, KeyCode::Down);
    assert_eq!(overlay.menu.selection.selected(), Some(0));
}

#[test]
fn menu_skips_empty_values() {
    let empty = SessionConfigOption::select("x", "X", "v", Vec::<SessionConfigSelectOption>::new());
    let opts = vec![empty, sel("model", "Model", "a", &[("a", "A")])];
    let overlay = SettingsOverlay::new(&opts, vec![], &[]);
    assert_eq!(overlay.menu.entries.len(), 1);
    assert_eq!(overlay.menu.entries[0].config_id, "model");
}

#[test]
fn menu_excludes_reasoning_effort() {
    let opts = vec![
        sel("model", "Model", "gpt-4o", &[("gpt-4o", "GPT-4o")]),
        sel("reasoning_effort", "Reasoning", "high", &[("none", "None"), ("low", "Low"), ("high", "High")]),
    ];
    let overlay = SettingsOverlay::new(&opts, vec![], &[]);
    assert!(overlay.menu.entries.iter().any(|e| e.config_id == "model"));
    assert!(!overlay.menu.entries.iter().any(|e| e.config_id == "reasoning_effort"));
}

#[test]
fn multi_select_detected_from_meta() {
    let overlay = SettingsOverlay::new(&[multi_select("model", "Model", "a", &[("a", "A"), ("b", "B")])], vec![], &[]);
    assert!(overlay.menu.entries[0].multi_select);
}

#[test]
fn multi_select_with_comma_shows_model_names() {
    let overlay =
        SettingsOverlay::new(&[multi_select("model", "Model", "a,b", &[("a", "Alpha"), ("b", "Beta")])], vec![], &[]);
    let display = overlay.menu.entries[0].display_name.as_deref().unwrap();
    assert!(display.contains("Alpha"), "display: {display}");
    assert!(display.contains("Beta"), "display: {display}");
}

#[test]
fn empty_config_options_creates_empty_menu() {
    let overlay = SettingsOverlay::new(&[], vec![], &[]);
    assert!(overlay.menu.entries.is_empty());
}

#[test]
fn picker_disabled_option_flagged_from_description() {
    let mut option = sel("model", "Model", "a", &[("a", "A")]);
    if let SessionConfigKind::Select(ref mut select) = option.kind {
        select.options = SessionConfigSelectOptions::Ungrouped(vec![
            SessionConfigSelectOption::new("a", "A"),
            SessionConfigSelectOption::new("b".to_string(), "B".to_string())
                .description("Unavailable: need key".to_string()),
        ]);
    }
    let overlay = SettingsOverlay::new(&[option], vec![], &[]);
    assert!(overlay.menu.entries[0].values[1].is_disabled);
}

// ── Pane navigation ──

#[test]
fn esc_closes_overlay_from_menu() {
    let opts = vec![sel("model", "Model", "a", &[("a", "A")])];
    let mut overlay = SettingsOverlay::new(&opts, vec![], &[]);
    let messages = press(&mut overlay, KeyCode::Esc);
    assert!(matches!(messages.as_slice(), [SurfaceMessage::Close]));
}

#[test]
fn enter_opens_picker_for_single_select() {
    let opts = vec![sel("model", "Model", "a", &[("a", "A"), ("b", "B")])];
    let mut overlay = SettingsOverlay::new(&opts, vec![], &[]);
    press(&mut overlay, KeyCode::Enter);
    assert!(footer(&overlay).contains("Confirm"), "footer: {}", footer(&overlay));
}

#[test]
fn enter_opens_model_selector_for_multi_select() {
    let mut overlay =
        SettingsOverlay::new(&[multi_select("model", "Model", "a", &[("a", "A"), ("b", "B")])], vec![], &[]);
    press(&mut overlay, KeyCode::Enter);
    assert!(footer(&overlay).contains("Toggle"), "footer: {}", footer(&overlay));
}

#[test]
fn picker_esc_returns_to_menu() {
    let opts = vec![sel("model", "Model", "a", &[("a", "A"), ("b", "B")])];
    let mut overlay = SettingsOverlay::new(&opts, vec![], &[]);
    press(&mut overlay, KeyCode::Enter);
    assert!(footer(&overlay).contains("Confirm"));
    press(&mut overlay, KeyCode::Esc);
    assert!(footer(&overlay).contains("Select"), "footer: {}", footer(&overlay));
}

// ── Picker ──

#[test]
fn picker_confirm_returns_set_config_option() {
    let opts = vec![sel("model", "Model", "a", &[("a", "A"), ("b", "B")])];
    let mut overlay = SettingsOverlay::new(&opts, vec![], &[]);
    press(&mut overlay, KeyCode::Enter);
    press(&mut overlay, KeyCode::Down);
    let messages = press(&mut overlay, KeyCode::Enter);
    match messages.as_slice() {
        [SurfaceMessage::SetConfigOption { config_id, value }] => {
            assert_eq!(config_id, "model");
            assert_eq!(value, "b");
        }
        other => panic!("expected SetConfigOption, got: {other:?}"),
    }
}

#[test]
fn picker_confirm_applies_change_to_menu() {
    let opts = vec![sel("model", "Model", "a", &[("a", "A"), ("b", "B")])];
    let mut overlay = SettingsOverlay::new(&opts, vec![], &[]);
    press(&mut overlay, KeyCode::Enter);
    press(&mut overlay, KeyCode::Down);
    press(&mut overlay, KeyCode::Enter);
    assert_eq!(overlay.menu.entries[0].current_raw_value, "b");
    assert_eq!(overlay.menu.entries[0].current_value_index, 1);
}

#[test]
fn picker_confirm_no_change_returns_empty() {
    let opts = vec![sel("model", "Model", "a", &[("a", "A"), ("b", "B")])];
    let mut overlay = SettingsOverlay::new(&opts, vec![], &[]);
    press(&mut overlay, KeyCode::Enter);
    assert!(press(&mut overlay, KeyCode::Enter).is_empty());
}

#[test]
fn picker_query_filters_by_name() {
    let opts = vec![sel(
        "model",
        "Model",
        "gpt",
        &[("openrouter:gpt-4o", "GPT-4o"), ("openrouter:claude", "Claude Sonnet"), ("openrouter:gemini", "Gemini Pro")],
    )];
    let mut overlay = SettingsOverlay::new(&opts, vec![], &[]);
    press(&mut overlay, KeyCode::Enter);
    type_query(&mut overlay, "gem");

    let messages = press(&mut overlay, KeyCode::Enter);
    match messages.as_slice() {
        [SurfaceMessage::SetConfigOption { value, .. }] => assert_eq!(value, "openrouter:gemini"),
        other => panic!("expected the only match to be confirmed, got: {other:?}"),
    }
}

// ── Model selector ──

#[test]
fn model_selector_toggles_and_commits_on_close() {
    let mut overlay = SettingsOverlay::new(
        &[multi_select(
            "model",
            "Model",
            "",
            &[("anthropic:opus", "Anthropic / Opus"), ("anthropic:sonnet", "Anthropic / Sonnet")],
        )],
        vec![],
        &[],
    );
    press(&mut overlay, KeyCode::Enter);
    press(&mut overlay, KeyCode::Enter);

    let messages = press(&mut overlay, KeyCode::Esc);
    match messages.as_slice() {
        [SurfaceMessage::SetConfigOption { config_id, value }] => {
            assert_eq!(config_id, "model");
            assert!(value.contains("anthropic:opus"), "value: {value}");
        }
        other => panic!("expected SetConfigOption, got: {other:?}"),
    }
}

#[test]
fn model_selector_toggling_twice_leaves_nothing_to_commit() {
    let mut overlay = SettingsOverlay::new(
        &[multi_select("model", "Model", "", &[("anthropic:opus", "Anthropic / Opus")])],
        vec![],
        &[],
    );
    press(&mut overlay, KeyCode::Enter);
    press(&mut overlay, KeyCode::Enter);
    press(&mut overlay, KeyCode::Enter);

    assert!(press(&mut overlay, KeyCode::Esc).is_empty());
    assert!(footer(&overlay).contains("Select"), "should be back on the menu");
}

#[test]
fn model_selector_preselects_from_current_value() {
    let mut overlay = SettingsOverlay::new(
        &[multi_select(
            "model",
            "Model",
            "anthropic:opus,anthropic:sonnet",
            &[("anthropic:opus", "Anthropic / Opus"), ("anthropic:sonnet", "Anthropic / Sonnet")],
        )],
        vec![],
        &[],
    );
    press(&mut overlay, KeyCode::Enter);
    // Both are already selected, so closing without edits changes nothing.
    assert!(press(&mut overlay, KeyCode::Esc).is_empty());
}

#[test]
fn model_selector_query_filters_before_toggling() {
    let mut overlay = SettingsOverlay::new(
        &[multi_select(
            "model",
            "Model",
            "",
            &[
                ("anthropic:opus", "Anthropic / Opus"),
                ("openai:gpt-4o", "OpenAI / GPT-4o"),
                ("google:gemini", "Google / Gemini"),
            ],
        )],
        vec![],
        &[],
    );
    press(&mut overlay, KeyCode::Enter);
    type_query(&mut overlay, "gpt");
    press(&mut overlay, KeyCode::Enter);

    match press(&mut overlay, KeyCode::Esc).as_slice() {
        [SurfaceMessage::SetConfigOption { value, .. }] => assert_eq!(value, "openai:gpt-4o"),
        other => panic!("expected only the filtered model to be selected, got: {other:?}"),
    }
}

// ── Config updates ──

#[test]
fn update_config_options_refreshes_menu() {
    let opts = vec![sel("model", "Model", "a", &[("a", "A"), ("b", "B")])];
    let mut overlay = SettingsOverlay::new(&opts, vec![], &[]);
    press(&mut overlay, KeyCode::Down);
    press(&mut overlay, KeyCode::Down);

    overlay.update_config_options(&[sel("model", "Model", "b", &[("a", "A"), ("b", "B")])]);
    assert_eq!(overlay.menu.entries[0].current_value_index, 1);
    assert_eq!(overlay.menu.entries[0].current_raw_value, "b");
}

#[test]
fn update_options_keeps_every_config_entry() {
    let opts = vec![sel("model", "Model", "a", &[("a", "A")])];
    let mut overlay = SettingsOverlay::new(&opts, vec![], &[]);
    overlay.update_config_options(&[
        sel("model", "Model", "b", &[("a", "A"), ("b", "B")]),
        sel("mode", "Mode", "code", &[("code", "Code")]),
    ]);
    assert_eq!(overlay.menu.entries.len(), 2);
    assert_eq!(overlay.menu.entries[0].current_raw_value, "b");
}

#[test]
fn apply_change_updates_menu_entry() {
    let opts = vec![sel("model", "Model", "a", &[("a", "A"), ("b", "B")])];
    let mut overlay = SettingsOverlay::new(&opts, vec![], &[]);
    overlay.apply_change(&SettingsChange { config_id: "model".to_string(), new_value: "b".to_string() });
    assert_eq!(overlay.menu.entries[0].current_raw_value, "b");
    assert_eq!(overlay.menu.entries[0].current_value_index, 1);
}

#[test]
fn reasoning_effort_extracted_from_options() {
    let opts = vec![
        sel("model", "Model", "gpt-4o", &[("gpt-4o", "GPT-4o")]),
        sel("reasoning_effort", "Reasoning", "high", &[("none", "None"), ("low", "Low"), ("high", "High")]),
    ];
    let overlay = SettingsOverlay::new(&opts, vec![], &[]);
    assert_eq!(overlay.current_reasoning_effort.as_deref(), Some("high"));
}

#[test]
fn reasoning_effort_none_filtered_out() {
    let opts = vec![
        sel("model", "Model", "gpt-4o", &[("gpt-4o", "GPT-4o")]),
        sel("reasoning_effort", "Reasoning", "none", &[("none", "None"), ("low", "Low")]),
    ];
    let overlay = SettingsOverlay::new(&opts, vec![], &[]);
    assert_eq!(overlay.current_reasoning_effort, None);
}

// ── Rendering ──

#[test]
fn small_terminal_renders_placeholder() {
    let opts = vec![sel("model", "Model", "a", &[("a", "A")])];
    let mut overlay = SettingsOverlay::new(&opts, vec![], &[]);
    let area = Rect::new(0, 0, 5, 2);
    let mut buffer = Buffer::empty(area);
    let theme = Theme::default();
    let mut highlighter = crate::syntax::SyntaxHighlighter::new();
    overlay.render(area, &mut buffer, &mut render_context(&theme, &mut highlighter));
    let text: String = buffer.content.iter().map(ratatui::buffer::Cell::symbol).collect();
    assert!(text.contains("term"), "text: {text}");
}

// ── Summaries and labels ──

#[test]
fn summarize_joins_non_empty_buckets() {
    assert_eq!(summarize(&[(2, "connected"), (0, "failed"), (1, "needs auth")], "none"), "2 connected, 1 needs auth");
    assert_eq!(summarize(&[(0, "connected")], "none"), "none");
}

#[test]
fn model_label_extracts_after_slash() {
    assert_eq!(model_label("Anthropic / Claude Sonnet"), "Claude Sonnet");
    assert_eq!(model_label("GPT-4o"), "GPT-4o");
}

#[test]
fn capability_tags_all_combinations() {
    assert_eq!(capability_tags(false, false), "");
    assert_eq!(capability_tags(true, false), "img");
    assert_eq!(capability_tags(false, true), "audio");
    assert_eq!(capability_tags(true, true), "img  audio");
}

// ── Elicitation lifetime ──

#[tokio::test(flavor = "current_thread")]
async fn dropping_overlay_cancels_pending_elicitation() {
    tokio::task::LocalSet::new()
        .run_until(async {
            use acp_utils::notifications::{CreateElicitationRequestParams, ElicitationAction, ElicitationParams};
            use acp_utils::testing::test_connection;

            let opts = vec![sel("model", "Model", "a", &[("a", "A")])];
            let mut overlay = SettingsOverlay::new(&opts, vec![], &[]);

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

/// Rendering services for a surface under test.
fn render_context<'a>(
    theme: &'a Theme,
    highlighter: &'a mut crate::syntax::SyntaxHighlighter,
) -> crate::render_context::RenderContext<'a> {
    crate::render_context::RenderContext { theme, highlighter, theme_generation: 0 }
}
