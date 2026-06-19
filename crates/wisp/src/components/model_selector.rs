use super::reasoning_bar::reasoning_bar;
use crate::settings::types::SettingsChange;
use std::cmp::Ordering;
use std::collections::HashSet;
use tui::{Combobox, Component, Event, Frame, Line, MouseEventKind, PickerKey, Searchable, ViewContext, classify_key};
use utils::ReasoningEffort;

#[derive(Debug, Clone)]
pub struct ModelEntry {
    pub value: String,
    pub name: String,
    pub reasoning_levels: Vec<ReasoningEffort>,
    pub supports_image: bool,
    pub supports_audio: bool,
    pub disabled_reason: Option<String>,
}

impl ModelEntry {
    pub fn is_disabled(&self) -> bool {
        self.disabled_reason.is_some()
    }

    fn provider_key(&self) -> &str {
        if let Some(provider) = self.value.strip_prefix("__unavailable:") {
            return provider;
        }
        self.value.split_once(':').map_or("Other", |(provider, _)| provider)
    }

    fn provider_label(&self) -> String {
        if let Some((provider, _)) = self.name.split_once(" / ") {
            return provider.to_string();
        }

        let key = self.provider_key();
        if key.is_empty() {
            return "Other".to_string();
        }

        let mut chars = key.chars();
        let first = chars.next().map(|c| c.to_uppercase().to_string()).unwrap_or_default();
        let rest = chars.as_str().to_lowercase();
        format!("{first}{rest}")
    }

    fn model_label(&self) -> &str {
        self.name.split_once(" / ").map_or(self.name.as_str(), |(_, model)| model)
    }
}

impl Searchable for ModelEntry {
    fn search_text(&self) -> String {
        format!("{} {}", self.name, self.value)
    }
}

fn compare_model_entries(a: &ModelEntry, b: &ModelEntry) -> Ordering {
    a.provider_key()
        .cmp(b.provider_key())
        .then_with(|| a.model_label().cmp(b.model_label()))
        .then_with(|| a.name.cmp(&b.name))
        .then_with(|| a.value.cmp(&b.value))
}

fn capability_tags(supports_image: bool, supports_audio: bool) -> &'static str {
    match (supports_image, supports_audio) {
        (true, true) => "img  audio",
        (true, false) => "img",
        (false, true) => "audio",
        (false, false) => "",
    }
}

const REASONING_EFFORT_CONFIG_ID: &str = "reasoning_effort";

pub struct ModelSelector {
    combobox: Combobox<ModelEntry>,
    all_items: Vec<ModelEntry>,
    selected_models: HashSet<String>,
    original_models: HashSet<String>,
    config_id: String,
    reasoning_effort: Option<ReasoningEffort>,
    original_reasoning_effort: Option<ReasoningEffort>,
}

#[derive(Debug)]
pub enum ModelSelectorMessage {
    Done(Vec<SettingsChange>),
}

impl ModelSelector {
    pub fn new(
        items: Vec<ModelEntry>,
        config_id: String,
        current_selection: Option<&str>,
        current_reasoning_effort: Option<&str>,
    ) -> Self {
        let selected_models: HashSet<String> =
            current_selection.map(|s| s.split(',').map(|p| p.trim().to_string()).collect()).unwrap_or_default();

        let reasoning = current_reasoning_effort.and_then(|s| s.parse().ok());

        let original_models = selected_models.clone();
        let all_items = items.clone();
        let mut combobox = Combobox::new(items);
        combobox.set_match_sort(compare_model_entries);
        if !selected_models.is_empty() {
            combobox.select_first_where(|item| !item.is_disabled() && selected_models.contains(&item.value));
        }
        if combobox.selected().is_some_and(ModelEntry::is_disabled) {
            combobox.select_first_where(|e| !e.is_disabled());
        }
        Self {
            combobox,
            all_items,
            selected_models,
            original_models,
            config_id,
            reasoning_effort: reasoning,
            original_reasoning_effort: reasoning,
        }
    }

    pub fn query(&self) -> &str {
        self.combobox.query()
    }

    /// Returns the value of the currently focused model entry.
    pub fn focused_value(&self) -> Option<String> {
        self.combobox.selected().map(|e| e.value.clone())
    }

    pub fn selected_values(&self) -> &HashSet<String> {
        &self.selected_models
    }

