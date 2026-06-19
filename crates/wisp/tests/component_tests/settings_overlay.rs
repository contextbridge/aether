use acp_utils::notifications::{McpServerAuthCapability, McpServerStatus, McpServerStatusEntry};
use agent_client_protocol::schema::{self as acp, SessionConfigSelectOption};
use tui::testing::render_component;
use tui::{Component, Event, KeyCode, KeyEvent, KeyModifiers, ViewContext};
use wisp::settings::menu::SettingsMenu;
use wisp::settings::overlay::{SettingsMessage, SettingsOverlay};

fn make_menu() -> SettingsMenu {
    let options = vec![
        agent_client_protocol::schema::SessionConfigOption::select(
            "provider",
            "Provider",
            "openrouter",
            vec![
                SessionConfigSelectOption::new("openrouter", "OpenRouter"),
                SessionConfigSelectOption::new("ollama", "Ollama"),
            ],
        ),
        agent_client_protocol::schema::SessionConfigOption::select(
            "model",
            "Model",
            "gpt-4o",
            vec![
                SessionConfigSelectOption::new("gpt-4o", "GPT-4o"),
                SessionConfigSelectOption::new("claude", "Claude"),
            ],
        ),
    ];
    SettingsMenu::from_config_options(&options)
}

fn make_multi_select_menu() -> SettingsMenu {
    let mut meta = serde_json::Map::new();
    meta.insert("multi_select".to_string(), serde_json::Value::Bool(true));
    let options = vec![
        agent_client_protocol::schema::SessionConfigOption::select(
            "provider",
            "Provider",
            "openrouter",
            vec![
                SessionConfigSelectOption::new("openrouter", "OpenRouter"),
                SessionConfigSelectOption::new("ollama", "Ollama"),
            ],
        ),
        agent_client_protocol::schema::SessionConfigOption::select(
            "model",
            "Model",
            "gpt-4o",
            vec![
                SessionConfigSelectOption::new("gpt-4o", "GPT-4o"),
                SessionConfigSelectOption::new("claude", "Claude"),
            ],
        )
        .meta(meta),
    ];
    SettingsMenu::from_config_options(&options)
}

fn make_server_statuses() -> Vec<McpServerStatusEntry> {
    vec![
        McpServerStatusEntry::new("github", McpServerStatus::Connected { tool_count: 5 }),
        McpServerStatusEntry::new("linear", McpServerStatus::NeedsOAuth)
            .with_auth_capability(McpServerAuthCapability::OAuth),
    ]
}

fn make_mixed_server_statuses() -> Vec<McpServerStatusEntry> {
    vec![
        McpServerStatusEntry::new("github", McpServerStatus::Connected { tool_count: 5 }),
        McpServerStatusEntry::new("math", McpServerStatus::Connected { tool_count: 3 }).with_proxied(true),
        McpServerStatusEntry::new("linear", McpServerStatus::NeedsOAuth)
            .with_auth_capability(McpServerAuthCapability::OAuth)
            .with_proxied(true),
    ]
}

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

fn render_footer(overlay: &mut SettingsOverlay) -> String {
    let height = 23_usize; // 24 - 1
    overlay.update_child_viewport(height.saturating_sub(4));
    let term = render_component(|ctx| overlay.render(ctx), 80, 24);
    let output = term.get_lines();
    // Overlay fills 23 lines (indices 0-22). Footer is line 21, bottom border is line 22.
    output[21].clone()
}

fn render_plain_text(overlay: &mut SettingsOverlay) -> Vec<String> {
    let height = 23_usize;
    overlay.update_child_viewport(height.saturating_sub(4));
    let term = render_component(|ctx| overlay.render(ctx), 80, 24);
    term.get_lines()
}

fn make_auth_methods() -> Vec<acp::AuthMethod> {
    vec![
        acp::AuthMethod::Agent(acp::AuthMethodAgent::new("anthropic", "Anthropic")),
        acp::AuthMethod::Agent(acp::AuthMethodAgent::new("openrouter", "OpenRouter")),
    ]
}

