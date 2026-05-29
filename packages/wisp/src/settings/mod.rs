#![doc = include_str!("../docs/settings_module.md")]

pub mod menu;
pub mod overlay;
pub(crate) mod picker;
pub mod types;

use crate::components::provider_login::{ProviderLoginEntry, ProviderLoginStatus, provider_login_summary};
use crate::components::server_status::server_status_summary;
use acp_utils::notifications::McpServerStatusEntry;
use acp_utils::settings::SettingsStore;
use agent_client_protocol::schema::{AuthMethod, SessionConfigOption};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use tracing::warn;

#[cfg(test)]
pub(crate) static WISP_HOME_ENV_MUTEX: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[doc = include_str!("../docs/wisp_settings.md")]
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct WispSettings {
    #[serde(default)]
    pub theme: ThemeSettings,
    #[serde(default)]
    pub content_padding: Option<u16>,
}

pub const DEFAULT_CONTENT_PADDING: usize = 2;

pub fn resolve_content_padding(settings: &WispSettings) -> usize {
    settings.content_padding.map_or(DEFAULT_CONTENT_PADDING, |v| v.max(2) as usize)
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ThemeSettings {
    #[serde(default)]
    pub file: Option<String>,
}

pub fn wisp_home() -> Option<PathBuf> {
    Some(SettingsStore::new("WISP_HOME", ".wisp")?.home().to_path_buf())
}

pub fn themes_dir_path() -> Option<PathBuf> {
    Some(wisp_home()?.join("themes"))
}

pub fn load_or_create_settings() -> WispSettings {
    if let Some(store) = SettingsStore::new("WISP_HOME", ".wisp") {
        store.load_or_create()
    } else {
        warn!("Unable to resolve Wisp settings path; using defaults");
        WispSettings::default()
    }
}

pub fn load_theme(settings: &WispSettings) -> tui::Theme {
    let Some(theme_file) = settings.theme.file.as_deref() else {
        return tui::Theme::default();
    };

    let Some(path) = resolve_theme_file_path(theme_file) else {
        warn!("Rejected unsafe theme filename: {}", theme_file);
        return tui::Theme::default();
    };

    tui::Theme::load_from_path(&path)
}

pub fn resolve_theme_file_path(file_name: &str) -> Option<PathBuf> {
    let trimmed = file_name.trim();
    if trimmed.is_empty() {
        return None;
    }

    let candidate = Path::new(trimmed);
    let base_name = candidate.file_name()?.to_str()?;
    if base_name != trimmed {
        return None;
    }

    if base_name == "." || base_name == ".." {
        return None;
    }

    Some(themes_dir_path()?.join(base_name))
}

pub fn list_theme_files() -> Vec<String> {
    let Some(themes_dir) = themes_dir_path() else {
        return Vec::new();
    };

    let Ok(entries) = std::fs::read_dir(themes_dir) else {
        return Vec::new();
    };

    let mut files = entries
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let Ok(file_type) = entry.file_type() else {
                return None;
            };

            if !file_type.is_file() {
                return None;
            }

            let name = entry.file_name().into_string().ok()?;
            if !name.ends_with(".tmTheme") {
                return None;
            }
            Some(name)
        })
        .collect::<Vec<_>>();

    files.sort_unstable();
    files
}

pub(crate) fn build_login_entries(auth_methods: &[AuthMethod]) -> Vec<ProviderLoginEntry> {
    auth_methods
        .iter()
        .map(|m| {
            let status = if m.description() == Some("authenticated") {
                ProviderLoginStatus::LoggedIn
            } else {
                ProviderLoginStatus::NeedsLogin
            };
            ProviderLoginEntry { method_id: m.id().0.to_string(), name: m.name().to_string(), status }
        })
        .collect()
}

pub(crate) fn create_overlay(
    config_options: &[SessionConfigOption],
    server_statuses: &[McpServerStatusEntry],
    auth_methods: &[AuthMethod],
) -> overlay::SettingsOverlay {
    let mut menu = menu::SettingsMenu::from_config_options(config_options);
    decorate_menu(&mut menu, server_statuses, auth_methods);
    overlay::SettingsOverlay::new(menu, server_statuses.to_vec(), auth_methods.to_vec())
        .with_reasoning_effort_from_options(config_options)
}