    pub fn reasoning_effort(&self) -> Option<ReasoningEffort> {
        self.reasoning_effort
    }

    #[cfg(test)]
    pub fn selected_count(&self) -> usize {
        self.selected_models.len()
    }

    pub fn toggle_focused(&mut self) {
        if let Some(entry) = self.combobox.selected()
            && !entry.is_disabled()
        {
            let value = entry.value.clone();
            if !self.selected_models.remove(&value) {
                self.selected_models.insert(value);
            }
        }
    }

    pub fn cycle_reasoning_effort_forward(&mut self) {
        if let Some(entry) = self.combobox.selected()
            && !entry.is_disabled()
            && !entry.reasoning_levels.is_empty()
        {
            self.reasoning_effort = ReasoningEffort::cycle_within(self.reasoning_effort, &entry.reasoning_levels);
        }
    }

    pub fn cycle_reasoning_effort_back(&mut self) {
        if let Some(entry) = self.combobox.selected()
            && !entry.is_disabled()
            && !entry.reasoning_levels.is_empty()
        {
            let levels = &entry.reasoning_levels;
            self.reasoning_effort = match self.reasoning_effort {
                None => levels.last().copied(),
                Some(current) => match levels.iter().position(|&l| l == current) {
                    Some(0) | None => None,
                    Some(i) => Some(levels[i - 1]),
                },
            };
        }
    }

    fn ensure_selectable(&mut self) {
        if let Some(entry) = self.combobox.selected()
            && entry.is_disabled()
        {
            self.combobox.select_first_where(|e| !e.is_disabled());
        }
    }

    fn clamp_reasoning_to_focused(&mut self) {
        if let Some(effort) = self.reasoning_effort
            && let Some(entry) = self.combobox.selected()
        {
            if entry.reasoning_levels.is_empty() {
                self.reasoning_effort = None;
            } else {
                self.reasoning_effort = Some(effort.clamp_to(&entry.reasoning_levels));
            }
        }
    }

    fn confirm(&self) -> Vec<SettingsChange> {
        let mut changes = Vec::new();
        if !self.selected_models.is_empty() && self.selected_models != self.original_models {
            let joined = self.selected_models.iter().cloned().collect::<Vec<_>>().join(",");
            changes.push(SettingsChange { config_id: self.config_id.clone(), new_value: joined });
        }
        if self.reasoning_effort != self.original_reasoning_effort {
            changes.push(SettingsChange {
                config_id: REASONING_EFFORT_CONFIG_ID.to_string(),
                new_value: ReasoningEffort::config_str(self.reasoning_effort).to_string(),
            });
        }
        changes
    }
}

impl ModelSelector {
    pub fn update_viewport(&mut self, max_height: usize) {
        let header_lines = if self.selected_models.is_empty() { 2 } else { 4 };
        let available = max_height.saturating_sub(header_lines);

        let mut max_items = available;
        for _ in 0..3 {
            self.combobox.set_max_visible(max_items.max(1));
            let matches = self.combobox.visible_matches_with_selection();
            let groups = count_provider_groups(&matches);
            let interstitial = if groups > 0 { groups + groups.saturating_sub(1) } else { 0 };
            let needed = max_items + interstitial;
            if needed <= available {
                break;
            }
            max_items = available.saturating_sub(interstitial);
        }
        self.combobox.set_max_visible(max_items.max(1));
    }
}

impl Component for ModelSelector {
    type Message = ModelSelectorMessage;