/// Helper to create a `ConfigOverlay` with the server status overlay open.
/// Replaces the old `with_server_overlay()` test-only method.
async fn open_server_overlay(mut menu: SettingsMenu, statuses: Vec<McpServerStatusEntry>) -> SettingsOverlay {
    menu.add_mcp_servers_entry("1 connected, 1 needs auth");
    let mut overlay = SettingsOverlay::new(menu, statuses, vec![]);
    // Navigate past provider (0) and model (1) to MCP servers (2)
    overlay.on_event(&Event::Key(key(KeyCode::Down))).await;
    overlay.on_event(&Event::Key(key(KeyCode::Down))).await;
    overlay.on_event(&Event::Key(key(KeyCode::Enter))).await;
    overlay
}

#[test]
fn bordered_box_fills_terminal_height_minus_one() {
    let mut overlay = SettingsOverlay::new(make_menu(), vec![], vec![]);
    let term = render_component(|ctx| overlay.render(ctx), 80, 24);
    let output = term.get_lines();
    // Frame fills 23 lines, leaving the last row (index 23) empty
    assert!(!output[22].is_empty(), "line 22 should have content");
    assert!(output[23].is_empty(), "line 23 should be empty");
}

#[test]
fn title_contains_configuration() {
    let mut overlay = SettingsOverlay::new(make_menu(), vec![], vec![]);
    let term = render_component(|ctx| overlay.render(ctx), 80, 24);
    let output = term.get_lines();
    assert!(output[0].contains("Configuration"));
}

#[test]
fn footer_shows_select_and_close_for_menu() {
    let mut overlay = SettingsOverlay::new(make_menu(), vec![], vec![]);
    let term = render_component(|ctx| overlay.render(ctx), 80, 24);
    let output = term.get_lines();
    let footer = &output[21]; // second to last content line (last is bottom border at 22)
    assert!(footer.contains("[Enter] Select"), "footer: {footer}");
    assert!(footer.contains("[Esc] Close"), "footer: {footer}");
}

#[tokio::test]
async fn footer_shows_confirm_and_back_for_picker() {
    let mut overlay = SettingsOverlay::new(make_menu(), vec![], vec![]);
    // Open picker
    overlay.on_event(&Event::Key(key(KeyCode::Enter))).await;
    let term = render_component(|ctx| overlay.render(ctx), 80, 24);
    let output = term.get_lines();
    let footer = &output[21];
    assert!(footer.contains("[Enter] Confirm"), "footer: {footer}");
    assert!(footer.contains("[Esc] Back"), "footer: {footer}");
}

#[tokio::test]
async fn footer_shows_authenticate_and_back_for_servers() {
    let menu = make_menu();
    let statuses = make_server_statuses();
    let mut overlay = open_server_overlay(menu, statuses).await;
    let footer = render_footer(&mut overlay);
    assert!(footer.contains("[Enter] Authenticate OAuth servers"), "footer: {footer}");
    assert!(footer.contains("[Esc] Back"), "footer: {footer}");
}

#[test]
fn selected_entry_has_bg_color() {
    let mut overlay = SettingsOverlay::new(make_menu(), vec![], vec![]);
    let ctx = ViewContext::new((80, 24));
    let term = render_component(|c| overlay.render(c), 80, 24);
    let output = term.get_lines();
    let row =
        output.iter().position(|l| l.contains("Provider: OpenRouter")).expect("expected provider row to be rendered");
    let style = term.style_of_text(row, "Provider: OpenRouter").unwrap();
    assert_eq!(style.bg, Some(ctx.theme.highlight_bg()), "selected entry should have highlight_bg");
}

