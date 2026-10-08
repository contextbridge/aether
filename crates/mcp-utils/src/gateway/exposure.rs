use super::{ToolFilter, ToolMatcher};
use rmcp::model::Tool;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(from = "ToolExposureConfig", into = "ToolExposureConfig")]
#[schemars(with = "ToolExposureConfig")]
pub enum ToolExposure {
    #[default]
    ModelVisible,
    Deferred(ToolFilter),
}

impl ToolExposure {
    pub fn deferred_all() -> Self {
        Self::Deferred(ToolFilter::default())
    }

    pub fn has_deferred_tools(&self) -> bool {
        matches!(self, Self::Deferred(_))
    }

    pub fn defers(&self, tool: &Tool) -> bool {
        match self {
            Self::ModelVisible => false,
            Self::Deferred(filter) => filter.is_tool_allowed(tool),
        }
    }

    pub fn defer_all_tools(&mut self) {
        if !self.has_deferred_tools() {
            *self = Self::deferred_all();
        }
    }

    pub(crate) fn is_model_visible(&self) -> bool {
        !self.has_deferred_tools()
    }
}

#[derive(Deserialize, Serialize, JsonSchema)]
#[serde(untagged, deny_unknown_fields)]
enum ToolExposureConfig {
    Enabled(bool),
    Rules {
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        include: Vec<ToolMatcher>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        exclude: Vec<ToolMatcher>,
    },
}

impl From<ToolExposureConfig> for ToolExposure {
    fn from(repr: ToolExposureConfig) -> Self {
        match repr {
            ToolExposureConfig::Enabled(false) => Self::ModelVisible,
            ToolExposureConfig::Enabled(true) => Self::deferred_all(),
            ToolExposureConfig::Rules { include, exclude } => {
                Self::Deferred(ToolFilter { allow: include, deny: exclude })
            }
        }
    }
}

impl From<ToolExposure> for ToolExposureConfig {
    fn from(exposure: ToolExposure) -> Self {
        match exposure {
            ToolExposure::ModelVisible => Self::Enabled(false),
            ToolExposure::Deferred(filter) if filter.is_empty() => Self::Enabled(true),
            ToolExposure::Deferred(ToolFilter { allow, deny }) => Self::Rules { include: allow, exclude: deny },
        }
    }
}
