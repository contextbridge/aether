use super::App;
use crate::command::{Command, FilesystemCommand};
use crate::session::session_config_view::{LocalConfigOption, LocalConfigView};
use crate::settings::UiSettings;
use crate::settings::overlay::{SettingsChange, SettingsMenuEntry, SettingsMenuValue};
use crate::theme::Theme;
use acp_utils::config_option_id::ConfigOptionId;
use utils::ReasoningEffort;

pub(super) fn cycle_reasoning_option(config_options: &[LocalConfigOption]) -> Option<(String, String)> {
    let view = LocalConfigView::new(config_options);
    let levels = view.reasoning_levels();
    if levels.is_empty() {
        return None;
    }
    let next = ReasoningEffort::cycle_within(view.reasoning_effort(), &levels);
    Some((ConfigOptionId::ReasoningEffort.as_str().to_string(), ReasoningEffort::config_str(next).to_string()))
}

impl App {
    /// Themes are persisted and parsed by the task runner. Only one change runs
    /// at a time, because two racing saves can finish in either order and leave
    /// both the renderer and the settings file on a choice the user moved past.
    pub(super) fn apply_theme_change(&mut self, value: &str) {
        self.apply_settings_change(&SettingsChange {
            config_id: acp_utils::config_option_id::THEME_CONFIG_ID.to_string(),
            new_value: value.to_string(),
        });
        if let Some(request) = self.ui.settings.request_theme_change(value.to_string()) {
            self.queue(Command::Filesystem(FilesystemCommand::ApplyTheme {
                settings: Box::new(request.settings),
                value: request.value,
            }));
        }
    }

    /// Adopts a finished theme change, unless the user has since chosen another
    /// one — that choice starts now, so it is the one that lands last.
    pub(super) fn finish_theme_change(&mut self, settings: Box<UiSettings>, theme: Theme, error: Option<String>) {
        if let Some(error) = error {
            self.notify(&format!("Failed to save theme settings: {error}"));
        }
        if let Some(request) = self.ui.settings.finish_theme_change(*settings) {
            self.queue(Command::Filesystem(FilesystemCommand::ApplyTheme {
                settings: Box::new(request.settings),
                value: request.value,
            }));
            return;
        }
        self.ui.theme = theme;
        self.ui.theme_generation.bump();
    }
}

pub(super) fn build_theme_entries(settings: &UiSettings, files: &[String]) -> Vec<SettingsMenuEntry> {
    use acp_utils::config_meta::SelectOptionMeta;
    use acp_utils::config_option_id::THEME_CONFIG_ID;

    let mut values: Vec<SettingsMenuValue> = Vec::new();

    values.push(SettingsMenuValue {
        value: String::new(),
        name: "Default".to_string(),
        group: None,
        description: Some("Built-in Sage theme".to_string()),
        is_disabled: false,
        meta: SelectOptionMeta::default(),
    });

    for file in files {
        let display = file.trim_end_matches(".tmTheme").to_string();
        values.push(SettingsMenuValue {
            value: file.clone(),
            name: display,
            group: None,
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