pub(crate) fn decorate_menu(
    menu: &mut menu::SettingsMenu,
    server_statuses: &[McpServerStatusEntry],
    auth_methods: &[AuthMethod],
) {
    let settings = load_or_create_settings();
    let theme_files = list_theme_files();
    menu.add_theme_entry(settings.theme.file.as_deref(), &theme_files);

    refresh_mcp_servers_entry(menu, server_statuses);

    if !auth_methods.is_empty() {
        let login_entries = build_login_entries(auth_methods);
        let login_summary = provider_login_summary(&login_entries);
        menu.add_provider_logins_entry(&login_summary);
    }
}

pub(crate) fn refresh_mcp_servers_entry(menu: &mut menu::SettingsMenu, server_statuses: &[McpServerStatusEntry]) {
    menu.upsert_mcp_servers_entry(&server_status_summary(server_statuses));
}

pub(crate) fn process_config_changes(changes: Vec<types::SettingsChange>) -> Vec<overlay::SettingsMessage> {
    use acp_utils::config_option_id::THEME_CONFIG_ID;

    let mut messages = Vec::new();
    for change in changes {
        if change.config_id == THEME_CONFIG_ID {
            let file = theme_file_from_picker_value(&change.new_value);
            let mut settings = load_or_create_settings();
            settings.theme.file = file;
            if let Err(err) = save_settings(&settings) {
                tracing::warn!("Failed to persist theme setting: {err}");
            }
            let theme = load_theme(&settings);
            messages.push(overlay::SettingsMessage::SetTheme(theme));
        } else {
            messages.push(overlay::SettingsMessage::SetConfigOption {
                config_id: change.config_id,
                value: change.new_value,
            });
        }
    }
    messages
}

fn theme_file_from_picker_value(value: &str) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() { None } else { Some(trimmed.to_string()) }
}

pub(crate) fn cycle_quick_option(config_options: &[SessionConfigOption]) -> Option<(String, String)> {
    use crate::components::status_line::is_cycleable_mode_option;
    use agent_client_protocol::schema::{SessionConfigKind, SessionConfigSelectOptions};

    let option = config_options.iter().find(|option| is_cycleable_mode_option(option))?;

    let SessionConfigKind::Select(ref select) = option.kind else {
        return None;
    };

    let SessionConfigSelectOptions::Ungrouped(ref options) = select.options else {
        return None;
    };

    if options.is_empty() {
        return None;
    }

    let current_index = options.iter().position(|entry| entry.value == select.current_value).unwrap_or(0);
    let next_index = (current_index + 1) % options.len();
    options.get(next_index).map(|next| (option.id.0.to_string(), next.value.0.to_string()))
}

pub(crate) fn cycle_reasoning_option(config_options: &[SessionConfigOption]) -> Option<(String, String)> {
    use crate::components::status_line::{extract_reasoning_effort, extract_reasoning_levels};
    use acp_utils::config_option_id::ConfigOptionId;
    use utils::ReasoningEffort;

    let levels = extract_reasoning_levels(config_options);
    if levels.is_empty() {
        return None;
    }

    let current = extract_reasoning_effort(config_options);
    let next = ReasoningEffort::cycle_within(current, &levels);
    Some((ConfigOptionId::ReasoningEffort.as_str().to_string(), ReasoningEffort::config_str(next).to_string()))
}

pub(crate) fn unhealthy_server_count(statuses: &[McpServerStatusEntry]) -> usize {
    use acp_utils::notifications::McpServerStatus;

    statuses.iter().filter(|status| !matches!(status.status, McpServerStatus::Connected { .. })).count()
}

