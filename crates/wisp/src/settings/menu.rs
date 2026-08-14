pub use super::types::{SettingsChange, SettingsMenuEntry, SettingsMenuEntryKind, SettingsMenuValue};
use acp_utils::config_meta::{ConfigOptionMeta, SelectOptionMeta};
use acp_utils::config_option_id::{ConfigOptionId, THEME_CONFIG_ID};
use agent_client_protocol::schema::v1::{SessionConfigKind, SessionConfigOption, SessionConfigSelectOptions};
use tui::{Component, Event, Frame, Line, SelectItem, SelectList, SelectListMessage, ViewContext};

pub struct SettingsMenu {
    list: SelectList<SettingsMenuEntry>,
}

pub enum SettingMenuMessage {
    CloseAll,
    OpenSelectedPicker,
    OpenMcpServers,
    OpenProviderLogins,
    OpenModelSelector,
}

impl SelectItem for SettingsMenuEntry {
    fn render_item(&self, selected: bool, ctx: &ViewContext) -> Line {
        let current_name = self
            .display_name
            .as_deref()
            .or_else(|| self.values.get(self.current_value_index).map(|v| v.name.as_str()))
            .unwrap_or("?");
        let current_disabled =
            self.display_name.is_none() && self.values.get(self.current_value_index).is_some_and(|v| v.is_disabled);
        let text = format!("{}: {}", self.title, current_name);
        if current_disabled {
            Line::styled(text, ctx.theme.muted())
        } else if selected {
            Line::with_style(text, ctx.theme.selected_row_style())
        } else {
            Line::new(text)
        }
    }
}

impl Component for SettingsMenu {
    type Message = SettingMenuMessage;

    async fn on_event(&mut self, event: &Event) -> Option<Vec<Self::Message>> {
        let outcome = self.list.on_event(event).await;
        match outcome.as_deref() {
            Some([SelectListMessage::Close]) => Some(vec![SettingMenuMessage::CloseAll]),
            Some([SelectListMessage::Select(_)]) => {
                let msg = match self.list.selected_item() {
                    Some(e) if e.entry_kind == SettingsMenuEntryKind::McpServers => SettingMenuMessage::OpenMcpServers,
                    Some(e) if e.entry_kind == SettingsMenuEntryKind::ProviderLogins => {
                        SettingMenuMessage::OpenProviderLogins
                    }
                    Some(e) if e.multi_select => SettingMenuMessage::OpenModelSelector,
                    _ => SettingMenuMessage::OpenSelectedPicker,
                };
                Some(vec![msg])
            }
            _ => outcome.map(|_| vec![]),
        }
    }

    fn render(&mut self, context: &ViewContext) -> Frame {
        self.list.render(context)
    }
}

impl SettingsMenu {
    pub fn from_config_options(options: &[SessionConfigOption]) -> Self {
        let entries: Vec<SettingsMenuEntry> = options
            .iter()
            .filter(|opt| opt.id.0.as_ref() != ConfigOptionId::ReasoningEffort.as_str())
            .filter_map(|opt| {
                let SessionConfigKind::Select(ref select) = opt.kind else {
                    return None;
                };

                let flat_options = match &select.options {
                    SessionConfigSelectOptions::Ungrouped(opts) => opts.clone(),
                    SessionConfigSelectOptions::Grouped(groups) => {
                        groups.iter().flat_map(|g| g.options.clone()).collect()
                    }
                    _ => return None,
                };

                if flat_options.is_empty() {
                    return None;
                }

                let current_value_index =
                    flat_options.iter().position(|o| o.value == select.current_value).unwrap_or(0);

                let values: Vec<SettingsMenuValue> = flat_options
                    .into_iter()
                    .map(|o| SettingsMenuValue {
                        value: o.value.0.to_string(),
                        name: o.name,
                        is_disabled: o.description.as_deref().is_some_and(|d| d.starts_with("Unavailable:")),
                        description: o.description,
                        meta: SelectOptionMeta::from_meta(o.meta.as_ref()),
                    })
                    .collect();

                let multi_select = ConfigOptionMeta::from_meta(opt.meta.as_ref()).multi_select;

                let display_name = if multi_select && select.current_value.0.contains(',') {
                    let parts: Vec<&str> = select.current_value.0.split(',').map(str::trim).collect();

                    let names: Vec<&str> = parts
                        .iter()
                        .filter_map(|val| values.iter().find(|v| v.value == *val).map(|v| v.name.as_str()))
                        .collect();

                    if names.is_empty() { Some(format!("{} models", parts.len())) } else { Some(names.join(", ")) }
                } else {
                    None
                };

                Some(SettingsMenuEntry {
                    config_id: opt.id.0.to_string(),
                    title: opt.name.clone(),
                    values,
                    current_value_index,
                    current_raw_value: select.current_value.0.to_string(),
                    entry_kind: SettingsMenuEntryKind::Select,
                    multi_select,
                    display_name,
                })
            })
            .collect();

        Self { list: SelectList::new(entries, "no settings options") }
    }

