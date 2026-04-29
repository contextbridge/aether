use aether_project::AetherSettingsSource;
use std::path::PathBuf;

#[derive(Clone, Debug, Default, clap::Args)]
pub struct SettingsSourceArgs {
    #[arg(long = "settings-json", conflicts_with = "settings_file")]
    pub settings_json: Option<String>,

    #[arg(long = "settings-file", conflicts_with = "settings_json")]
    pub settings_file: Option<PathBuf>,
}

impl SettingsSourceArgs {
    pub fn source(&self) -> Option<AetherSettingsSource> {
        if let Some(json) = &self.settings_json {
            Some(AetherSettingsSource::Json(json.clone()))
        } else {
            self.settings_file.as_ref().map(|path| AetherSettingsSource::File(path.clone()))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_json_maps_to_json_source() {
        let args = SettingsSourceArgs { settings_json: Some("{\"agents\":[]}".to_string()), settings_file: None };

        let Some(AetherSettingsSource::Json(json)) = args.source() else {
            panic!("expected JSON settings source");
        };
        assert_eq!(json, "{\"agents\":[]}");
    }

    #[test]
    fn settings_file_maps_to_file_source() {
        let args = SettingsSourceArgs { settings_json: None, settings_file: Some(PathBuf::from("settings.json")) };

        let Some(AetherSettingsSource::File(path)) = args.source() else {
            panic!("expected file settings source");
        };
        assert_eq!(path, PathBuf::from("settings.json"));
    }
}