#[test]
fn render_root_menu_shows_top_level_rows() {
    let mut overlay = SettingsOverlay::new(make_menu(), vec![], vec![]);

    let lines = render_plain_text(&mut overlay);
    let text = lines.join("\n");

    assert!(text.contains("Provider: OpenRouter"), "rendered:\n{text}");
    assert!(text.contains("Model: GPT-4o"), "rendered:\n{text}");
    assert!(text.contains("[Enter] Select"), "rendered:\n{text}");
    assert!(text.contains("[Esc] Close"), "rendered:\n{text}");
}

#[tokio::test]
async fn render_picker_hides_top_level_rows() {
    let mut overlay = SettingsOverlay::new(make_menu(), vec![], vec![]);
    overlay.on_event(&Event::Key(key(KeyCode::Enter))).await;

    let lines = render_plain_text(&mut overlay);
    let text = lines.join("\n");

    assert!(text.contains("Provider search:"), "rendered:\n{text}");
    assert!(!text.contains("Provider: OpenRouter"), "rendered:\n{text}");
    assert!(!text.contains("Model: GPT-4o"), "rendered:\n{text}");
    assert!(text.contains("[Enter] Confirm"), "rendered:\n{text}");
    assert!(text.contains("[Esc] Back"), "rendered:\n{text}");
}

#[tokio::test]
async fn render_model_selector_hides_top_level_rows() {
    let mut overlay = SettingsOverlay::new(make_multi_select_menu(), vec![], vec![]);
    overlay.on_event(&Event::Key(key(KeyCode::Down))).await;
    overlay.on_event(&Event::Key(key(KeyCode::Enter))).await;

    let lines = render_plain_text(&mut overlay);
    let text = lines.join("\n");

    assert!(text.contains("Model search:"), "rendered:\n{text}");
    assert!(!text.contains("Provider: OpenRouter"), "rendered:\n{text}");
    assert!(!text.contains("Model: GPT-4o"), "rendered:\n{text}");
    assert!(text.contains("Toggle"), "rendered:\n{text}");
    assert!(text.contains("Effort: none"), "rendered:\n{text}");
    assert!(!text.contains("[Tab] Reasoning"), "rendered:\n{text}");
    assert!(text.contains("[Esc] Done"), "rendered:\n{text}");
}

#[tokio::test]
async fn render_server_overlay_hides_top_level_rows() {
    let menu = make_menu();
    let statuses = make_server_statuses();
    let mut overlay = open_server_overlay(menu, statuses).await;

    let lines = render_plain_text(&mut overlay);
    let text = lines.join("\n");

    assert!(text.contains("github  \u{2713} 5 tools"), "rendered:\n{text}");
    assert!(text.contains("linear  \u{26A1} needs authentication"), "rendered:\n{text}");
    assert!(!text.contains("Provider: OpenRouter"), "rendered:\n{text}");
    assert!(!text.contains("Model: GPT-4o"), "rendered:\n{text}");
    assert!(text.contains("[Enter] Authenticate OAuth servers"), "rendered:\n{text}");
    assert!(text.contains("[Esc] Back"), "rendered:\n{text}");
}

#[tokio::test]
async fn render_server_overlay_groups_direct_and_proxied_statuses() {
    let menu = make_menu();
    let statuses = make_mixed_server_statuses();
    let mut overlay = open_server_overlay(menu, statuses).await;
    let lines = render_plain_text(&mut overlay);
    let text = lines.join("\n");

    assert!(text.contains("Direct"), "rendered:\n{text}");
    assert!(text.contains("github  \u{2713} 5 tools"), "rendered:\n{text}");
    assert!(text.contains("Proxied"), "rendered:\n{text}");
    assert!(text.contains("math  \u{2713} 3 tools"), "rendered:\n{text}");
    assert!(text.contains("linear  \u{26A1} needs authentication"), "rendered:\n{text}");
    assert!(!text.contains("proxy  \u{2713} 1 tool"), "rendered:\n{text}");
}