    async fn on_event(&mut self, event: &Event) -> Option<Vec<Self::Message>> {
        if let Event::Mouse(mouse) = event {
            return match mouse.kind {
                MouseEventKind::ScrollUp => {
                    self.combobox.move_up_where(|e| !e.is_disabled());
                    self.clamp_reasoning_to_focused();
                    Some(vec![])
                }
                MouseEventKind::ScrollDown => {
                    self.combobox.move_down_where(|e| !e.is_disabled());
                    self.clamp_reasoning_to_focused();
                    Some(vec![])
                }
                _ => Some(vec![]),
            };
        }
        let Event::Key(key) = event else {
            return None;
        };
        match classify_key(*key, self.combobox.query().is_empty()) {
            PickerKey::Escape => {
                let changes = self.confirm();
                Some(vec![ModelSelectorMessage::Done(changes)])
            }
            PickerKey::MoveUp => {
                self.combobox.move_up_where(|e| !e.is_disabled());
                self.clamp_reasoning_to_focused();
                Some(vec![])
            }
            PickerKey::MoveDown => {
                self.combobox.move_down_where(|e| !e.is_disabled());
                self.clamp_reasoning_to_focused();
                Some(vec![])
            }
            PickerKey::Tab => {
                if let Some(entry) = self.combobox.selected()
                    && !entry.is_disabled()
                    && !entry.reasoning_levels.is_empty()
                {
                    self.reasoning_effort =
                        ReasoningEffort::cycle_within(self.reasoning_effort, &entry.reasoning_levels);
                }
                Some(vec![])
            }
            PickerKey::Confirm => {
                self.toggle_focused();
                Some(vec![])
            }
            PickerKey::Char(c) => {
                self.combobox.push_query_char(c);
                self.ensure_selectable();
                Some(vec![])
            }
            PickerKey::Backspace => {
                self.combobox.pop_query_char();
                self.ensure_selectable();
                Some(vec![])
            }
            PickerKey::MoveLeft
            | PickerKey::MoveRight
            | PickerKey::BackTab
            | PickerKey::BackspaceOnEmpty
            | PickerKey::ControlChar
            | PickerKey::Other => Some(vec![]),
        }
    }

    fn render(&mut self, context: &ViewContext) -> Frame {
        let mut lines = Vec::new();
        let header = format!("  Model search: {}", self.combobox.query());
        lines.push(Line::new(header));
        lines.push(Line::new(String::new()));

        if !self.selected_models.is_empty() {
            let names: Vec<&str> = self
                .all_items
                .iter()
                .filter(|item| self.selected_models.contains(&item.value))
                .map(|item| item.name.as_str())
                .collect();
            let selected_text = format!("  Selected: {}", names.join(", "));
            lines.push(Line::styled(selected_text, context.theme.muted()));
            lines.push(Line::new(String::new()));
        }

        let mut item_lines = Vec::new();
        if self.combobox.is_empty() {
            item_lines.push(Line::new("  (no matches found)".to_string()));
        } else {
            let selected = &self.selected_models;
            let mut last_provider: Option<&str> = None;

            let items = self.combobox.visible_matches_with_selection();

            for (entry, is_focused) in &items {
                let provider = entry.provider_key();
                if last_provider != Some(provider) {
                    if !item_lines.is_empty() {
                        item_lines.push(Line::new(String::new()));
                    }
                    item_lines.push(Line::styled(entry.provider_label(), context.theme.heading()));
                    last_provider = Some(provider);
                }

                if entry.is_disabled() {
                    let reason = entry.disabled_reason.as_deref().unwrap_or("unavailable");
                    let label = format!("    {}  {}", entry.model_label(), reason);
                    item_lines.push(Line::styled(label, context.theme.muted()));
                    continue;
                }

                let check = if selected.contains(&entry.value) { "[x] " } else { "[ ] " };

                let label = format!("{check}{}", entry.model_label());
                if *is_focused {
                    let mut line = Line::with_style(label, context.theme.selected_row_style());
                    let indicator_style = context.theme.selected_row_style_with_fg(context.theme.highlight_fg());
                    if !entry.reasoning_levels.is_empty() {
                        let bar = reasoning_bar(self.reasoning_effort, entry.reasoning_levels.len());
                        line.push_with_style(format!("    {bar}"), indicator_style);
                    }
                    let caps = capability_tags(entry.supports_image, entry.supports_audio);
                    if !caps.is_empty() {
                        line.push_with_style(format!("    {caps}"), indicator_style);
                    }
                    item_lines.push(line);
                } else {
                    item_lines.push(Line::styled(label, context.theme.text_primary()));
                }
            }
        }

        let max_h = context.size.height as usize;
        let available_for_items = max_h.saturating_sub(lines.len());
        item_lines.truncate(available_for_items);
        lines.extend(item_lines);

        Frame::new(lines)
    }
}

fn count_provider_groups(items: &[(&ModelEntry, bool)]) -> usize {
    let mut count = 0;
    let mut last_provider: Option<&str> = None;
    for (entry, _) in items {
        let provider = entry.provider_key();
        if last_provider != Some(provider) {
            count += 1;
            last_provider = Some(provider);
        }
    }
    count
}

