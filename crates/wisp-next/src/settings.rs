use acp_utils::settings::SettingsStore;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::path::{Path, PathBuf};
use tracing::warn;

pub const DEFAULT_CONTENT_PADDING: usize = 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ContextUsageDisplay {
    pub used_tokens: u32,
    pub limit_tokens: u32,
}

impl ContextUsageDisplay {
    pub fn new(used_tokens: u32, limit_tokens: u32) -> Self {
        Self { used_tokens, limit_tokens }
    }

    pub fn used_ratio(&self) -> f64 {
        if self.limit_tokens == 0 {
            return 0.0;
        }
        (f64::from(self.used_tokens) / f64::from(self.limit_tokens)).clamp(0.0, 1.0)
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct UiSettings {
    pub theme: ThemeSettings,
    pub content_padding: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status_line: Option<StatusLineSettings>,
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

impl StatusLineSettings {
    /// Wisp defaults: left = [cwd, gitRef], right = [agent, mode, model, reasoning, context, serverHealth]
    pub fn wisp_defaults() -> Self {
        Self {
            separator: Some(default_separator()),
            left: Some(default_left_segments()),
            right: Some(default_right_segments()),
        }
    }

    /// Aether defaults: same left, right omits agent segment
    pub fn aether_defaults() -> Self {
        Self {
            separator: Some(default_separator()),
            left: Some(default_left_segments()),
            right: Some(vec![
                StatusLineSegmentConfig::Mode,
                StatusLineSegmentConfig::Model { max_width: None },
                StatusLineSegmentConfig::Reasoning,
                StatusLineSegmentConfig::Context,
                StatusLineSegmentConfig::ServerHealth,
            ]),
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
        Self::wisp_defaults().resolve()
    }
}

impl UiSettings {
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

pub fn resolve_status_line_settings(settings: &UiSettings) -> ResolvedStatusLineSettings {
    settings.status_line.clone().unwrap_or_default().resolve()
}

pub fn resolve_content_padding(settings: &UiSettings) -> usize {
    settings.content_padding.map_or(DEFAULT_CONTENT_PADDING, |value| value.max(2) as usize)
}

pub fn resolve_theme_file_path(settings: &UiSettings) -> Option<PathBuf> {
    let file_name = settings.theme.file.as_deref()?.trim();
    resolve_theme_file_path_from_name(file_name)
}

pub fn load_or_create_settings() -> UiSettings {
    SettingsStore::new("WISP_HOME", ".wisp").map_or_else(
        || {
            warn!("Unable to resolve Wisp settings path; using defaults");
            UiSettings::default()
        },
        |store| store.load_or_create(),
    )
}

pub fn save_settings(settings: &UiSettings) -> std::io::Result<()> {
    let store = SettingsStore::new("WISP_HOME", ".wisp")
        .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::NotFound, "Unable to resolve Wisp settings path"))?;
    store.save(settings)
}

pub fn themes_dir_path() -> Option<PathBuf> {
    let home = SettingsStore::new("WISP_HOME", ".wisp")?.home().to_path_buf();
    Some(home.join("themes"))
}

pub fn list_theme_files() -> Vec<String> {
    let Some(themes_dir) = themes_dir_path() else {
        return Vec::new();
    };
    let Ok(entries) = std::fs::read_dir(themes_dir) else {
        return Vec::new();
    };
    let mut files: Vec<String> = entries
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
        .collect();
    files.sort_unstable();
    files
}

pub fn load_theme_file(file_name: &str) -> crate::theme::Theme {
    let Some(path) = resolve_theme_file_path_from_name(file_name) else {
        return crate::theme::Theme::default();
    };
    crate::theme::Theme::load_from_path(&path)
}

fn resolve_theme_file_path_from_name(file_name: &str) -> Option<PathBuf> {
    let trimmed = file_name.trim();
    if trimmed.is_empty() {
        return None;
    }
    let candidate = Path::new(trimmed);
    let base_name = candidate.file_name()?.to_str()?;
    if base_name != trimmed {
        return None;
    }
    if matches!(base_name, "." | "..") {
        return None;
    }
    Some(themes_dir_path()?.join(base_name))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_line_segments_support_shorthand_and_object_forms() {
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

    #[test]
    fn aether_defaults_omit_agent_segment() {
        let resolved = resolve_status_line_settings(
            &UiSettings::default().with_default_status_line(StatusLineSettings::aether_defaults()),
        );
        assert!(!resolved.right.contains(&StatusLineSegmentConfig::Agent));
        assert!(resolved.right.contains(&StatusLineSegmentConfig::Model { max_width: None }));
    }

    #[test]
    fn wisp_defaults_include_agent_segment() {
        let resolved = resolve_status_line_settings(
            &UiSettings::default().with_default_status_line(StatusLineSettings::wisp_defaults()),
        );
        assert!(resolved.right.contains(&StatusLineSegmentConfig::Agent));
    }

    #[test]
    fn explicit_status_line_keeps_agent_for_aether() {
        let settings = UiSettings {
            status_line: Some(StatusLineSettings {
                right: Some(vec![StatusLineSegmentConfig::Agent]),
                ..Default::default()
            }),
            ..Default::default()
        };

        let resolved =
            resolve_status_line_settings(&settings.with_default_status_line(StatusLineSettings::aether_defaults()));
        assert_eq!(resolved.right, vec![StatusLineSegmentConfig::Agent]);
    }

    #[test]
    fn partial_status_line_keeps_launcher_default_segments() {
        let settings = UiSettings {
            status_line: Some(StatusLineSettings { separator: Some(" | ".to_string()), ..Default::default() }),
            ..Default::default()
        };

        let resolved =
            resolve_status_line_settings(&settings.with_default_status_line(StatusLineSettings::aether_defaults()));

        assert_eq!(resolved.separator, " | ");
        assert_eq!(resolved.left, StatusLineSettings::aether_defaults().left.unwrap());
        assert_eq!(resolved.right, StatusLineSettings::aether_defaults().right.unwrap());
    }

    #[test]
    fn explicit_empty_right_stays_empty() {
        let settings = UiSettings {
            status_line: Some(StatusLineSettings { right: Some(vec![]), ..Default::default() }),
            ..Default::default()
        };

        let resolved =
            resolve_status_line_settings(&settings.with_default_status_line(StatusLineSettings::aether_defaults()));
        assert!(resolved.right.is_empty());
    }

    #[test]
    fn explicit_empty_left_stays_empty() {
        let settings = UiSettings {
            status_line: Some(StatusLineSettings { left: Some(vec![]), ..Default::default() }),
            ..Default::default()
        };

        let resolved =
            resolve_status_line_settings(&settings.with_default_status_line(StatusLineSettings::wisp_defaults()));
        assert!(resolved.left.is_empty());
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
            r#"{"contentPadding":4,"theme":{"file":"nord.tmTheme","future":true},"statusLine":{"left":["cwd"],"right":["agent"]}}"#,
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
        let err = serde_json::from_str::<UiSettings>(r#"{"statusLine": {"left": ["cwd"], "unknownField": true}}"#)
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
    fn invalid_shorthand_segment_name_is_rejected() {
        let err =
            serde_json::from_str::<UiSettings>(r#"{"statusLine": {"left": ["invalidSegmentName"]}}"#).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("unknown variant") || msg.contains("did not match"),
            "should reject invalid segment names, got: {msg}"
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