#[tokio::test]
async fn render_provider_login_overlay_hides_top_level_rows() {
    let mut menu = make_menu();
    menu.add_provider_logins_entry("2 needs login");
    let mut overlay = SettingsOverlay::new(menu, vec![], make_auth_methods());
    overlay.on_event(&Event::Key(key(KeyCode::Down))).await;
    overlay.on_event(&Event::Key(key(KeyCode::Down))).await;
    let outcome = overlay.on_event(&Event::Key(key(KeyCode::Enter))).await;
    assert!(outcome.is_some());

    let lines = render_plain_text(&mut overlay);
    let text = lines.join("\n");

    assert!(text.contains("Anthropic  \u{26A1} needs login"), "rendered:\n{text}");
    assert!(text.contains("OpenRouter  \u{26A1} needs login"), "rendered:\n{text}");
    assert!(!text.contains("Provider: OpenRouter"), "rendered:\n{text}");
    assert!(!text.contains("Model: GPT-4o"), "rendered:\n{text}");
    assert!(text.contains("[Enter] Authenticate"), "rendered:\n{text}");
    assert!(text.contains("[Esc] Back"), "rendered:\n{text}");
}

#[test]
fn narrow_terminal_does_not_panic() {
    let mut overlay = SettingsOverlay::new(make_menu(), vec![], vec![]);
    let term = render_component(|ctx| overlay.render(ctx), 4, 3);
    let output = term.get_lines();
    assert!(!output.is_empty());
}

#[test]
fn very_small_terminal_shows_fallback() {
    let mut overlay = SettingsOverlay::new(make_menu(), vec![], vec![]);
    // Width must be >= text length to avoid truncation, but small enough to trigger fallback
    // MIN_WIDTH=6, MIN_HEIGHT=3, height = rows-1, so rows=3 gives height=2 < 3
    let term = render_component(|ctx| overlay.render(ctx), 30, 3);
    let output = term.get_lines();
    assert!(output[0].contains("too small"), "got: {output:?}");
}

#[test]
fn update_settings_options_never_renders_reasoning_row() {
    // Initial options include model + reasoning_effort
    let initial_options = vec![
        agent_client_protocol::schema::SessionConfigOption::select(
            "model",
            "Model",
            "claude-opus",
            vec![
                SessionConfigSelectOption::new("claude-opus", "Claude Opus"),
                SessionConfigSelectOption::new("deepseek-chat", "DeepSeek Chat"),
            ],
        ),
        agent_client_protocol::schema::SessionConfigOption::select(
            "reasoning_effort",
            "Reasoning Effort",
            "high",
            vec![
                SessionConfigSelectOption::new("none", "None"),
                SessionConfigSelectOption::new("low", "Low"),
                SessionConfigSelectOption::new("medium", "Medium"),
                SessionConfigSelectOption::new("high", "High"),
            ],
        ),
    ];
    let menu = SettingsMenu::from_config_options(&initial_options);
    let mut overlay = SettingsOverlay::new(menu, vec![], vec![]);

    // Rendered lines do not contain Reasoning Effort
    let term = render_component(|ctx| overlay.render(ctx), 80, 24);
    let text = term.get_lines().join("\n");
    assert!(!text.contains("Reasoning Effort"), "Reasoning Effort should NOT appear initially; got:\n{text}");

    // After update to model-only options, still no Reasoning Effort
    let updated_options = vec![agent_client_protocol::schema::SessionConfigOption::select(
        "model",
        "Model",
        "deepseek-chat",
        vec![
            SessionConfigSelectOption::new("claude-opus", "Claude Opus"),
            SessionConfigSelectOption::new("deepseek-chat", "DeepSeek Chat"),
        ],
    )];
    overlay.update_config_options(&updated_options);

    let term = render_component(|ctx| overlay.render(ctx), 80, 24);
    let text = term.get_lines().join("\n");
    assert!(!text.contains("Reasoning Effort"), "Reasoning Effort should NOT appear after update; got:\n{text}");
}

