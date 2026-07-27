use super::App;
use crate::session_config_view::{SessionConfigView, as_select};
use crate::settings::UiSettings;
use crate::settings_overlay::{SettingsMenuEntry, SettingsMenuValue};
use acp_utils::config_option_id::ConfigOptionId;
use agent_client_protocol::schema::{self as acp};
use utils::ReasoningEffort;

pub(super) fn cycle_quick_option(config_options: &[acp::SessionConfigOption]) -> Option<(String, String)> {
    let (id, value) = SessionConfigView::new(config_options).next_mode()?;
    Some((id.to_string(), value.to_string()))
}

pub(super) fn cycle_reasoning_option(config_options: &[acp::SessionConfigOption]) -> Option<(String, String)> {
    let view = SessionConfigView::new(config_options);
    let levels = view.reasoning_levels();
    if levels.is_empty() {
        return None;
    }
    let next = ReasoningEffort::cycle_within(view.reasoning_effort(), &levels);
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
        .filter_map(|option| Some((option.id.0.to_string(), as_select(option)?.current_value.0.to_string())))
        .collect()
}

impl App {
    /// Themes are persisted and parsed by the task runner.
    pub(super) fn apply_theme_change(&mut self, value: &str) {
        let mut settings = self.ui.settings.clone();
        settings.theme.file = (!value.is_empty()).then(|| value.to_string());
        self.spawn(crate::tasks::Task::ApplyTheme { settings, value: value.to_string() });
        self.apply_settings_change(&crate::settings_overlay::SettingsChange {
            config_id: acp_utils::config_option_id::THEME_CONFIG_ID.to_string(),
            new_value: value.to_string(),
        });
    }
}

pub(super) fn build_theme_entries(settings: &UiSettings, files: &[String]) -> Vec<SettingsMenuEntry> {
    use acp_utils::config_meta::SelectOptionMeta;
    use acp_utils::config_option_id::THEME_CONFIG_ID;

    let mut values: Vec<SettingsMenuValue> = Vec::new();

    values.push(SettingsMenuValue {
        value: String::new(),
        name: "Default".to_string(),
        description: Some("Built-in Nord theme".to_string()),
        is_disabled: false,
        meta: SelectOptionMeta::default(),
    });

    for file in files {
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
        local: true,
        multi_select: false,
        display_name: None,
    }]
}
