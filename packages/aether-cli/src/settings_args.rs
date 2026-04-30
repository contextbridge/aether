use aether_project::{AetherSettingsSource, SettingsFileSource};
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, Default, clap::Args)]
pub struct SettingsSourceArgs {
    #[arg(long = "settings-json", conflicts_with = "settings_file")]
    pub settings_json: Option<String>,

    #[arg(long = "settings-file", conflicts_with = "settings_json")]
    pub settings_file: Option<PathBuf>,
}

impl SettingsSourceArgs {
    pub fn source(&self, root: &Path) -> Option<AetherSettingsSource> {
        if let Some(json) = &self.settings_json {
            Some(AetherSettingsSource::Json(json.clone()))
        } else {
            self.settings_file
                .as_ref()
                .map(|path| AetherSettingsSource::File(SettingsFileSource::new(path.clone(), root)))
        }
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
    fn settings_file_maps_to_file_source() {
        let args = SettingsSourceArgs { settings_json: None, settings_file: Some(PathBuf::from("settings.json")) };

        let Some(AetherSettingsSource::File(source)) = args.source(Path::new("/workspace")) else {
            panic!("expected file settings source");
        };
        assert_eq!(source, SettingsFileSource::new("settings.json", "/workspace"));
    }
}