#[tokio::test]
async fn footer_shows_toggle_when_model_selector_open() {
    let mut overlay = SettingsOverlay::new(make_multi_select_menu(), vec![], vec![]);

    overlay.on_event(&Event::Key(key(KeyCode::Down))).await;
    overlay.on_event(&Event::Key(key(KeyCode::Enter))).await;

    let term = render_component(|ctx| overlay.render(ctx), 80, 24);
    let output = term.get_lines();
    let footer = &output[21];
    assert!(footer.contains("Toggle"), "footer: {footer}");
    assert!(footer.contains("Effort: none"), "footer: {footer}");
    assert!(!footer.contains("[Tab] Reasoning"), "footer: {footer}");
    assert!(footer.contains("[Esc] Done"), "footer: {footer}");
}

#[tokio::test]
async fn tall_terminal_shows_more_picker_items() {
    // Create a menu with many model options
    let many_models: Vec<SessionConfigSelectOption> =
        (0..20).map(|i| SessionConfigSelectOption::new(format!("model-{i}"), format!("Model {i}"))).collect();
    let options =
        vec![agent_client_protocol::schema::SessionConfigOption::select("model", "Model", "model-0", many_models)];
    let menu = SettingsMenu::from_config_options(&options);
    let mut overlay = SettingsOverlay::new(menu, vec![], vec![]);
    overlay.on_event(&Event::Key(key(KeyCode::Enter))).await; // open picker

    // Render at a tall terminal (60 rows)
    let height_tall = 59_usize; // 60 - 1
    overlay.update_child_viewport(height_tall.saturating_sub(4));
    let term_tall = render_component(|ctx| overlay.render(ctx), 80, 60);
    let tall_lines = term_tall.get_lines();
    let tall_model_lines = tall_lines.iter().filter(|l| l.contains("Model ")).count();

    // Render at a short terminal (15 rows)
    let height_short = 14_usize; // 15 - 1
    overlay.update_child_viewport(height_short.saturating_sub(4));
    let term_short = render_component(|ctx| overlay.render(ctx), 80, 15);
    let short_lines = term_short.get_lines();
    let short_model_lines = short_lines.iter().filter(|l| l.contains("Model ")).count();

    assert!(
        tall_model_lines > short_model_lines,
        "tall terminal ({tall_model_lines} items) should show more picker items than short ({short_model_lines})"
    );
}

#[tokio::test]
async fn server_overlay_esc_closes_server_not_settings_overlay() {
    let menu = make_menu();
    let statuses = make_server_statuses();
    let mut overlay = open_server_overlay(menu, statuses).await;
    assert!(render_footer(&mut overlay).contains("Authenticate OAuth servers"));

    let outcome = overlay.on_event(&Event::Key(key(KeyCode::Esc))).await;
    assert!(outcome.is_some());
    assert!(render_footer(&mut overlay).contains("[Enter] Select"));
    assert!(outcome.unwrap().is_empty());
}

#[tokio::test]
async fn multi_select_entry_opens_model_selector() {
    let mut overlay = SettingsOverlay::new(make_multi_select_menu(), vec![], vec![]);

    // Navigate to the model entry (index 1: provider=0, model=1)
    overlay.on_event(&Event::Key(key(KeyCode::Down))).await;
    overlay.on_event(&Event::Key(key(KeyCode::Enter))).await;

    let footer = render_footer(&mut overlay);
    assert!(footer.contains("Toggle"), "expected model selector, got: {footer}");
    assert!(footer.contains("Effort: none"), "expected effort label, got: {footer}");
}

#[tokio::test]
async fn connected_oauth_server_can_reauth_from_server_overlay() {
    let menu = make_menu();
    let statuses = vec![
        McpServerStatusEntry::new("remote", McpServerStatus::Connected { tool_count: 2 })
            .with_auth_capability(McpServerAuthCapability::OAuth),
    ];

    let mut overlay = open_server_overlay(menu, statuses).await;
    let messages = overlay.on_event(&Event::Key(key(KeyCode::Enter))).await.unwrap();
    match messages.as_slice() {
        [SettingsMessage::AuthenticateServer(name)] => assert_eq!(name, "remote"),
        other => panic!("messages: {other:?}"),
    }
}
