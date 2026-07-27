use super::{App, RuntimeEffect};
use crate::session_config_view::{SessionConfigView, as_select};
use crate::settings::UiSettings;
use crate::settings_overlay::{SettingsMenuEntry, SettingsMenuValue};
use crate::theme::Theme;
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
    /// Themes are persisted and parsed by the task runner. Only one change runs
    /// at a time, because two racing saves can finish in either order and leave
    /// both the presenter and the settings file on a choice the user moved past.
    pub(super) fn apply_theme_change(&mut self, value: &str) {
        self.apply_settings_change(&crate::settings_overlay::SettingsChange {
            config_id: acp_utils::config_option_id::THEME_CONFIG_ID.to_string(),
            new_value: value.to_string(),
        });
        if self.theme_change.in_flight {
            self.theme_change.queued = Some(value.to_string());
        } else {
            self.spawn_theme_change(value.to_string());
        }
    }

    /// Adopts a finished theme change, unless the user has since chosen another
    /// one — that choice starts now, so it is the one that lands last.
    pub(super) fn finish_theme_change(&mut self, settings: UiSettings, theme: Theme, error: Option<String>) {
        self.theme_change.in_flight = false;
        if let Some(error) = error {
            self.notify(&format!("Failed to save theme settings: {error}"));
        }
        if let Some(value) = self.theme_change.queued.take() {
            self.spawn_theme_change(value);
            return;
        }
        self.ui.settings = settings;
        self.effects.push_back(RuntimeEffect::SetTheme(theme));
    }

    fn spawn_theme_change(&mut self, value: String) {
        let mut settings = self.ui.settings.clone();
        settings.theme.file = (!value.is_empty()).then(|| value.clone());
        self.theme_change.in_flight = true;
        self.spawn(crate::tasks::Task::ApplyTheme { settings, value });
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
