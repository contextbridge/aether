use std::fmt;
use std::str::FromStr;

pub const THEME_CONFIG_ID: &str = "__theme";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ConfigOptionId {
    Mode,
    Model,
    ReasoningEffort,
}

impl ConfigOptionId {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Mode => "mode",
            Self::Model => "model",
            Self::ReasoningEffort => "reasoning_effort",
        }
    }
}

impl fmt::Display for ConfigOptionId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, thiserror::Error)]
#[error("Unknown config option: {0}")]
pub struct UnknownConfigOptionId(pub String);

impl FromStr for ConfigOptionId {
    type Err = UnknownConfigOptionId;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "mode" => Ok(Self::Mode),
            "model" => Ok(Self::Model),
            "reasoning_effort" => Ok(Self::ReasoningEffort),
            _ => Err(UnknownConfigOptionId(s.to_string())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_all_variants() {
        for id in [ConfigOptionId::Mode, ConfigOptionId::Model, ConfigOptionId::ReasoningEffort] {
            let parsed: ConfigOptionId = id.as_str().parse().unwrap();
            assert_eq!(parsed, id);
        }
    }

    #[test]
    fn display_matches_as_str() {
        for id in [ConfigOptionId::Mode, ConfigOptionId::Model, ConfigOptionId::ReasoningEffort] {
            assert_eq!(id.to_string(), id.as_str());
        }
    }

    #[test]
    fn unknown_string_returns_error() {
        let err = "unknown_option".parse::<ConfigOptionId>().unwrap_err();
        assert_eq!(err.0, "unknown_option");
        assert!(err.to_string().contains("unknown_option"));
    }
}