#[cfg(test)]
mod tests {
    use super::*;
    use tui::{KeyCode, KeyEvent, KeyModifiers, MouseEvent, MouseEventKind};

    fn entry(value: &str, name: &str, levels: Vec<ReasoningEffort>) -> ModelEntry {
        ModelEntry {
            value: value.to_string(),
            name: name.to_string(),
            reasoning_levels: levels,
            supports_image: false,
            supports_audio: false,
            disabled_reason: None,
        }
    }

    fn disabled_entry(value: &str, name: &str, reason: &str) -> ModelEntry {
        ModelEntry {
            value: value.to_string(),
            name: name.to_string(),
            reasoning_levels: vec![],
            supports_image: false,
            supports_audio: false,
            disabled_reason: Some(reason.to_string()),
        }
    }

    fn make_items() -> Vec<ModelEntry> {
        vec![
            entry("anthropic:claude-sonnet-4-5", "Anthropic / Claude Sonnet 4.5", vec![]),
            entry("deepseek:deepseek-chat", "DeepSeek / DeepSeek Chat", vec![]),
            entry("gemini:gemini-2.5-pro", "Google / Gemini 2.5 Pro", vec![]),
        ]
    }

    fn make_selector() -> ModelSelector {
        ModelSelector::new(make_items(), "model".to_string(), None, None)
    }

    fn sel(items: Vec<ModelEntry>, selected: Option<&str>, reasoning: Option<&str>) -> ModelSelector {
        ModelSelector::new(items, "model".to_string(), selected, reasoning)
    }

    async fn send(s: &mut ModelSelector, k: KeyEvent) -> Option<Vec<ModelSelectorMessage>> {
        s.on_event(&Event::Key(k)).await
    }

