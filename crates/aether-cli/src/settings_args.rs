use aether_project::{AetherSettings, AetherSettingsSource, AgentCatalog, SettingsError, SettingsFileSource};
use std::path::{Path, PathBuf};
use thiserror::Error;

#[derive(Clone, Debug, Default, clap::Args)]
pub struct SettingsSourceArgs {
    #[arg(long = "settings-json", conflicts_with = "settings_file")]
    pub settings_json: Option<String>,

    #[arg(long = "settings-file", conflicts_with = "settings_json")]
    pub settings_file: Option<PathBuf>,
}

/// A JSON options object supplied both an inline `settings` object and a `settingsFile`.
#[derive(Debug, Error)]
#[error("settings and settingsFile cannot both be supplied")]
pub struct ConflictingSettingsSources;

impl SettingsSourceArgs {
    pub fn from_json_options(
        settings: Option<AetherSettings>,
        settings_file: Option<PathBuf>,
    ) -> Result<Self, ConflictingSettingsSources> {
        if settings.is_some() && settings_file.is_some() {
            return Err(ConflictingSettingsSources);
        }
        Ok(Self {
            settings_json: settings.map(|settings| serde_json::to_string(&settings).expect("settings serialize")),
            settings_file,
        })
    }

    pub fn source(&self, root: &Path) -> Option<AetherSettingsSource> {
        if let Some(json) = &self.settings_json {
            Some(AetherSettingsSource::Json(json.clone()))
        } else {
            self.settings_file
                .as_ref()
                .map(|path| AetherSettingsSource::File(SettingsFileSource::new(path.clone(), root)))
        }
    }

    pub fn load_settings(&self, cwd: &Path) -> Result<AetherSettings, SettingsError> {
        match self.source(cwd) {
            Some(source) => AetherSettings::load(cwd, [source]),
            None => AetherSettings::load_default(cwd),
        }
    }

    pub fn load_agent_catalog(&self, cwd: &Path) -> Result<AgentCatalog, SettingsError> {
        let settings = self.load_settings(cwd)?;
        AgentCatalog::from_settings_or_empty(cwd, settings)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_json_maps_to_json_source() {
        let args = SettingsSourceArgs { settings_json: Some("{\"agents\":[]}".to_string()), settings_file: None };

        let Some(AetherSettingsSource::Json(json)) = args.source(Path::new(".")) else {
            panic!("expected JSON settings source");
        };
        assert_eq!(json, "{\"agents\":[]}");
    }

    #[test]
    fn from_json_options_serializes_inline_settings() {
        let args = SettingsSourceArgs::from_json_options(Some(AetherSettings::default()), None).unwrap();

        assert!(args.settings_json.is_some());
        assert!(args.settings_file.is_none());
    }

    #[test]
    fn from_json_options_rejects_both_settings_and_file() {
        let error = SettingsSourceArgs::from_json_options(
            Some(AetherSettings::default()),
            Some(PathBuf::from("settings.json")),
        );

        assert!(matches!(error, Err(ConflictingSettingsSources)));
    }

    #[test]
    fn settings_file_maps_to_file_source() {
        let args = SettingsSourceArgs { settings_json: None, settings_file: Some(PathBuf::from("settings.json")) };

        let Some(AetherSettingsSource::File(source)) = args.source(Path::new("/workspace")) else {
            panic!("expected file settings source");
        };
        assert_eq!(source, SettingsFileSource::new("settings.json", "/workspace"));
    }
}
