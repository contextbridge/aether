use super::App;
use crate::command::{Command, HostCommand};
use client_core::session_config_view::{LocalConfigOption, LocalConfigValue, LocalConfigView};
use crate::settings::overlay::{SettingsChange, SettingsMenuEntry};
use crate::settings::{ThemeSettings, UiSettings};
use crate::theme::{Theme, ThemeApplicationError};
use acp_utils::config_option_id::ConfigOptionId;
use clankerdiff_ratatui::theme::ReviewTheme;
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
    /// Themes are loaded and persisted by the task runner. Only one change runs
    /// at a time, because two racing saves can finish in either order and leave
    /// both the renderer and the settings file on a choice the user moved past.
    pub(super) fn apply_theme_change(&mut self, value: &str) {
        let selection = match ThemeSettings::from_selection_id(value) {
            Ok(selection) => selection,
            Err(error) => {
                self.notify(&format!("Invalid theme selection: {error}"));
                return;
            }
        };
        if let Some(settings) = self.ui.settings.request_theme_change(selection) {
            self.queue(Command::Host(HostCommand::ApplyTheme { settings: Box::new(settings) }));
        }
    }

    pub(super) fn finish_theme_change(&mut self, result: Result<(Box<UiSettings>, Theme), ThemeApplicationError>) {
        let settings = match result {
            Ok((settings, theme)) => {
                self.ui.theme = theme;
                Some(*settings)
            }
            Err(error) => {
                self.notify(&format!("Failed to apply theme: {error}"));
                None
            }
        };
        self.ui.theme_generation.bump();
        if let Some(settings) = self.ui.settings.finish_theme_change(settings) {
            self.queue(Command::Host(HostCommand::ApplyTheme { settings: Box::new(settings) }));
        }
        self.apply_settings_change(&SettingsChange {
            config_id: acp_utils::config_option_id::THEME_CONFIG_ID.to_string(),
            new_value: self.ui.settings.ui().theme.selection_id(),
        });
    }
}

pub(super) fn build_theme_entries(settings: &UiSettings, files: &[String]) -> Vec<SettingsMenuEntry> {
    use acp_utils::config_option_id::THEME_CONFIG_ID;

    let builtin = ReviewTheme::catalog().into_iter().map(|descriptor| LocalConfigValue {
        value: format!("builtin:{}", descriptor.id),
        name: descriptor.name,
        description: Some("Built-in theme".into()),
        ..LocalConfigValue::default()
    });
    let custom = files.iter().map(|file| LocalConfigValue {
        value: format!("file:{file}"),
        name: file.trim_end_matches(".json").to_string(),
        ..LocalConfigValue::default()
    });
    let values: Vec<LocalConfigValue> = builtin.chain(custom).collect();

    let current_file = settings.theme.selection_id();

    vec![SettingsMenuEntry {
        config_id: THEME_CONFIG_ID.to_string(),
        title: "Theme".to_string(),
        values,
        current_raw_value: current_file,
        local: true,
        multi_select: false,
    }]
}
