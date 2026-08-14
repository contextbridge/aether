#![doc = include_str!("../docs/settings_module.md")]

pub mod menu;
pub mod overlay;
pub(crate) mod picker;
pub mod types;

use crate::components::provider_login::{ProviderLoginEntry, ProviderLoginStatus, provider_login_summary};
use crate::components::server_status::server_status_summary;
use acp_utils::notifications::McpServerStatusEntry;
use acp_utils::settings::SettingsStore;
use agent_client_protocol::schema::v1::{AuthMethod, SessionConfigOption};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::path::{Path, PathBuf};
use tracing::warn;

#[cfg(test)]
pub(crate) static WISP_HOME_ENV_MUTEX: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WispSettings {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status_line: Option<StatusLineSettings>,
    #[serde(default, skip_serializing_if = "ThemeSettings::is_empty")]
    pub theme: ThemeSettings,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_padding: Option<u16>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ThemeSettings {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StatusLineSettings {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub separator: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub left: Option<Vec<StatusLineSegmentConfig>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub right: Option<Vec<StatusLineSegmentConfig>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StatusLineSegmentConfig {
    Cwd { max_width: Option<u16> },
    GitRef,
    Agent,
    Mode,
    Model { max_width: Option<u16> },
    Reasoning,
    Context,
    ServerHealth,
    Text { value: String, style: Option<StatusLineStyle> },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedStatusLineSettings {
    pub separator: String,
    pub left: Vec<StatusLineSegmentConfig>,
    pub right: Vec<StatusLineSegmentConfig>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum StatusLineStyle {
    Primary,
    Secondary,
    Muted,
    Info,
    Success,
    Warning,
    Error,
}

impl ThemeSettings {
    pub fn is_empty(&self) -> bool {
        self.file.is_none()
    }
}
impl WispSettings {
    pub fn with_default_status_line(mut self, default: StatusLineSettings) -> Self {
        let user = self.status_line.unwrap_or_default();
        self.status_line = Some(StatusLineSettings {
            separator: user.separator.or(default.separator),
            left: user.left.or(default.left),
            right: user.right.or(default.right),
        });
        self
    }
}

impl StatusLineSettings {
    pub fn defaults() -> Self {
        Self {
            separator: Some(default_separator()),
            left: Some(default_left_segments()),
            right: Some(default_right_segments()),
        }
    }

    pub fn resolve(self) -> ResolvedStatusLineSettings {
        ResolvedStatusLineSettings {
            separator: self.separator.unwrap_or_else(default_separator),
            left: self.left.unwrap_or_else(default_left_segments),
            right: self.right.unwrap_or_else(default_right_segments),
        }
    }

    pub fn resolved_defaults() -> ResolvedStatusLineSettings {
        Self::defaults().resolve()
    }
}

impl From<StatusLineSegmentName> for StatusLineSegmentConfig {
    fn from(name: StatusLineSegmentName) -> Self {
        match name {
            StatusLineSegmentName::Cwd => Self::Cwd { max_width: None },
            StatusLineSegmentName::GitRef => Self::GitRef,
            StatusLineSegmentName::Agent => Self::Agent,
            StatusLineSegmentName::Mode => Self::Mode,
            StatusLineSegmentName::Model => Self::Model { max_width: None },
            StatusLineSegmentName::Reasoning => Self::Reasoning,
            StatusLineSegmentName::Context => Self::Context,
            StatusLineSegmentName::ServerHealth => Self::ServerHealth,
        }
    }
}

impl From<StatusLineSegmentConfigObject> for StatusLineSegmentConfig {
    fn from(object: StatusLineSegmentConfigObject) -> Self {
        match object {
            StatusLineSegmentConfigObject::Cwd { max_width } => Self::Cwd { max_width },
            StatusLineSegmentConfigObject::GitRef => Self::GitRef,
            StatusLineSegmentConfigObject::Agent => Self::Agent,
            StatusLineSegmentConfigObject::Mode => Self::Mode,
            StatusLineSegmentConfigObject::Model { max_width } => Self::Model { max_width },
            StatusLineSegmentConfigObject::Reasoning => Self::Reasoning,
            StatusLineSegmentConfigObject::Context => Self::Context,
            StatusLineSegmentConfigObject::ServerHealth => Self::ServerHealth,
            StatusLineSegmentConfigObject::Text { value, style } => Self::Text { value, style },
        }
    }
}

impl<'de> Deserialize<'de> for StatusLineSegmentConfig {
    fn deserialize<T: Deserializer<'de>>(deserializer: T) -> Result<Self, T::Error> {
        Ok(match StatusLineSegmentConfigWire::deserialize(deserializer)? {
            StatusLineSegmentConfigWire::Shorthand(name) => name.into(),
            StatusLineSegmentConfigWire::Object(object) => object.into(),
        })
    }
}

impl Serialize for StatusLineSegmentConfig {
    fn serialize<T: Serializer>(&self, serializer: T) -> Result<T::Ok, T::Error> {
        match self {
            Self::Cwd { max_width: None } => StatusLineSegmentName::Cwd.serialize(serializer),
            Self::Cwd { max_width } => {
                Serialize::serialize(&StatusLineSegmentConfigObject::Cwd { max_width: *max_width }, serializer)
            }
            Self::GitRef => StatusLineSegmentName::GitRef.serialize(serializer),
            Self::Agent => StatusLineSegmentName::Agent.serialize(serializer),
            Self::Mode => StatusLineSegmentName::Mode.serialize(serializer),
            Self::Model { max_width: None } => StatusLineSegmentName::Model.serialize(serializer),
            Self::Model { max_width } => {
                Serialize::serialize(&StatusLineSegmentConfigObject::Model { max_width: *max_width }, serializer)
            }
            Self::Reasoning => StatusLineSegmentName::Reasoning.serialize(serializer),
            Self::Context => StatusLineSegmentName::Context.serialize(serializer),
            Self::ServerHealth => StatusLineSegmentName::ServerHealth.serialize(serializer),
            Self::Text { value, style } => Serialize::serialize(
                &StatusLineSegmentConfigObject::Text { value: value.clone(), style: *style },
                serializer,
            ),
        }
    }
}

#[derive(Deserialize)]
#[serde(untagged)]
enum StatusLineSegmentConfigWire {
    Shorthand(StatusLineSegmentName),
    Object(StatusLineSegmentConfigObject),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
enum StatusLineSegmentName {
    Cwd,
    GitRef,
    Agent,
    Mode,
    Model,
    Reasoning,
    Context,
    ServerHealth,
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase", deny_unknown_fields)]
enum StatusLineSegmentConfigObject {
    Cwd {
        #[serde(default, skip_serializing_if = "Option::is_none", rename = "maxWidth")]
        max_width: Option<u16>,
    },
    GitRef,
    Agent,
    Mode,
    Model {
        #[serde(default, skip_serializing_if = "Option::is_none", rename = "maxWidth")]
        max_width: Option<u16>,
    },
    Reasoning,
    Context,
    ServerHealth,
    Text {
        value: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        style: Option<StatusLineStyle>,
    },
}

fn default_separator() -> String {
    " · ".to_string()
}

fn default_left_segments() -> Vec<StatusLineSegmentConfig> {
    vec![StatusLineSegmentConfig::Cwd { max_width: None }, StatusLineSegmentConfig::GitRef]
}

fn default_right_segments() -> Vec<StatusLineSegmentConfig> {
    vec![
        StatusLineSegmentConfig::Agent,
        StatusLineSegmentConfig::Mode,
        StatusLineSegmentConfig::Model { max_width: None },
        StatusLineSegmentConfig::Reasoning,
        StatusLineSegmentConfig::Context,
        StatusLineSegmentConfig::ServerHealth,
    ]
}

pub const DEFAULT_CONTENT_PADDING: usize = 2;

pub fn resolve_content_padding(settings: &WispSettings) -> usize {
    settings.content_padding.map_or(DEFAULT_CONTENT_PADDING, |v| v.max(2) as usize)
}

pub fn resolve_status_line_settings(settings: &WispSettings) -> ResolvedStatusLineSettings {
    settings.status_line.clone().unwrap_or_default().resolve()
}

pub fn wisp_home() -> Option<PathBuf> {
    Some(SettingsStore::new("WISP_HOME", ".wisp")?.home().to_path_buf())
}

pub fn themes_dir_path() -> Option<PathBuf> {
    Some(wisp_home()?.join("themes"))
}

pub fn load_or_create_settings(default_status_line: StatusLineSettings) -> WispSettings {
    load_or_create_raw_settings().with_default_status_line(default_status_line)
}

fn load_or_create_raw_settings() -> WispSettings {
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
    let settings = load_or_create_raw_settings();
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
            let mut settings = load_or_create_raw_settings();
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
    use agent_client_protocol::schema::v1::{SessionConfigKind, SessionConfigSelectOptions};

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
            WispSettings { theme: ThemeSettings { file: Some("my-theme.json".to_string()) }, ..Default::default() };
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
                assert_eq!(
                    load_or_create_settings(StatusLineSettings::defaults()).theme.file.as_deref(),
                    Some("custom.tmTheme")
                );
            });
        });
    }

    #[test]
    fn process_theme_change_persists_default_as_none() {
        let temp_dir = TempDir::new().unwrap();
        with_wisp_home(temp_dir.path(), || {
            save_settings(&WispSettings {
                theme: ThemeSettings { file: Some("old.tmTheme".to_string()) },
                ..Default::default()
            })
            .unwrap();
            let _ = process_config_changes(vec![change(THEME_CONFIG_ID, "   ")]);
            assert_eq!(load_or_create_settings(StatusLineSettings::defaults()).theme.file, None);
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

    fn aether_default_status_line() -> StatusLineSettings {
        StatusLineSettings {
            separator: Some(" · ".to_string()),
            left: Some(vec![StatusLineSegmentConfig::Cwd { max_width: None }, StatusLineSegmentConfig::GitRef]),
            right: Some(vec![
                StatusLineSegmentConfig::Mode,
                StatusLineSegmentConfig::Model { max_width: None },
                StatusLineSegmentConfig::Reasoning,
                StatusLineSegmentConfig::Context,
                StatusLineSegmentConfig::ServerHealth,
            ]),
        }
    }

    #[test]
    fn aether_defaults_omit_agent_segment() {
        let resolved = resolve_status_line_settings(
            &WispSettings::default().with_default_status_line(aether_default_status_line()),
        );
        assert!(!resolved.right.contains(&StatusLineSegmentConfig::Agent));
        assert!(resolved.right.contains(&StatusLineSegmentConfig::Model { max_width: None }));
    }

    #[test]
    fn wisp_defaults_include_agent_segment() {
        let resolved = resolve_status_line_settings(
            &WispSettings::default().with_default_status_line(StatusLineSettings::defaults()),
        );
        assert!(resolved.right.contains(&StatusLineSegmentConfig::Agent));
    }

    #[test]
    fn explicit_status_line_keeps_agent_for_aether() {
        let settings = WispSettings {
            status_line: Some(StatusLineSettings {
                right: Some(vec![StatusLineSegmentConfig::Agent]),
                ..Default::default()
            }),
            ..Default::default()
        };

        let resolved = resolve_status_line_settings(&settings.with_default_status_line(aether_default_status_line()));
        assert_eq!(resolved.right, vec![StatusLineSegmentConfig::Agent]);
    }

    #[test]
    fn partial_status_line_keeps_launcher_default_segments() {
        let settings = WispSettings {
            status_line: Some(StatusLineSettings { separator: Some(" | ".to_string()), ..Default::default() }),
            ..Default::default()
        };

        let resolved = resolve_status_line_settings(&settings.with_default_status_line(aether_default_status_line()));

        assert_eq!(resolved.separator, " | ");
        assert_eq!(resolved.left, aether_default_status_line().left.unwrap());
        assert_eq!(resolved.right, aether_default_status_line().right.unwrap());
    }

    #[test]
    fn explicit_empty_right_stays_empty() {
        let settings = WispSettings {
            status_line: Some(StatusLineSettings { right: Some(vec![]), ..Default::default() }),
            ..Default::default()
        };

        let resolved = resolve_status_line_settings(&settings.with_default_status_line(aether_default_status_line()));
        assert!(resolved.right.is_empty());
    }

    #[test]
    fn status_line_segments_support_shorthand_and_object_forms() {
        let settings: WispSettings = serde_json::from_str(
            r#"{
                "statusLine": {
                    "left": ["cwd", "gitRef"],
                    "right": ["agent", {"type": "model", "maxWidth": 32}]
                }
            }"#,
        )
        .unwrap();

        let status_line = settings.status_line.unwrap();
        assert_eq!(
            status_line.left,
            Some(vec![StatusLineSegmentConfig::Cwd { max_width: None }, StatusLineSegmentConfig::GitRef])
        );
        assert_eq!(
            status_line.right,
            Some(vec![StatusLineSegmentConfig::Agent, StatusLineSegmentConfig::Model { max_width: Some(32) }])
        );
    }

    #[test]
    fn simple_status_line_segments_serialize_as_shorthand() {
        let segments = vec![
            StatusLineSegmentConfig::Cwd { max_width: None },
            StatusLineSegmentConfig::GitRef,
            StatusLineSegmentConfig::Agent,
            StatusLineSegmentConfig::Model { max_width: None },
        ];

        assert_eq!(serde_json::to_value(&segments).unwrap(), serde_json::json!(["cwd", "gitRef", "agent", "model"]));
    }
}
