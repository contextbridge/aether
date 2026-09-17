use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use utils::matches_name_pattern;

/// Which of a server's tools are model-visible or deferred for progressive discovery.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(from = "ToolExposureConfig", into = "ToolExposureConfig")]
#[schemars(with = "ToolExposureConfig")]
pub enum ToolExposure {
    #[default]
    ModelVisible,
    Deferred(DeferredToolRules),
}

#[derive(Debug, Clone, Default, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct DeferredToolRules {
    /// Tool names to defer. An empty list includes every tool.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub include: Vec<String>,
    /// Tool names to keep model-visible. Exclude rules take precedence over include rules.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub exclude: Vec<String>,
}

impl ToolExposure {
    pub fn deferred_all() -> Self {
        Self::Deferred(DeferredToolRules::default())
    }

    pub fn is_model_visible(&self) -> bool {
        matches!(self, Self::ModelVisible)
    }

    pub fn has_deferred_tools(&self) -> bool {
        matches!(self, Self::Deferred(_))
    }

    pub fn is_model_visible_tool(&self, tool_name: &str) -> bool {
        match self {
            Self::ModelVisible => true,
            Self::Deferred(rules) => !rules.matches(tool_name),
        }
    }

    /// Defer every tool, preserving any existing rules.
    pub fn defer_all_tools(&mut self) {
        if self.is_model_visible() {
            *self = Self::deferred_all();
        }
    }
}

impl DeferredToolRules {
    pub fn new(include: &[&str], exclude: &[&str]) -> Self {
        Self {
            include: include.iter().map(ToString::to_string).collect(),
            exclude: exclude.iter().map(ToString::to_string).collect(),
        }
    }

    fn matches(&self, tool_name: &str) -> bool {
        let included =
            self.include.is_empty() || self.include.iter().any(|pattern| matches_name_pattern(pattern, tool_name));
        let excluded = self.exclude.iter().any(|pattern| matches_name_pattern(pattern, tool_name));
        included && !excluded
    }
}

/// The `deferTools` config field's wire shape: a boolean or an include/exclude object.
#[derive(Deserialize, Serialize, JsonSchema)]
#[serde(untagged)]
enum ToolExposureConfig {
    Enabled(bool),
    Rules(DeferredToolRules),
}

impl From<ToolExposureConfig> for ToolExposure {
    fn from(repr: ToolExposureConfig) -> Self {
        match repr {
            ToolExposureConfig::Enabled(false) => Self::ModelVisible,
            ToolExposureConfig::Enabled(true) => Self::deferred_all(),
            ToolExposureConfig::Rules(rules) => Self::Deferred(rules),
        }
    }
}

impl From<ToolExposure> for ToolExposureConfig {
    fn from(exposure: ToolExposure) -> Self {
        match exposure {
            ToolExposure::ModelVisible => Self::Enabled(false),
            ToolExposure::Deferred(rules) if rules == DeferredToolRules::default() => Self::Enabled(true),
            ToolExposure::Deferred(rules) => Self::Rules(rules),
        }
    }
}