    #[allow(dead_code)] // Used by integration tests
    pub fn from_entries(entries: Vec<SettingsMenuEntry>) -> Self {
        Self { list: SelectList::new(entries, "no settings options") }
    }

    #[cfg(test)]
    pub fn options(&self) -> &[SettingsMenuEntry] {
        self.list.items()
    }

    #[cfg(test)]
    pub fn selected_index(&self) -> usize {
        self.list.selected_index()
    }

    pub fn add_theme_entry(&mut self, current_theme_file: Option<&str>, theme_files: &[String]) {
        let mut values = Vec::with_capacity(theme_files.len() + 1);
        values.push(SettingsMenuValue {
            value: String::new(),
            name: "Default".to_string(),
            description: None,
            is_disabled: false,
            meta: SelectOptionMeta::default(),
        });

        values.extend(theme_files.iter().map(|file| SettingsMenuValue {
            value: file.clone(),
            name: file.clone(),
            description: None,
            is_disabled: false,
            meta: SelectOptionMeta::default(),
        }));

        let current_value_index =
            current_theme_file.and_then(|file| values.iter().position(|v| v.value == file)).unwrap_or(0);
        let current_raw_value = values.get(current_value_index).map(|v| v.value.clone()).unwrap_or_default();

        self.list.push(SettingsMenuEntry {
            config_id: THEME_CONFIG_ID.to_string(),
            title: "Theme".to_string(),
            values,
            current_value_index,
            current_raw_value,
            entry_kind: SettingsMenuEntryKind::Select,
            multi_select: false,
            display_name: None,
        });
    }

    pub fn add_mcp_servers_entry(&mut self, summary: &str) {
        self.list.push(SettingsMenuEntry {
            config_id: "__mcp_servers".to_string(),
            title: "MCP Servers".to_string(),
            values: vec![SettingsMenuValue {
                value: String::new(),
                name: summary.to_string(),
                description: None,
                is_disabled: false,
                meta: SelectOptionMeta::default(),
            }],
            current_value_index: 0,
            current_raw_value: String::new(),
            entry_kind: SettingsMenuEntryKind::McpServers,
            multi_select: false,
            display_name: None,
        });
    }

    pub fn upsert_mcp_servers_entry(&mut self, summary: &str) {
        if let Some(entry) =
            self.list.items_mut().iter_mut().find(|entry| entry.entry_kind == SettingsMenuEntryKind::McpServers)
        {
            if let Some(value) = entry.values.first_mut() {
                value.name = summary.to_string();
            }
            return;
        }

        self.add_mcp_servers_entry(summary);
    }

    pub fn add_provider_logins_entry(&mut self, summary: &str) {
        self.list.push(SettingsMenuEntry {
            config_id: "__provider_logins".to_string(),
            title: "Provider Logins".to_string(),
            values: vec![SettingsMenuValue {
                value: String::new(),
                name: summary.to_string(),
                description: None,
                is_disabled: false,
                meta: SelectOptionMeta::default(),
            }],
            current_value_index: 0,
            current_raw_value: String::new(),
            entry_kind: SettingsMenuEntryKind::ProviderLogins,
            multi_select: false,
            display_name: None,
        });
    }

    pub fn update_options(&mut self, options: &[SessionConfigOption]) {
        let prev_index = self.list.selected_index();
        *self = Self::from_config_options(options);
        let max = self.list.len().saturating_sub(1);
        self.list.set_selected(prev_index.min(max));
    }

    pub fn selected_entry(&self) -> Option<&SettingsMenuEntry> {
        self.list.selected_item()
    }

