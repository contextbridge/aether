use acp_utils::config_option_id::{ConfigOptionId, UnknownConfigOptionId};
use llm::ReasoningEffort;
use thiserror::Error;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ConfigSetting {
    Mode(String),
    Model(String),
    ReasoningEffort(Option<ReasoningEffort>),
}

impl ConfigSetting {
    pub(crate) fn parse(config_id: &str, value: &str) -> Result<Self, ConfigSettingError> {
        match config_id.parse()? {
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
}

#[derive(Debug, Error)]
pub(crate) enum ConfigSettingError {
    #[error(transparent)]
    UnknownConfigId(#[from] UnknownConfigOptionId),
    #[error("Invalid value '{value}' for config option '{config_id}'")]
    InvalidValue { config_id: String, value: String },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_each_config_option() {
        let cases = [
            ("mode", "Planner", ConfigSetting::Mode("Planner".into())),
            ("model", "anthropic:claude-sonnet-4-5", ConfigSetting::Model("anthropic:claude-sonnet-4-5".into())),
            ("reasoning_effort", "high", ConfigSetting::ReasoningEffort(Some(ReasoningEffort::High))),
            ("reasoning_effort", "max", ConfigSetting::ReasoningEffort(Some(ReasoningEffort::Max))),
            ("reasoning_effort", "", ConfigSetting::ReasoningEffort(None)),
        ];
        for (config_id, value, expected) in cases {
            assert_eq!(ConfigSetting::parse(config_id, value).unwrap(), expected, "{config_id}={value}");
        }
    }

    #[test]
    fn rejects_unknown_ids_and_invalid_values() {
        let err = ConfigSetting::parse("unknown", "value").unwrap_err();
        assert!(matches!(err, ConfigSettingError::UnknownConfigId(UnknownConfigOptionId(ref id)) if id == "unknown"));
        assert!(err.to_string().contains("unknown"));

        let err = ConfigSetting::parse("reasoning_effort", "ultra").unwrap_err();
        assert!(matches!(err, ConfigSettingError::InvalidValue { .. }));
        assert_eq!(err.to_string(), "Invalid value 'ultra' for config option 'reasoning_effort'");
    }
}
