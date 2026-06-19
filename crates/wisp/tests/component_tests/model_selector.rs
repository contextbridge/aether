use tui::testing::render_component;
use tui::{Component, Event, KeyCode, KeyEvent, KeyModifiers, ViewContext};
use wisp::components::model_selector::{ModelEntry, ModelSelector};

async fn type_query(picker: &mut ModelSelector, text: &str) {
    for c in text.chars() {
        picker.on_event(&Event::Key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE))).await;
    }
}

fn rendered_lines(selector: &mut ModelSelector) -> Vec<String> {
    let term = render_component(|ctx| selector.render(ctx), 120, 40);
    let lines = term.get_lines();
    let last_non_empty = lines.iter().rposition(|l| !l.is_empty()).map_or(0, |i| i + 1);
    lines[..last_non_empty].to_vec()
}

fn model_values() -> Vec<ModelEntry> {
    vec![
        ModelEntry {
            value: "anthropic:claude-sonnet-4-5".to_string(),
            name: "Anthropic / Claude Sonnet 4.5".to_string(),
            reasoning_levels: vec![],
            supports_image: false,
            supports_audio: false,
            disabled_reason: None,
        },
        ModelEntry {
            value: "deepseek:deepseek-chat".to_string(),
            name: "DeepSeek / DeepSeek Chat".to_string(),
            reasoning_levels: vec![],
            supports_image: false,
            supports_audio: false,
            disabled_reason: None,
        },
        ModelEntry {
            value: "gemini:gemini-2.5-pro".to_string(),
            name: "Google / Gemini 2.5 Pro".to_string(),
            reasoning_levels: vec![],
            supports_image: false,
            supports_audio: false,
            disabled_reason: None,
        },
    ]
}

fn model_values_with_groups() -> Vec<ModelEntry> {
    vec![
        ModelEntry {
            value: "openrouter:anthropic/claude-sonnet-4-5".to_string(),
            name: "OpenRouter / Claude Sonnet 4.5".to_string(),
            reasoning_levels: vec![],
            supports_image: false,
            supports_audio: false,
            disabled_reason: None,
        },
        ModelEntry {
            value: "openrouter:google/gemini-2.5-pro".to_string(),
            name: "OpenRouter / Gemini 2.5 Pro".to_string(),
            reasoning_levels: vec![],
            supports_image: false,
            supports_audio: false,
            disabled_reason: None,
        },
        ModelEntry {
            value: "anthropic:claude-sonnet-4-5".to_string(),
            name: "Anthropic / Claude Sonnet 4.5".to_string(),
            reasoning_levels: vec![],
            supports_image: false,
            supports_audio: false,
            disabled_reason: None,
        },
        ModelEntry {
            value: "gemini:gemini-2.5-pro".to_string(),
            name: "Google / Gemini 2.5 Pro".to_string(),
            reasoning_levels: vec![],
            supports_image: false,
            supports_audio: false,
            disabled_reason: None,
        },
    ]
}

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

fn focused_provider_and_row(selector: &mut ModelSelector) -> (String, String) {
    let ctx = ViewContext::new((120, 40));
    let term = render_component(|ctx| selector.render(ctx), 120, 40);
    let lines = term.get_lines();
    let last_non_empty = lines.iter().rposition(|l| !l.is_empty()).map_or(0, |i| i + 1);
    let lines = &lines[..last_non_empty];

    // Find the focused row by checking for highlight_bg style
    let focused_idx = lines
        .iter()
        .enumerate()
        .position(|(i, _)| {
            let style = term.get_style_at(i, 0);
            style.bg == Some(ctx.theme.highlight_bg())
        })
        .expect("should have focused row");

    let provider = lines[..focused_idx]
        .iter()
        .rev()
        .map(|line| line.trim())
        .find(|line| {
            !line.is_empty() && !line.contains("Model search:") && !line.contains("Selected:") && !line.starts_with('[')
        })
        .expect("should find provider header")
        .to_string();

    (provider, lines[focused_idx].clone())
}

fn model_values_with_reasoning() -> Vec<ModelEntry> {
    vec![
        ModelEntry {
            value: "anthropic:claude-opus-4-6".to_string(),
            name: "Anthropic / Claude Opus 4.6".to_string(),
            reasoning_levels: vec![
                utils::ReasoningEffort::Low,
                utils::ReasoningEffort::Medium,
                utils::ReasoningEffort::High,
            ],
            supports_image: false,
            supports_audio: false,
            disabled_reason: None,
        },
        ModelEntry {
            value: "deepseek:deepseek-chat".to_string(),
            name: "DeepSeek / DeepSeek Chat".to_string(),
            reasoning_levels: vec![],
            supports_image: false,
            supports_audio: false,
            disabled_reason: None,
        },
    ]
}

