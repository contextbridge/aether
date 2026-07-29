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

/// A configured status-line segment.
///
/// Uses the legacy wire contract so old and new wisp clients can share
/// `~/.wisp/settings.json`: simple segments round-trip as shorthand strings
/// (`"cwd"`, `"model"`, ...), while segments that carry data — Cwd/Model with
/// `maxWidth`, and Text — are tagged objects (`{"type":"model","maxWidth":40}`).
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

impl Serialize for StatusLineSegmentConfig {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.clone().into_wire().serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for StatusLineSegmentConfig {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Ok(match StatusLineSegmentConfigWire::deserialize(deserializer)? {
            StatusLineSegmentConfigWire::Shorthand(name) => name.into(),
            StatusLineSegmentConfigWire::Object(object) => object.into(),
        })
    }
}

#[derive(Serialize, Deserialize)]
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
        #[serde(default, rename = "maxWidth")]
        max_width: Option<u16>,
    },
    GitRef,
    Agent,
    Mode,
    Model {
        #[serde(default, rename = "maxWidth")]
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

impl StatusLineSegmentConfig {
    fn into_wire(self) -> StatusLineSegmentConfigWire {
        match self {
            Self::Cwd { max_width: None } => StatusLineSegmentConfigWire::Shorthand(StatusLineSegmentName::Cwd),
            Self::Cwd { max_width: Some(max_width) } => {
                StatusLineSegmentConfigWire::Object(StatusLineSegmentConfigObject::Cwd { max_width: Some(max_width) })
            }
            Self::GitRef => StatusLineSegmentConfigWire::Shorthand(StatusLineSegmentName::GitRef),
            Self::Agent => StatusLineSegmentConfigWire::Shorthand(StatusLineSegmentName::Agent),
            Self::Mode => StatusLineSegmentConfigWire::Shorthand(StatusLineSegmentName::Mode),
            Self::Model { max_width: None } => StatusLineSegmentConfigWire::Shorthand(StatusLineSegmentName::Model),
            Self::Model { max_width: Some(max_width) } => {
                StatusLineSegmentConfigWire::Object(StatusLineSegmentConfigObject::Model { max_width: Some(max_width) })
            }
            Self::Reasoning => StatusLineSegmentConfigWire::Shorthand(StatusLineSegmentName::Reasoning),
            Self::Context => StatusLineSegmentConfigWire::Shorthand(StatusLineSegmentName::Context),
            Self::ServerHealth => StatusLineSegmentConfigWire::Shorthand(StatusLineSegmentName::ServerHealth),
            Self::Text { value, style } => {
                StatusLineSegmentConfigWire::Object(StatusLineSegmentConfigObject::Text { value, style })
            }
        }
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
    pub fn resolve(self) -> ResolvedStatusLineSettings {
        ResolvedStatusLineSettings {
            separator: self.separator.unwrap_or_else(default_separator),
            left: self.left.unwrap_or_else(default_left_segments),
            right: self.right.unwrap_or_else(default_right_segments),
        }
    }
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
    use std::fs;

    use acp_utils::settings::SettingsStore;
    use tempfile::TempDir;

    /// A realistic `~/.wisp/settings.json` written by the legacy wisp client:
    /// shorthand status-line strings (`cwd`, `gitRef`, ...) where no data is needed,
    /// tagged objects only where the legacy schema requires them
    /// (`{"type":"model","maxWidth":40}`, `{"type":"text",...}`).
    const LEGACY_WISP_SETTINGS: &str = include_str!("../tests/fixtures/legacy_settings.json");

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
    fn status_line_segments_serialize_with_legacy_wire_contract() {
        let segments = vec![
            StatusLineSegmentConfig::Cwd { max_width: None },
            StatusLineSegmentConfig::GitRef,
            StatusLineSegmentConfig::Agent,
            StatusLineSegmentConfig::Model { max_width: None },
        ];

        assert_eq!(serde_json::to_value(&segments).unwrap(), serde_json::json!(["cwd", "gitRef", "agent", "model"]));
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
    fn legacy_shorthand_status_line_segments_load() {
        let settings: UiSettings = serde_json::from_str(LEGACY_WISP_SETTINGS)
            .expect("legacy ~/.wisp/settings.json with shorthand status-line segments must load");

        let status_line = settings.status_line.expect("statusLine should be present");
        assert_eq!(status_line.separator.as_deref(), Some(" | "));
        assert_eq!(
            status_line.left,
            Some(vec![
                StatusLineSegmentConfig::Cwd { max_width: None },
                StatusLineSegmentConfig::GitRef,
                StatusLineSegmentConfig::Text { value: "v1.2".to_string(), style: Some(StatusLineStyle::Muted) },
            ]),
        );
        assert_eq!(
            status_line.right,
            Some(vec![
                StatusLineSegmentConfig::Agent,
                StatusLineSegmentConfig::Mode,
                StatusLineSegmentConfig::Model { max_width: Some(40) },
                StatusLineSegmentConfig::Reasoning,
                StatusLineSegmentConfig::Context,
                StatusLineSegmentConfig::ServerHealth,
            ]),
        );

        assert_eq!(settings.content_padding, Some(3));
        assert_eq!(settings.theme.file.as_deref(), Some("gruvbox-dark.tmTheme"));
    }

    #[test]
    fn legacy_status_line_resolves_loaded_segments() {
        let settings: UiSettings = serde_json::from_str(LEGACY_WISP_SETTINGS).unwrap();
        let resolved = resolve_status_line_settings(&settings);

        assert_eq!(resolved.separator, " | ");
        assert_eq!(
            resolved.left,
            vec![
                StatusLineSegmentConfig::Cwd { max_width: None },
                StatusLineSegmentConfig::GitRef,
                StatusLineSegmentConfig::Text { value: "v1.2".to_string(), style: Some(StatusLineStyle::Muted) },
            ],
        );
        assert_eq!(
            resolved.right,
            vec![
                StatusLineSegmentConfig::Agent,
                StatusLineSegmentConfig::Mode,
                StatusLineSegmentConfig::Model { max_width: Some(40) },
                StatusLineSegmentConfig::Reasoning,
                StatusLineSegmentConfig::Context,
                StatusLineSegmentConfig::ServerHealth,
            ],
        );
    }

    #[test]
    fn legacy_status_line_round_trips_through_settings_store() {
        let loaded: UiSettings = serde_json::from_str(LEGACY_WISP_SETTINGS).unwrap();

        let temp = TempDir::new().unwrap();
        let store = SettingsStore::from_path(temp.path());
        store.save(&loaded).expect("save via the real SettingsStore must succeed");

        // The saved file must reproduce the legacy wire form (shorthand for simple
        // segments, tagged objects only where data is required), proving old and
        // new wisp clients can alternate writes without churn.
        let on_disk = fs::read_to_string(temp.path().join("settings.json")).unwrap();
        let saved: serde_json::Value = serde_json::from_str(&on_disk).unwrap();
        let expected: serde_json::Value = serde_json::from_str(LEGACY_WISP_SETTINGS).unwrap();
        assert_eq!(saved, expected, "save must reproduce the legacy wire form");

        let reloaded: UiSettings = store.load_or_create();
        assert_eq!(reloaded, loaded, "every modeled field must survive the save/reload");
    }

    #[test]
    fn loaded_segments_round_trip_with_legacy_wire_contract() {
        let loaded: UiSettings = serde_json::from_str(LEGACY_WISP_SETTINGS).unwrap();
        let status_line = loaded.status_line.unwrap();

        assert_eq!(
            serde_json::to_value(status_line.left.as_ref().unwrap()).unwrap(),
            serde_json::json!(["cwd", "gitRef", {"type": "text", "value": "v1.2", "style": "muted"}]),
        );
        assert_eq!(
            serde_json::to_value(status_line.right.as_ref().unwrap()).unwrap(),
            serde_json::json!([
                "agent",
                "mode",
                {"type": "model", "maxWidth": 40},
                "reasoning",
                "context",
                "serverHealth"
            ]),
        );
    }

    #[test]
    fn unknown_status_line_segment_string_is_rejected() {
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