    pub fn apply_change(&mut self, change: &SettingsChange) {
        let Some(entry) = self.list.items_mut().iter_mut().find(|entry| entry.config_id == change.config_id) else {
            return;
        };

        entry.current_raw_value.clone_from(&change.new_value);
        if let Some(index) = entry.values.iter().position(|value| value.value == change.new_value) {
            entry.current_value_index = index;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_client_protocol::schema::v1::{
        SessionConfigOption, SessionConfigOptionCategory, SessionConfigSelectOption,
    };
    use tui::{KeyCode, KeyEvent, KeyModifiers};

    fn sel(id: &str, name: &str, current: &str, values: &[(&str, &str)]) -> SessionConfigOption {
        let options: Vec<SessionConfigSelectOption> =
            values.iter().map(|(v, n)| SessionConfigSelectOption::new((*v).to_string(), (*n).to_string())).collect();
        SessionConfigOption::select(id.to_string(), name.to_string(), current.to_string(), options)
    }

    fn menu(opts: &[SessionConfigOption]) -> SettingsMenu {
        SettingsMenu::from_config_options(opts)
    }

    fn key(code: KeyCode) -> Event {
        Event::Key(KeyEvent::new(code, KeyModifiers::NONE))
    }

    async fn press(menu: &mut SettingsMenu, code: KeyCode) -> Option<Vec<SettingMenuMessage>> {
        menu.on_event(&key(code)).await
    }

    fn theme_files() -> Vec<String> {
        vec!["sage.tmTheme".into(), "nord.tmTheme".into()]
    }

    fn theme_menu(current: Option<&str>) -> SettingsMenu {
        let mut m = menu(&[]);
        m.add_theme_entry(current, &theme_files());
        m
    }

    #[test]
    fn from_config_options_builds_entries() {
        let m = menu(&[
            sel("model", "Model", "gpt-4o", &[("gpt-4o", "GPT-4o"), ("claude", "Claude")]),
            sel("mode", "Mode", "code", &[("code", "Code"), ("chat", "Chat")]),
        ]);
        assert_eq!(m.options().len(), 2);
        assert_eq!(m.options()[0].config_id, "model");
        assert_eq!(m.options()[0].current_value_index, 0);
        assert_eq!(m.options()[0].display_name, None);
        assert_eq!(m.options()[1].config_id, "mode");
    }

    #[test]
    fn from_config_options_finds_current_value() {
        let m =
            menu(&[sel("model", "Model", "claude", &[("gpt-4o", "GPT-4o"), ("claude", "Claude"), ("llama", "Llama")])]);
        assert_eq!(m.options()[0].current_value_index, 1);
    }

    #[tokio::test]
    async fn navigation_wraps_around() {
        let mut m = menu(&[
            sel("a", "A", "v1", &[("v1", "V1")]),
            sel("b", "B", "v1", &[("v1", "V1")]),
            sel("c", "C", "v1", &[("v1", "V1")]),
        ]);
        assert_eq!(m.selected_index(), 0);

        press(&mut m, KeyCode::Up).await;
        assert_eq!(m.selected_index(), 2);

        press(&mut m, KeyCode::Down).await;
        assert_eq!(m.selected_index(), 0);

        for _ in 0..3 {
            press(&mut m, KeyCode::Down).await;
        }
        assert_eq!(m.selected_index(), 0);
    }

    #[test]
    fn update_options_clamps_index() {
        let mut m = menu(&[
            sel("a", "A", "v1", &[("v1", "V1")]),
            sel("b", "B", "v1", &[("v1", "V1")]),
            sel("c", "C", "v1", &[("v1", "V1")]),
        ]);
        m.list.set_selected(2);
        m.update_options(&[sel("a", "A", "v1", &[("v1", "V1")])]);
        assert_eq!(m.selected_index(), 0);
    }

    #[test]
    fn update_options_preserves_index_when_within_bounds() {
        let mut m = menu(&[
            sel("provider", "Provider", "a", &[("a", "A"), ("b", "B")]),
            sel("model", "Model", "m1", &[("m1", "M1"), ("m2", "M2")]),
        ]);
        m.list.set_selected(1);
        m.update_options(&[
            sel("provider", "Provider", "b", &[("a", "A"), ("b", "B")]),
            sel("model", "Model", "m3", &[("m3", "M3")]),
        ]);
        assert_eq!(m.selected_index(), 1);
    }

    #[test]
    fn from_config_options_skips_empty_values() {
        let empty = SessionConfigOption::select("x", "X", "v", Vec::<SessionConfigSelectOption>::new());
        let m = menu(&[empty, sel("model", "Model", "a", &[("a", "A")])]);
        assert_eq!(m.options().len(), 1);
        assert_eq!(m.options()[0].config_id, "model");
    }

    #[test]
    fn from_config_options_with_category() {
        let opt = sel("model", "Model", "gpt-4o", &[("gpt-4o", "GPT-4o")]).category(SessionConfigOptionCategory::Model);
        let m = menu(&[opt]);
        assert_eq!(m.options().len(), 1);
        assert_eq!(m.options()[0].title, "Model");
    }

    #[test]
    fn upsert_mcp_servers_entry_adds_when_missing() {
        let mut m = menu(&[]);
        m.upsert_mcp_servers_entry("none");

        let entries = m
            .options()
            .iter()
            .filter(|entry| entry.entry_kind == SettingsMenuEntryKind::McpServers)
            .collect::<Vec<_>>();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].values[0].name, "none");
    }