fn make_selector(values: Vec<ModelEntry>) -> ModelSelector {
    ModelSelector::new(values, "model".to_string(), None, None)
}

fn make_selector_with(values: Vec<ModelEntry>, selection: Option<&str>, reasoning: Option<&str>) -> ModelSelector {
    ModelSelector::new(values, "model".to_string(), selection, reasoning)
}

#[tokio::test]
async fn search_filters_entries() {
    let mut builder = make_selector(model_values());
    type_query(&mut builder, "deepseek").await;
    let lines = rendered_lines(&mut builder);
    assert!(lines.iter().any(|l| l.trim() == "DeepSeek"));
    assert!(lines.iter().any(|l| l.contains("[ ] DeepSeek Chat")));
}

#[test]
fn render_groups_models_under_provider_headers() {
    let mut builder = make_selector(model_values_with_groups());
    let lines = rendered_lines(&mut builder);

    let openrouter_headers = lines.iter().filter(|l| l.trim() == "OpenRouter").count();
    assert_eq!(openrouter_headers, 1, "expected one OpenRouter header line");
    assert!(
        lines.windows(2).any(|w| w[0].trim().is_empty() && w[1].trim() == "Anthropic"),
        "expected blank separator before next provider: {lines:?}"
    );
    assert!(lines.iter().any(|l| l.contains("[ ] Claude Sonnet 4.5")));
    assert!(lines.iter().any(|l| l.contains("[ ] Gemini 2.5 Pro")));
}

#[tokio::test]
async fn search_filters_and_keeps_provider_headers() {
    let mut builder = make_selector(model_values_with_groups());
    type_query(&mut builder, "gemini").await;
    let lines = rendered_lines(&mut builder);

    assert!(lines.iter().any(|l| l.trim() == "OpenRouter"), "missing OpenRouter header in filtered results: {lines:?}");
    assert!(lines.iter().any(|l| l.trim() == "Google"), "missing Google header in filtered results: {lines:?}");
    assert!(lines.iter().any(|l| l.contains("[ ] Gemini 2.5 Pro")));
}

#[tokio::test]
async fn search_does_not_duplicate_provider_headers() {
    let values = vec![
        ModelEntry {
            value: "codex:gpt-5".to_string(),
            name: "Codex / GPT-5".to_string(),
            reasoning_levels: vec![],
            supports_image: false,
            supports_audio: false,
            disabled_reason: None,
        },
        ModelEntry {
            value: "openrouter:gpt-5".to_string(),
            name: "OpenRouter / GPT-5".to_string(),
            reasoning_levels: vec![],
            supports_image: false,
            supports_audio: false,
            disabled_reason: None,
        },
        ModelEntry {
            value: "codex:gpt-5-mini".to_string(),
            name: "Codex / GPT-5 Mini".to_string(),
            reasoning_levels: vec![],
            supports_image: false,
            supports_audio: false,
            disabled_reason: None,
        },
        ModelEntry {
            value: "openrouter:gpt-5-mini".to_string(),
            name: "OpenRouter / GPT-5 Mini".to_string(),
            reasoning_levels: vec![],
            supports_image: false,
            supports_audio: false,
            disabled_reason: None,
        },
    ];
    let mut selector = make_selector(values);
    type_query(&mut selector, "gpt").await;
    let lines = rendered_lines(&mut selector);

    let codex_count = lines.iter().filter(|l| l.trim() == "Codex").count();
    let openrouter_count = lines.iter().filter(|l| l.trim() == "OpenRouter").count();
    assert_eq!(codex_count, 1, "expected exactly one Codex header, got {codex_count}: {lines:?}");
    assert_eq!(openrouter_count, 1, "expected exactly one OpenRouter header, got {openrouter_count}: {lines:?}");
}

