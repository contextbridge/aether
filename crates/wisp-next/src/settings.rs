use acp_utils::settings::SettingsStore;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use tracing::warn;

pub const DEFAULT_CONTENT_PADDING: usize = 2;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct UiSettings {
    pub theme: ThemeSettings,
    pub content_padding: Option<u16>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ThemeSettings {
    pub file: Option<String>,
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

pub fn resolve_content_padding(settings: &UiSettings) -> usize {
    settings.content_padding.map_or(DEFAULT_CONTENT_PADDING, |value| value.max(2) as usize)
}

pub fn resolve_theme_file_path(settings: &UiSettings) -> Option<PathBuf> {
    let file_name = settings.theme.file.as_deref()?.trim();
    let candidate = Path::new(file_name);
    let base_name = candidate.file_name()?.to_str()?;
    if file_name.is_empty() || base_name != file_name || matches!(base_name, "." | "..") {
        return None;
    }

    Some(SettingsStore::new("WISP_HOME", ".wisp")?.home().join("themes").join(base_name))
}
