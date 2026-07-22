use super::{
    App, ConfigOptionId, ReasoningEffort, SessionConfigOptionCategory, SessionConfigView, SettingsMenuEntry,
    SettingsMenuEntryKind, SettingsMenuValue, UiSettings, acp,
};
pub(super) fn is_cycleable_mode_option(option: &acp::SessionConfigOption) -> bool {
    matches!(option.kind, acp::SessionConfigKind::Select(_))
        && option.category == Some(SessionConfigOptionCategory::Mode)
}

pub(super) fn cycle_quick_option(config_options: &[acp::SessionConfigOption]) -> Option<(String, String)> {
    let option = config_options.iter().find(|option| is_cycleable_mode_option(option))?;

    let acp::SessionConfigKind::Select(ref select) = option.kind else {
        return None;
    };

    let acp::SessionConfigSelectOptions::Ungrouped(ref options) = select.options else {
        return None;
    };

    if options.is_empty() {
        return None;
    }

    let current_index = options.iter().position(|entry| entry.value == select.current_value).unwrap_or(0);
    let next_index = (current_index + 1) % options.len();
    options.get(next_index).map(|next| (option.id.0.to_string(), next.value.0.to_string()))
}

pub(super) fn extract_reasoning_levels(config_options: &[acp::SessionConfigOption]) -> Vec<ReasoningEffort> {
    SessionConfigView::new(config_options).reasoning_levels()
}

pub(super) fn extract_reasoning_effort(config_options: &[acp::SessionConfigOption]) -> Option<ReasoningEffort> {
    SessionConfigView::new(config_options).reasoning_effort()
}

pub(super) fn cycle_reasoning_option(config_options: &[acp::SessionConfigOption]) -> Option<(String, String)> {
    let levels = extract_reasoning_levels(config_options);
    if levels.is_empty() {
        return None;
    }

    let current = extract_reasoning_effort(config_options);
    let next = ReasoningEffort::cycle_within(current, &levels);
    Some((ConfigOptionId::ReasoningEffort.as_str().to_string(), ReasoningEffort::config_str(next).to_string()))
}

pub(super) fn update_config_option_value(options: &mut [acp::SessionConfigOption], config_id: &str, value: &str) {
    let Some(option) = options.iter_mut().find(|option| option.id.0.as_ref() == config_id) else {
        return;
    };
    let acp::SessionConfigKind::Select(select) = &mut option.kind else {
        return;
    };
    select.current_value = value.to_string().into();
}

pub(super) fn extract_config_selections(config_options: &[acp::SessionConfigOption]) -> Vec<(String, String)> {
    config_options
        .iter()
        .filter_map(|option| {
            if let acp::SessionConfigKind::Select(ref select) = option.kind {
                Some((option.id.0.to_string(), select.current_value.0.to_string()))
            } else {
                None
            }
        })
        .collect()
}

impl App {
    pub fn take_pending_theme(&mut self) -> Option<crate::theme::Theme> {
        self.pending_theme.take()
    }

    pub(super) fn apply_theme_change(&mut self, value: &str) {
        let file = if value.is_empty() { None } else { Some(value.to_string()) };
        self.ui_settings.theme.file = file;
        if let Err(e) = crate::settings::save_settings(&self.ui_settings) {
            tracing::warn!("Failed to save theme settings: {e}");
        }
        let theme =
            if value.is_empty() { crate::theme::Theme::default() } else { crate::settings::load_theme_file(value) };
        self.pending_theme = Some(theme);
        if let Some(overlay) = &mut self.settings_overlay {
            overlay.apply_change(&crate::settings_overlay::SettingsChange {
                config_id: acp_utils::config_option_id::THEME_CONFIG_ID.to_string(),
                new_value: value.to_string(),
            });
        }
    }
}

pub(super) fn build_theme_entries(settings: &UiSettings) -> Vec<SettingsMenuEntry> {
    use acp_utils::config_meta::SelectOptionMeta;
    use acp_utils::config_option_id::THEME_CONFIG_ID;

    let files = crate::settings::list_theme_files();
    let mut values: Vec<SettingsMenuValue> = Vec::new();

    values.push(SettingsMenuValue {
        value: String::new(),
        name: "Default".to_string(),
        description: Some("Built-in Nord theme".to_string()),
        is_disabled: false,
        meta: SelectOptionMeta::default(),
    });

    for file in &files {
        let display = file.trim_end_matches(".tmTheme").to_string();
        values.push(SettingsMenuValue {
            value: file.clone(),
            name: display,
            description: None,
            is_disabled: false,
            meta: SelectOptionMeta::default(),
        });
    }

    let current_file = settings.theme.file.as_deref().unwrap_or("");
    let current_value_index =
        if current_file.is_empty() { 0 } else { values.iter().position(|v| v.value == current_file).unwrap_or(0) };

    vec![SettingsMenuEntry {
        config_id: THEME_CONFIG_ID.to_string(),
        title: "Theme".to_string(),
        values,
        current_value_index,
        current_raw_value: current_file.to_string(),
        entry_kind: SettingsMenuEntryKind::Theme,
        multi_select: false,
        display_name: None,
    }]
}