#[tokio::test]
async fn grouped_navigation_follows_rendered_order() {
    let mut selector = make_selector(model_values_with_groups());

    let (provider, focused) = focused_provider_and_row(&mut selector);
    assert_eq!(provider, "Anthropic");
    assert!(focused.contains("Claude Sonnet 4.5"));

    selector.on_event(&Event::Key(key(KeyCode::Down))).await;
    let (provider, focused) = focused_provider_and_row(&mut selector);
    assert_eq!(provider, "Google");
    assert!(focused.contains("Gemini 2.5 Pro"));

    selector.on_event(&Event::Key(key(KeyCode::Down))).await;
    let (provider, focused) = focused_provider_and_row(&mut selector);
    assert_eq!(provider, "OpenRouter");
    assert!(focused.contains("Claude Sonnet 4.5"));
}

#[tokio::test]
async fn grouped_navigation_after_search_follows_rendered_order() {
    let mut selector = make_selector(model_values_with_groups());
    type_query(&mut selector, "2.5").await;

    let (provider, focused) = focused_provider_and_row(&mut selector);
    assert_eq!(provider, "Google");
    assert!(focused.contains("Gemini 2.5 Pro"));

    selector.on_event(&Event::Key(key(KeyCode::Down))).await;
    let (provider, focused) = focused_provider_and_row(&mut selector);
    assert_eq!(provider, "OpenRouter");
    assert!(focused.contains("Gemini 2.5 Pro"));
}

#[test]
fn grouped_render_respects_small_height() {
    let mut builder = make_selector(model_values_with_groups());
    builder.update_viewport(6);
    let term = render_component(|ctx| builder.render(ctx), 120, 6);
    let output = term.get_lines();
    let non_empty_count = output.iter().filter(|l| !l.is_empty()).count();
    assert!(non_empty_count <= 6, "rendered too many lines for viewport: {output:?}");
    assert!(
        !output.iter().any(|l| l.contains("model selected") || l.contains("selected")),
        "did not expect bottom selected-count footer: {output:?}"
    );
}

#[test]
fn render_shows_selected_models_at_top() {
    let mut builder =
        make_selector_with(model_values(), Some("anthropic:claude-sonnet-4-5,deepseek:deepseek-chat"), None);
    let lines = rendered_lines(&mut builder);
    // Second line after header should be a spacer, then selected models line
    assert!(lines[1].trim().is_empty(), "expected spacer line after header");
    assert!(lines[2].contains("Selected:"), "expected Selected line, got: {}", lines[2]);
    assert!(lines[2].contains("Claude Sonnet 4.5"));
    assert!(lines[2].contains("DeepSeek Chat"));
    assert!(lines.get(3).is_some_and(|l| l.trim().is_empty()), "expected spacer line after selected line");
}

#[test]
fn render_hides_selected_line_when_none_selected() {
    let mut builder = make_selector(model_values());
    let lines = rendered_lines(&mut builder);
    assert!(!lines.iter().any(|l| l.contains("Selected:")), "should not show Selected line when nothing is selected");
    assert!(lines.get(1).is_some_and(|l| l.trim().is_empty()), "expected blank line after search header");
}

#[test]
fn render_shows_bar_on_focused_reasoning_row() {
    let mut selector = make_selector_with(model_values_with_reasoning(), None, Some("medium"));
    let term = render_component(|ctx| selector.render(ctx), 120, 40);
    let output = term.get_lines();
    let ctx = ViewContext::new((120, 40));
    let focused_line = output
        .iter()
        .enumerate()
        .find(|(i, _)| term.get_style_at(*i, 0).bg == Some(ctx.theme.highlight_bg()))
        .map(|(_, l)| l)
        .expect("should have focused line");
    assert!(focused_line.contains("medium [■■·]"), "expected reasoning bar with effort label, got: {focused_line}");
    assert!(!focused_line.contains("reasoning"), "expected no generic reasoning label, got: {focused_line}");
}

#[tokio::test]
async fn render_no_bar_on_non_reasoning_focused_row() {
    let mut selector = make_selector_with(model_values_with_reasoning(), None, Some("medium"));
    // Move to non-reasoning model
    selector.on_event(&Event::Key(key(KeyCode::Down))).await;
    let ctx = ViewContext::new((120, 40));
    let term = render_component(|ctx| selector.render(ctx), 120, 40);
    let output = term.get_lines();
    let focused_line = output
        .iter()
        .enumerate()
        .find(|(i, _)| term.get_style_at(*i, 0).bg == Some(ctx.theme.highlight_bg()))
        .map(|(_, l)| l)
        .expect("should have focused line");
    assert!(!focused_line.contains('■'), "should not show bar on non-reasoning model");
}
