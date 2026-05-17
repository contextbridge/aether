use acp_utils::config_option_id::ConfigOptionId;
use llm::ReasoningEffort;
use thiserror::Error;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfigSetting {
    Mode(String),
    Model(String),
    ReasoningEffort(Option<ReasoningEffort>),
}

impl ConfigSetting {
    pub fn parse(config_id: &str, value: &str) -> Result<Self, ConfigSettingError> {
        let id: ConfigOptionId =
            config_id.parse().map_err(|_| ConfigSettingError::UnknownConfigId(config_id.to_string()))?;

        match id {
            ConfigOptionId::Mode => Ok(Self::Mode(value.to_string())),
            ConfigOptionId::Model => Ok(Self::Model(value.to_string())),
            ConfigOptionId::ReasoningEffort => {
                let effort = ReasoningEffort::parse(value).map_err(|_| ConfigSettingError::InvalidValue {
                    config_id: config_id.to_string(),
                    value: value.to_string(),
                })?;
                Ok(Self::ReasoningEffort(effort))
            }
        }
    }

    pub fn config_id(&self) -> ConfigOptionId {
        match self {
            Self::Mode(_) => ConfigOptionId::Mode,
            Self::Model(_) => ConfigOptionId::Model,
            Self::ReasoningEffort(_) => ConfigOptionId::ReasoningEffort,
        }
    }
}

#[derive(Debug, Error)]
pub enum ConfigSettingError {
    #[error("Unknown config option: {0}")]
    UnknownConfigId(String),
    #[error("Invalid value '{value}' for config option '{config_id}'")]
    InvalidValue { config_id: String, value: String },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_mode() {
        let setting = ConfigSetting::parse("mode", "Planner").unwrap();
        assert_eq!(setting, ConfigSetting::Mode("Planner".to_string()));
        assert_eq!(setting.config_id(), ConfigOptionId::Mode);
    }

    #[test]
    fn parse_model() {
        let setting = ConfigSetting::parse("model", "anthropic:claude-sonnet-4-5").unwrap();
        assert_eq!(setting, ConfigSetting::Model("anthropic:claude-sonnet-4-5".to_string()));
        assert_eq!(setting.config_id(), ConfigOptionId::Model);
    }

    #[test]
    fn parse_reasoning_effort_high() {
        let setting = ConfigSetting::parse("reasoning_effort", "high").unwrap();
        assert_eq!(setting, ConfigSetting::ReasoningEffort(Some(ReasoningEffort::High)));
        assert_eq!(setting.config_id(), ConfigOptionId::ReasoningEffort);
    }

    #[test]
    fn parse_reasoning_effort_none() {
        let setting = ConfigSetting::parse("reasoning_effort", "none").unwrap();
        assert_eq!(setting, ConfigSetting::ReasoningEffort(None));
    }

    #[test]
    fn parse_reasoning_effort_empty() {
        let setting = ConfigSetting::parse("reasoning_effort", "").unwrap();
        assert_eq!(setting, ConfigSetting::ReasoningEffort(None));
    }

    #[test]
    fn unknown_config_id_returns_error() {
        let err = ConfigSetting::parse("unknown", "value").unwrap_err();
        assert!(matches!(err, ConfigSettingError::UnknownConfigId(ref id) if id == "unknown"));
        assert!(err.to_string().contains("unknown"));
    }

    #[test]
    fn invalid_reasoning_effort_returns_error() {
        let err = ConfigSetting::parse("reasoning_effort", "max").unwrap_err();
        assert!(matches!(err, ConfigSettingError::InvalidValue { .. }));
        assert!(err.to_string().contains("max"));
        assert!(err.to_string().contains("reasoning_effort"));
    }

    #[test]
    fn config_id_round_trip() {
        let cases: Vec<(&str, &str)> = vec![("mode", "test"), ("model", "test"), ("reasoning_effort", "low")];
        for (id_str, value) in cases {
            let setting = ConfigSetting::parse(id_str, value).unwrap();
            assert_eq!(setting.config_id().as_str(), id_str);
        }
    }
}