    fn k(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn assert_confirm_models(changes: &[SettingsChange], expected: &[&str]) {
        let model_change = changes.iter().find(|c| c.config_id == "model").unwrap();
        let parts: HashSet<&str> = model_change.new_value.split(',').collect();
        for val in expected {
            assert!(parts.contains(val), "expected {val} in {parts:?}");
        }
        assert_eq!(parts.len(), expected.len());
    }

    use ReasoningEffort::*;

    fn reasoning_3() -> Vec<ReasoningEffort> {
        vec![Low, Medium, High]
    }

    fn reasoning_4() -> Vec<ReasoningEffort> {
        vec![Low, Medium, High, Xhigh]
    }

    fn make_reasoning_items() -> Vec<ModelEntry> {
        vec![
            entry("anthropic:claude-opus-4-6", "Anthropic / Claude Opus 4.6", reasoning_3()),
            entry("deepseek:deepseek-chat", "DeepSeek / DeepSeek Chat", vec![]),
        ]
    }

    fn make_mixed_reasoning_items() -> Vec<ModelEntry> {
        vec![
            entry("codex:gpt-5.4-codex", "Codex / GPT-5.4 Codex", reasoning_4()),
            entry("anthropic:claude-opus-4-6", "Anthropic / Claude Opus 4.6", reasoning_3()),
        ]
    }

    fn many_provider_items() -> Vec<ModelEntry> {
        ["a:m1", "b:m2", "c:m3", "d:m4", "e:m5", "f:m6"]
            .into_iter()
            .map(|v| {
                let (prov, model) = v.split_once(':').unwrap();
                entry(v, &format!("{} / {}", prov.to_uppercase(), model.to_uppercase()), vec![])
            })
            .collect()
    }

    #[tokio::test]
    async fn enter_toggles_focused_model() {
        let mut s = make_selector();
        assert_eq!(s.selected_count(), 0);
        send(&mut s, k(KeyCode::Enter)).await;
        assert_eq!(s.selected_count(), 1);
        send(&mut s, k(KeyCode::Enter)).await;
        assert_eq!(s.selected_count(), 0);
    }

    #[tokio::test]
    async fn space_adds_to_search_query_not_selects() {
        let mut s = make_selector();
        assert_eq!(s.selected_count(), 0);
        assert_eq!(s.query(), "");

        send(&mut s, k(KeyCode::Char('K'))).await;
        send(&mut s, k(KeyCode::Char('i'))).await;
        send(&mut s, k(KeyCode::Char('m'))).await;
        send(&mut s, k(KeyCode::Char('i'))).await;
        send(&mut s, k(KeyCode::Char(' '))).await;
        send(&mut s, k(KeyCode::Char('2'))).await;

        assert_eq!(s.query(), "Kimi 2");
        assert_eq!(s.selected_count(), 0, "space should not select the focused model");
    }

    #[test]
    fn confirm_returns_empty_when_nothing_changed() {
        for (items, selected, reasoning) in [
            (make_items(), None, None),
            (make_items(), Some("anthropic:claude-sonnet-4-5,deepseek:deepseek-chat"), None),
            (make_reasoning_items(), Some("anthropic:claude-opus-4-6"), Some("high")),
        ] {
            let s = sel(items, selected, reasoning);
            assert!(s.confirm().is_empty());
        }
    }

    #[tokio::test]
    async fn confirm_with_one_returns_single_model() {
        let mut s = make_selector();
        send(&mut s, k(KeyCode::Enter)).await;
        let changes = s.confirm();
        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].config_id, "model");
        assert_eq!(changes[0].new_value, "anthropic:claude-sonnet-4-5");
    }

    #[tokio::test]
    async fn confirm_with_two_returns_comma_joined() {
        let mut s = make_selector();
        send(&mut s, k(KeyCode::Enter)).await;
        send(&mut s, k(KeyCode::Down)).await;
        send(&mut s, k(KeyCode::Enter)).await;
        assert_confirm_models(&s.confirm(), &["anthropic:claude-sonnet-4-5", "deepseek:deepseek-chat"]);
    }

    #[test]
    fn pre_selected_values_from_current_selection() {
        let s = sel(make_items(), Some("anthropic:claude-sonnet-4-5,deepseek:deepseek-chat"), None);
        assert_eq!(s.selected_count(), 2);
    }

    #[tokio::test]
    async fn escape_returns_done_action() {
        let mut s = make_selector();
        let msgs = send(&mut s, k(KeyCode::Esc)).await.unwrap();
        match msgs.as_slice() {
            [ModelSelectorMessage::Done(changes)] => assert!(changes.is_empty()),
            other => panic!("expected Done([]), got: {other:?}"),
        }
    }

    #[tokio::test]
    async fn escape_with_selections_returns_done_with_change() {
        let mut s = make_selector();
        send(&mut s, k(KeyCode::Enter)).await;
        send(&mut s, k(KeyCode::Down)).await;
        send(&mut s, k(KeyCode::Enter)).await;

        let msgs = send(&mut s, k(KeyCode::Esc)).await.unwrap();
        match msgs.as_slice() {
            [ModelSelectorMessage::Done(changes)] => {
                assert_confirm_models(changes, &["anthropic:claude-sonnet-4-5", "deepseek:deepseek-chat"]);
            }
            other => panic!("expected Done with model change, got: {other:?}"),
        }
    }

    #[tokio::test]
    async fn escape_after_toggle_returns_change() {
        let mut s = sel(make_items(), Some("anthropic:claude-sonnet-4-5"), None);
        send(&mut s, k(KeyCode::Down)).await;
        send(&mut s, k(KeyCode::Enter)).await;
        assert_confirm_models(&s.confirm(), &["anthropic:claude-sonnet-4-5", "deepseek:deepseek-chat"]);
    }

    #[test]
    fn reasoning_cycle_within_wraps() {
        let levels = &[Low, Medium, High];
        let expected = [(None, Some(Low)), (Some(Low), Some(Medium)), (Some(Medium), Some(High)), (Some(High), None)];
        for (input, output) in expected {
            assert_eq!(ReasoningEffort::cycle_within(input, levels), output);
        }
    }

    #[tokio::test]
    async fn tab_cycles_reasoning_levels() {
        let cases: Vec<(Vec<ModelEntry>, usize, Vec<Option<ReasoningEffort>>)> = vec![
            // 3-level model (first item, no Down needed)
            (make_reasoning_items(), 0, vec![None, Some(Low), Some(Medium), Some(High), None]),
            // 4-level model (Anthropic first, Codex second, need 1 Down)
            (make_mixed_reasoning_items(), 1, vec![None, Some(Low), Some(Medium), Some(High), Some(Xhigh), None]),
        ];
        for (items, downs, expected_sequence) in cases {
            let mut s = sel(items, None, None);
            for _ in 0..downs {
                send(&mut s, k(KeyCode::Down)).await;
            }
            assert_eq!(s.reasoning_effort, expected_sequence[0]);
            for expected in &expected_sequence[1..] {
                send(&mut s, k(KeyCode::Tab)).await;
                assert_eq!(s.reasoning_effort, *expected);
            }
        }
    }

    #[tokio::test]
    async fn tab_on_non_reasoning_model_is_noop() {
        let mut s = sel(make_reasoning_items(), None, None);
        send(&mut s, k(KeyCode::Down)).await;
        assert!(s.combobox.selected().unwrap().reasoning_levels.is_empty());
        send(&mut s, k(KeyCode::Tab)).await;
        assert_eq!(s.reasoning_effort, None);
    }

    #[tokio::test]
    async fn confirm_returns_both_model_and_reasoning_changes() {
        let mut s = sel(make_reasoning_items(), None, None);
        send(&mut s, k(KeyCode::Enter)).await;
        send(&mut s, k(KeyCode::Tab)).await;

        let changes = s.confirm();
        assert_eq!(changes.len(), 2, "expected model + reasoning changes");
        assert!(changes.iter().any(|c| c.config_id == "model"));
        assert!(changes.iter().any(|c| c.config_id == "reasoning_effort" && c.new_value == "low"));
    }

    #[tokio::test]
    async fn confirm_returns_only_reasoning_when_only_reasoning_changed() {
        let mut s = sel(make_reasoning_items(), Some("anthropic:claude-opus-4-6"), None);
        send(&mut s, k(KeyCode::Tab)).await;
        send(&mut s, k(KeyCode::Tab)).await;

        let changes = s.confirm();
        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].config_id, "reasoning_effort");
        assert_eq!(changes[0].new_value, "medium");
    }

    #[tokio::test]
    async fn mouse_scroll_moves_selection() {
        let mut s = make_selector();
        let first = s.combobox.selected().unwrap().value.clone();

        let mouse = |kind| Event::Mouse(MouseEvent { kind, column: 0, row: 0, modifiers: KeyModifiers::NONE });

        let outcome = s.on_event(&mouse(MouseEventKind::ScrollDown)).await;
        assert!(outcome.is_some(), "mouse scroll should be consumed");
        let second = s.combobox.selected().unwrap().value.clone();
        assert_ne!(first, second, "scroll down should move to a different model");

        s.on_event(&mouse(MouseEventKind::ScrollUp)).await;
        let back = s.combobox.selected().unwrap().value.clone();
        assert_eq!(first, back, "scroll up should return to the original model");
    }

    #[tokio::test]
    async fn moving_to_fewer_levels_clamps_xhigh_to_high() {
        let mut s = sel(make_mixed_reasoning_items(), None, None);
        send(&mut s, k(KeyCode::Down)).await; // Move to Codex (4 levels)
        for _ in 0..4 {
            send(&mut s, k(KeyCode::Tab)).await; // Low -> Medium -> High -> Xhigh
        }
        assert_eq!(s.reasoning_effort, Some(Xhigh));

        send(&mut s, k(KeyCode::Up)).await; // Back to Anthropic (3 levels)
        assert_eq!(s.reasoning_effort, Some(High), "xhigh should clamp to high on a 3-level model");
    }

    #[tokio::test]
    async fn focused_item_always_visible_after_scroll() {
        let mut s = sel(many_provider_items(), None, None);
        s.update_viewport(10);

        let ctx = ViewContext::new((80, 10));
        let highlight_bg = ctx.theme.highlight_bg();

        for _ in 0..6 {
            send(&mut s, k(KeyCode::Down)).await;
            let frame = s.render(&ctx);
            let lines = frame.lines();
            assert!(
                lines.iter().any(|l| l.spans().iter().any(|span| span.style().bg == Some(highlight_bg))),
                "focused item must be visible after scrolling down, got: {:?}",
                lines.iter().map(tui::Line::plain_text).collect::<Vec<_>>()
            );
        }
    }

    #[test]
    fn capability_tags_empty_when_no_support() {
        assert_eq!(capability_tags(false, false), "");
    }

    #[test]
    fn capability_tags_image_only() {
        assert_eq!(capability_tags(true, false), "img");
    }

    #[test]
    fn capability_tags_audio_only() {
        assert_eq!(capability_tags(false, true), "audio");
    }

    #[test]
    fn capability_tags_both() {
        assert_eq!(capability_tags(true, true), "img  audio");
    }

    #[test]
    fn focused_row_shows_capability_indicators() {
        let items = vec![ModelEntry {
            value: "anthropic:claude-sonnet-4-5".to_string(),
            name: "Anthropic / Claude Sonnet 4.5".to_string(),
            reasoning_levels: vec![],
            supports_image: true,
            supports_audio: true,
            disabled_reason: None,
        }];
        let mut s = sel(items, None, None);
        let ctx = ViewContext::new((80, 10));
        let frame = s.render(&ctx);
        let text: String = frame.lines().iter().map(tui::Line::plain_text).collect();
        assert!(text.contains("img"), "focused row should show img indicator");
        assert!(text.contains("audio"), "focused row should show audio indicator");
    }

    #[test]
    fn unfocused_row_hides_capability_indicators() {
        let items = vec![
            entry("a:m1", "A / M1", vec![]),
            ModelEntry {
                value: "b:m2".to_string(),
                name: "B / M2".to_string(),
                reasoning_levels: vec![],
                supports_image: true,
                supports_audio: true,
                disabled_reason: None,
            },
        ];
        let mut s = sel(items, None, None);
        let ctx = ViewContext::new((80, 10));
        let frame = s.render(&ctx);
        for line in frame.lines() {
            let text = line.plain_text();
            if text.contains("M2") {
                assert!(!text.contains("img"), "unfocused row should not show img");
                assert!(!text.contains("audio"), "unfocused row should not show audio");
            }
        }
    }

    #[tokio::test]
    async fn disabled_entry_not_toggleable() {
        let items = vec![entry("a:m1", "A / M1", vec![]), disabled_entry("b:m2", "B / M2", "set B_API_KEY")];
        let mut s = sel(items, None, None);
        send(&mut s, k(KeyCode::Enter)).await;
        assert_eq!(s.selected_count(), 1);

        send(&mut s, k(KeyCode::Down)).await;
        let focused = s.focused_value().unwrap();
        assert_eq!(focused, "a:m1", "navigation should wrap back to enabled entry");
        send(&mut s, k(KeyCode::Enter)).await;
        assert_eq!(s.selected_count(), 0, "toggling the same enabled entry should deselect it");
    }

    #[tokio::test]
    async fn navigation_skips_disabled_entries() {
        let items = vec![
            entry("a:m1", "A / M1", vec![]),
            disabled_entry("b:m2", "B / M2", "set B_API_KEY"),
            entry("c:m3", "C / M3", vec![]),
        ];
        let mut s = sel(items, None, None);
        assert_eq!(s.focused_value().unwrap(), "a:m1");

        send(&mut s, k(KeyCode::Down)).await;
        assert_eq!(s.focused_value().unwrap(), "c:m3", "should skip disabled entry");

        send(&mut s, k(KeyCode::Up)).await;
        assert_eq!(s.focused_value().unwrap(), "a:m1", "should skip disabled entry going up");
    }

    #[test]
    fn disabled_entry_renders_with_reason() {
        let items = vec![entry("a:m1", "A / M1", vec![]), disabled_entry("b:m2", "B / M2", "set B_API_KEY")];
        let mut s = sel(items, None, None);
        let ctx = ViewContext::new((80, 10));
        let frame = s.render(&ctx);
        let lines: Vec<String> = frame.lines().iter().map(tui::Line::plain_text).collect();
        let disabled_line = lines.iter().find(|l| l.contains("M2")).expect("disabled entry should be rendered");
        assert!(disabled_line.contains("set B_API_KEY"), "should show reason: {disabled_line}");
        assert!(!disabled_line.contains('['), "disabled entry should not have checkbox: {disabled_line}");
    }

    #[test]
    fn provider_key_handles_unavailable_prefix() {
        let e = disabled_entry("__unavailable:moonshot", "Moonshot (5 models)", "set MOONSHOT_API_KEY");
        assert_eq!(e.provider_key(), "moonshot");
    }

    #[test]
    fn initial_selection_skips_disabled() {
        let items = vec![disabled_entry("a:m1", "A / M1", "set A_API_KEY"), entry("b:m2", "B / M2", vec![])];
        let s = sel(items, None, None);
        assert_eq!(s.focused_value().unwrap(), "b:m2");
    }
}
