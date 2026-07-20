#[cfg(feature = "testing")]
pub mod overlay;
#[cfg(not(feature = "testing"))]
pub(crate) mod overlay;
mod settings_model;
mod themes;

pub(crate) use settings_model::SettingsModel;
pub use themes::{list_theme_files, load_theme_file, resolve_theme_file_path};

use acp_utils::settings::SettingsStore;
use serde::{Deserialize, Serialize};
use tracing::warn;

pub const DEFAULT_CONTENT_PADDING: usize = 2;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct UiSettings {
    pub theme: ThemeSettings,
    pub content_padding: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status_line: Option<StatusLineSettings>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub keybindings: Option<KeybindingsSettings>,
}

/// Overrides for the global command bindings, as `"ctrl+g"`-style strings.
/// Absent entries keep their defaults.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct KeybindingsSettings {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cancel: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub submit: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub open_command_picker: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub open_file_picker: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub toggle_git_diff: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cycle_reasoning: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cycle_mode: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub open_prompt_search: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ThemeSettings {
    pub file: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StatusLineSettings {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub separator: Option<String>,
    #[serde(default, deserialize_with = "deserialize_segments", skip_serializing_if = "Option::is_none")]
    pub left: Option<Vec<StatusLineSegmentConfig>>,
    #[serde(default, deserialize_with = "deserialize_segments", skip_serializing_if = "Option::is_none")]
    pub right: Option<Vec<StatusLineSegmentConfig>>,
}