    #[test]
    fn upsert_mcp_servers_entry_updates_existing_without_duplicate() {
        let mut m = menu(&[]);
        m.upsert_mcp_servers_entry("1 connected");
        m.upsert_mcp_servers_entry("none");

        let entries = m
            .options()
            .iter()
            .filter(|entry| entry.entry_kind == SettingsMenuEntryKind::McpServers)
            .collect::<Vec<_>>();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].values[0].name, "none");
    }

    #[tokio::test]
    async fn key_enter_opens_picker_and_escape_closes() {
        for (code, expected) in [(KeyCode::Enter, "OpenSelectedPicker"), (KeyCode::Esc, "CloseAll")] {
            let mut m = menu(&[sel("model", "Model", "a", &[("a", "A")])]);
            let msgs = press(&mut m, code).await.unwrap();
            let tag = match &msgs[..] {
                [SettingMenuMessage::OpenSelectedPicker] => "OpenSelectedPicker",
                [SettingMenuMessage::CloseAll] => "CloseAll",
                _ => "other",
            };
            assert_eq!(tag, expected, "key {code:?} should produce {expected}");
        }
    }

    #[test]
    fn multi_select_detected_from_meta() {
        for (has_meta, expected) in [(true, true), (false, false)] {
            let mut opt = sel("model", "Model", "a", &[("a", "A"), ("b", "B")]);
            if has_meta {
                opt = opt.meta(ConfigOptionMeta { multi_select: true }.into_meta());
            }
            let m = menu(&[opt]);
            assert_eq!(m.options()[0].multi_select, expected, "meta={has_meta}");
        }
    }

    #[tokio::test]
    async fn multi_select_entry_opens_model_selector() {
        let opt = sel("model", "Model", "a", &[("a", "A"), ("b", "B")])
            .meta(ConfigOptionMeta { multi_select: true }.into_meta());
        let mut m = menu(&[opt]);
        let msgs = press(&mut m, KeyCode::Enter).await.unwrap();
        assert!(matches!(msgs.as_slice(), [SettingMenuMessage::OpenModelSelector]));
    }

    #[test]
    fn multi_select_with_comma_value_shows_model_names() {
        let opt = sel("model", "Model", "a,b", &[("a", "Alpha"), ("b", "Beta")])
            .meta(ConfigOptionMeta { multi_select: true }.into_meta());
        let display = menu(&[opt]).options()[0].display_name.as_deref().unwrap().to_string();
        assert!(display.contains("Alpha"), "display: {display}");
        assert!(display.contains("Beta"), "display: {display}");
    }

    #[test]
    fn apply_change_updates_matching_entry_value_and_index() {
        let mut m = theme_menu(None);
        m.apply_change(&SettingsChange {
            config_id: THEME_CONFIG_ID.to_string(),
            new_value: "nord.tmTheme".to_string(),
        });
        assert_eq!(m.options()[0].current_raw_value, "nord.tmTheme");
        assert_eq!(m.options()[0].current_value_index, 2);
    }

    #[test]
    fn add_theme_entry_inserts_theme_row() {
        let m = theme_menu(None);
        assert_eq!(m.options().len(), 1);
        let t = &m.options()[0];
        assert_eq!(t.config_id, THEME_CONFIG_ID);
        assert_eq!(t.title, "Theme");
        assert_eq!(t.entry_kind, SettingsMenuEntryKind::Select);
        assert!(!t.multi_select);
        assert_eq!(t.values.len(), 3);
        assert_eq!(t.values[0].name, "Default");
        assert_eq!(t.values[0].value, "");
        assert_eq!(t.values[1].value, "sage.tmTheme");
        assert_eq!(t.values[2].value, "nord.tmTheme");
    }

    #[test]
    fn add_theme_entry_selects_correct_index() {
        let cases: &[(Option<&str>, usize, &str)] =
            &[(None, 0, ""), (Some("nord.tmTheme"), 2, "nord.tmTheme"), (Some("missing.tmTheme"), 0, "")];
        for &(current, expected_idx, expected_raw) in cases {
            let m = theme_menu(current);
            let t = &m.options()[0];
            assert_eq!(t.current_value_index, expected_idx, "current={current:?}");
            assert_eq!(t.current_raw_value, expected_raw, "current={current:?}");
        }
    }

    #[test]
    fn from_config_options_excludes_reasoning_effort_entry() {
        let m = menu(&[
            sel("model", "Model", "gpt-4o", &[("gpt-4o", "GPT-4o"), ("claude", "Claude")]),
            sel(
                "reasoning_effort",
                "Reasoning Effort",
                "high",
                &[("none", "None"), ("low", "Low"), ("medium", "Medium"), ("high", "High")],
            ),
        ]);
        assert!(m.options().iter().any(|e| e.config_id == "model"));
        assert!(!m.options().iter().any(|e| e.config_id == "reasoning_effort"));
    }
}