pub fn save_settings(settings: &WispSettings) -> std::io::Result<()> {
    let store = SettingsStore::new("WISP_HOME", ".wisp")
        .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::NotFound, "Unable to resolve Wisp settings path"))?;

    store.save(settings)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_helpers::with_wisp_home;
    use acp_utils::config_option_id::THEME_CONFIG_ID;
    use acp_utils::settings::SettingsStore;
    use std::fs;
    use tempfile::TempDir;

    fn change(config_id: &str, new_value: &str) -> types::SettingsChange {
        types::SettingsChange { config_id: config_id.to_string(), new_value: new_value.to_string() }
    }

    fn with_themes_dir(f: impl FnOnce(&std::path::Path)) {
        let temp_dir = TempDir::new().unwrap();
        let themes = temp_dir.path().join("themes");
        fs::create_dir_all(&themes).unwrap();
        f(&themes);
        std::mem::drop(temp_dir);
    }

    #[test]
    fn round_trip_serde() {
        let temp_dir = TempDir::new().unwrap();
        let store = SettingsStore::from_path(temp_dir.path());
        let settings =
            WispSettings { theme: ThemeSettings { file: Some("my-theme.json".to_string()) }, content_padding: None };
        store.save(&settings).unwrap();
        assert_eq!(store.load_or_create::<WispSettings>(), settings);
    }

    #[test]
    fn resolve_theme_file_path_allows_basename_only() {
        for rejected in ["", "../escape.json", "subdir/theme.json"] {
            assert!(resolve_theme_file_path(rejected).is_none(), "should reject {rejected:?}");
        }
        #[cfg(windows)]
        assert!(resolve_theme_file_path("..\\escape.json").is_none());
    }

    #[test]
    fn list_theme_files_returns_sorted_and_filters_correctly() {
        // Sorted .tmTheme files only, ignoring directories and non-.tmTheme files
        with_themes_dir(|themes| {
            fs::create_dir_all(themes.join("nested")).unwrap();
            fs::write(themes.join("zeta.tmTheme"), "z").unwrap();
            fs::write(themes.join("alpha.tmTheme"), "a").unwrap();
            fs::write(themes.join("readme.txt"), "ignored").unwrap();

            with_wisp_home(themes.parent().unwrap(), || {
                assert_eq!(list_theme_files(), vec!["alpha.tmTheme", "zeta.tmTheme"]);
            });
        });
    }

    #[test]
    fn list_theme_files_returns_empty_when_themes_dir_missing() {
        let temp_dir = TempDir::new().unwrap();
        with_wisp_home(temp_dir.path(), || {
            assert!(list_theme_files().is_empty());
        });
    }

    #[test]
    fn theme_file_from_picker_value_parsing() {
        for (input, expected) in [
            ("   ", None),
            ("", None),
            ("sage.tmTheme", Some("sage.tmTheme")),
            ("  spaced.tmTheme  ", Some("spaced.tmTheme")),
        ] {
            assert_eq!(theme_file_from_picker_value(input), expected.map(String::from), "input: {input:?}");
        }
    }

    #[test]
    fn process_theme_change_persists_and_produces_set_theme() {
        use crate::test_helpers::CUSTOM_TMTHEME;
        use tui::Color;

        with_themes_dir(|themes| {
            fs::write(themes.join("custom.tmTheme"), CUSTOM_TMTHEME).unwrap();

            with_wisp_home(themes.parent().unwrap(), || {
                let messages = process_config_changes(vec![change(THEME_CONFIG_ID, "custom.tmTheme")]);
                let theme = messages.iter().find_map(|m| match m {
                    overlay::SettingsMessage::SetTheme(t) => Some(t),
                    _ => None,
                });
                assert!(theme.is_some(), "should produce SetTheme message");
                assert_eq!(theme.unwrap().text_primary(), Color::Rgb { r: 0x11, g: 0x22, b: 0x33 });
                assert_eq!(load_or_create_settings().theme.file.as_deref(), Some("custom.tmTheme"));
            });
        });
    }

    #[test]
    fn process_theme_change_persists_default_as_none() {
        let temp_dir = TempDir::new().unwrap();
        with_wisp_home(temp_dir.path(), || {
            save_settings(&WispSettings {
                theme: ThemeSettings { file: Some("old.tmTheme".to_string()) },
                content_padding: None,
            })
            .unwrap();
            let _ = process_config_changes(vec![change(THEME_CONFIG_ID, "   ")]);
            assert_eq!(load_or_create_settings().theme.file, None);
        });
    }

    #[test]
    fn process_non_theme_change_produces_set_config_option() {
        let messages = process_config_changes(vec![change("provider", "ollama")]);
        match messages.as_slice() {
            [overlay::SettingsMessage::SetConfigOption { config_id, value }] => {
                assert_eq!(config_id, "provider");
                assert_eq!(value, "ollama");
            }
            other => panic!("expected SetConfigOption, got: {other:?}"),
        }
    }
}