/// A configured status-line segment, as a tagged object
/// (`{"type":"model","maxWidth":40}`).
///
/// A segment carrying no options may also be written as its bare name
/// (`"model"`). Both clients read the same `~/.wisp/settings.json`, and a file
/// serde rejects is discarded whole, so refusing the shorthand would silently
/// reset every unrelated setting alongside it. Writing always uses the object
/// form.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase", deny_unknown_fields)]
pub enum StatusLineSegmentConfig {
    Cwd {
        #[serde(default, rename = "maxWidth", skip_serializing_if = "Option::is_none")]
        max_width: Option<u16>,
    },
    GitRef,
    Agent,
    Mode,
    Model {
        #[serde(default, rename = "maxWidth", skip_serializing_if = "Option::is_none")]
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

impl UiSettings {
    /// Fill missing status-line fields while preserving explicit user settings.
    pub fn with_default_status_line(mut self, default: StatusLineSettings) -> Self {
        let current = self.status_line.unwrap_or_default();
        self.status_line = Some(StatusLineSettings {
            separator: current.separator.or(default.separator),
            left: current.left.or(default.left),
            right: current.right.or(default.right),
        });
        self
    }
}

impl StatusLineSettings {
    pub fn resolve(self) -> ResolvedStatusLineSettings {
        ResolvedStatusLineSettings {
            separator: self.separator.unwrap_or_else(default_separator),
            left: self.left.unwrap_or_else(default_left_segments),
            right: self.right.unwrap_or_else(default_right_segments),
        }
    }
}

/// The bare-name spelling of every segment that takes no options.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
enum SegmentName {
    Cwd,
    GitRef,
    Agent,
    Mode,
    Model,
    Reasoning,
    Context,
    ServerHealth,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum SegmentWire {
    Name(SegmentName),
    Config(StatusLineSegmentConfig),
}

impl From<SegmentWire> for StatusLineSegmentConfig {
    fn from(wire: SegmentWire) -> Self {
        match wire {
            SegmentWire::Config(config) => config,
            SegmentWire::Name(SegmentName::Cwd) => Self::Cwd { max_width: None },
            SegmentWire::Name(SegmentName::GitRef) => Self::GitRef,
            SegmentWire::Name(SegmentName::Agent) => Self::Agent,
            SegmentWire::Name(SegmentName::Mode) => Self::Mode,
            SegmentWire::Name(SegmentName::Model) => Self::Model { max_width: None },
            SegmentWire::Name(SegmentName::Reasoning) => Self::Reasoning,
            SegmentWire::Name(SegmentName::Context) => Self::Context,
            SegmentWire::Name(SegmentName::ServerHealth) => Self::ServerHealth,
        }
    }
}

fn deserialize_segments<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<Vec<StatusLineSegmentConfig>>, D::Error> {
    let segments = Option::<Vec<SegmentWire>>::deserialize(deserializer)?;
    Ok(segments.map(|segments| segments.into_iter().map(Into::into).collect()))
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

pub fn resolve_status_line_settings(settings: &UiSettings) -> ResolvedStatusLineSettings {
    settings.status_line.clone().unwrap_or_default().resolve()
}

pub fn resolve_content_padding(settings: &UiSettings) -> usize {
    settings.content_padding.map_or(DEFAULT_CONTENT_PADDING, |value| value.max(2) as usize)
}

pub fn load_or_create_settings() -> UiSettings {
    store().map_or_else(
        || {
            warn!("Unable to resolve Wisp settings path; using defaults");
            UiSettings::default()
        },
        |store| store.load_or_create(),
    )
}

pub fn save_settings(settings: &UiSettings) -> std::io::Result<()> {
    let store = store()
        .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::NotFound, "Unable to resolve Wisp settings path"))?;
    store.save(settings)
}

/// The on-disk home for wisp settings, shared by every load and save.
fn store() -> Option<SettingsStore> {
    SettingsStore::new("WISP_HOME", ".wisp")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn launcher_defaults_fill_only_missing_status_line_fields() {
        let settings = UiSettings {
            status_line: Some(StatusLineSettings {
                separator: Some(" | ".to_string()),
                left: None,
                right: Some(Vec::new()),
            }),
            ..UiSettings::default()
        };
        let defaults = StatusLineSettings {
            separator: Some(" · ".to_string()),
            left: Some(vec![StatusLineSegmentConfig::GitRef]),
            right: Some(vec![StatusLineSegmentConfig::Agent]),
        };

        let status_line = settings.with_default_status_line(defaults).status_line.unwrap();

        assert_eq!(status_line.separator.as_deref(), Some(" | "));
        assert_eq!(status_line.left, Some(vec![StatusLineSegmentConfig::GitRef]));
        assert_eq!(status_line.right, Some(Vec::new()));
    }

    #[test]
    fn status_line_segments_support_tagged_objects() {
        let settings: UiSettings = serde_json::from_str(
            r#"{
                "statusLine": {
                    "left": [{"type": "cwd"}, {"type": "gitRef"}],
                    "right": [{"type": "agent"}, {"type": "model", "maxWidth": 32}]
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
    fn shorthand_segment_names_are_read_as_optionless_segments() {
        let settings: UiSettings = serde_json::from_str(
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

    /// A settings file serde rejects is discarded whole, taking every unrelated
    /// setting with it, so the shorthand must not cost the reader its theme.
    #[test]
    fn a_shorthand_status_line_does_not_discard_the_rest_of_the_file() {
        let settings: UiSettings = serde_json::from_str(
            r#"{
                "contentPadding": 4,
                "theme": {"file": "nord.tmTheme"},
                "statusLine": {"left": ["cwd"]}
            }"#,
        )
        .unwrap();

        assert_eq!(settings.content_padding, Some(4));
        assert_eq!(settings.theme.file.as_deref(), Some("nord.tmTheme"));
    }

    #[test]
    fn segments_always_serialize_as_objects() {
        let settings = StatusLineSettings {
            separator: None,
            left: Some(vec![StatusLineSegmentConfig::Cwd { max_width: None }, StatusLineSegmentConfig::GitRef]),
            right: None,
        };

        assert_eq!(
            serde_json::to_value(&settings).unwrap(),
            serde_json::json!({"left": [{"type": "cwd"}, {"type": "gitRef"}]})
        );
    }

    #[test]
    fn text_segment_with_style_deserializes() {
        let settings: UiSettings = serde_json::from_str(
            r#"{
                "statusLine": {
                    "left": [{"type": "text", "value": "hello", "style": "warning"}]
                }
            }"#,
        )
        .unwrap();

        let status_line = settings.status_line.unwrap();
        assert_eq!(
            status_line.left,
            Some(vec![StatusLineSegmentConfig::Text {
                value: "hello".to_string(),
                style: Some(StatusLineStyle::Warning)
            }])
        );
    }

    #[test]
    fn status_line_settings_present_are_no_longer_ignored() {
        let settings: UiSettings = serde_json::from_str(
            r#"{"contentPadding":4,"theme":{"file":"nord.tmTheme","future":true},"statusLine":{"left":[{"type":"cwd"}],"right":[{"type":"agent"}]}}"#,
        )
        .unwrap();

        assert_eq!(settings.content_padding, Some(4));
        assert_eq!(settings.theme.file.as_deref(), Some("nord.tmTheme"));
        assert!(settings.status_line.is_some());
        let sl = settings.status_line.unwrap();
        assert_eq!(sl.left, Some(vec![StatusLineSegmentConfig::Cwd { max_width: None }]));
        assert_eq!(sl.right, Some(vec![StatusLineSegmentConfig::Agent]));
    }

    #[test]
    fn cwd_max_width_deserializes() {
        let settings: UiSettings = serde_json::from_str(
            r#"{
                "statusLine": {
                    "right": [{"type": "cwd", "maxWidth": 30}]
                }
            }"#,
        )
        .unwrap();

        let sl = settings.status_line.unwrap();
        assert_eq!(sl.right, Some(vec![StatusLineSegmentConfig::Cwd { max_width: Some(30) }]));
    }

    #[test]
    fn cwd_max_width_serializes_as_object() {
        let seg = StatusLineSegmentConfig::Cwd { max_width: Some(30) };
        let json = serde_json::to_value(&seg).unwrap();
        assert_eq!(json, serde_json::json!({"type": "cwd", "maxWidth": 30}));
    }

    #[test]
    fn status_line_settings_rejects_unknown_fields() {
        let err =
            serde_json::from_str::<UiSettings>(r#"{"statusLine": {"left": [{"type":"cwd"}], "unknownField": true}}"#)
                .unwrap_err();
        assert!(
            err.to_string().contains("unknown field"),
            "should reject unknown fields in StatusLineSettings, got: {err}"
        );
    }

    #[test]
    fn status_line_segment_object_rejects_unknown_fields() {
        let err = serde_json::from_str::<UiSettings>(
            r#"{"statusLine": {"left": [{"type": "cwd", "maxWidth": 30, "foo": 42}]}}"#,
        )
        .unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("unknown field") || msg.contains("did not match"),
            "should reject unknown fields in segment objects, got: {msg}"
        );
    }

    #[test]
    fn invalid_status_line_style_is_rejected() {
        let err = serde_json::from_str::<UiSettings>(
            r#"{"statusLine": {"left": [{"type": "text", "value": "hi", "style": "notARealStyle"}]}}"#,
        )
        .unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("unknown variant") || msg.contains("did not match"),
            "should reject invalid style names, got: {msg}"
        );
    }
}
