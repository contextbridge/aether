use super::LlmModel;
use crate::ReasoningEffort;
use std::fmt;
use std::str::FromStr;

/// A validated model selection: a single model or several alloyed models,
/// parsed from a comma-separated `provider:model` list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelSpec(Vec<LlmModel>);

impl ModelSpec {
    pub fn models(&self) -> &[LlmModel] {
        &self.0
    }

    /// Reasoning levels supported by every model in the spec.
    pub fn reasoning_levels(&self) -> Vec<ReasoningEffort> {
        ReasoningEffort::selectable_levels()
            .iter()
            .filter(|level| self.0.iter().all(|model| model.effective_reasoning_levels().contains(level)))
            .copied()
            .collect()
    }

    pub fn validate_reasoning_effort(&self, effort: Option<ReasoningEffort>) -> Result<(), ReasoningEffortError> {
        let Some(effort) = effort else {
            return Ok(());
        };
        self.0.iter().try_for_each(|model| model.validate_reasoning_effort(effort))
    }

    /// The nearest effort supported by every model in the spec, or `None`
    /// when the spec does not support reasoning at all.
    pub fn clamp_reasoning_effort(&self, effort: Option<ReasoningEffort>) -> Option<ReasoningEffort> {
        if effort.is_some_and(|effort| !effort.is_enabled()) {
            return effort;
        }
        let levels: Vec<_> = self.reasoning_levels().into_iter().filter(|effort| effort.is_enabled()).collect();
        effort.filter(|_| !levels.is_empty()).map(|effort| effort.clamp_to(&levels))
    }
}

impl FromStr for ModelSpec {
    type Err = ModelSpecError;

    fn from_str(spec: &str) -> Result<Self, Self::Err> {
        if spec.trim().is_empty() {
            return Err(ModelSpecError::Empty);
        }
        spec.split(',')
            .map(str::trim)
            .map(|part| {
                if part.is_empty() {
                    return Err(ModelSpecError::EmptyEntry);
                }
                part.parse::<LlmModel>()
                    .map_err(|source| ModelSpecError::InvalidModel { model: part.to_string(), source })
            })
            .collect::<Result<Vec<_>, _>>()
            .map(Self)
    }
}

impl fmt::Display for ModelSpec {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (index, model) in self.0.iter().enumerate() {
            if index > 0 {
                write!(formatter, ",")?;
            }
            write!(formatter, "{model}")?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModelSpecError {
    Empty,
    EmptyEntry,
    InvalidModel { model: String, source: String },
}

impl fmt::Display for ModelSpecError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => write!(formatter, "model spec cannot be empty"),
            Self::EmptyEntry => write!(formatter, "model spec contains an empty entry"),
            Self::InvalidModel { model, source } => write!(formatter, "invalid model '{model}': {source}"),
        }
    }
}

impl std::error::Error for ModelSpecError {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReasoningEffortError {
    InvalidSpec(ModelSpecError),
    Unsupported { model: String, effort: ReasoningEffort, supported: Vec<ReasoningEffort> },
}

impl fmt::Display for ReasoningEffortError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidSpec(source) => write!(formatter, "{source}"),
            Self::Unsupported { model, supported, .. } if supported.is_empty() => {
                write!(formatter, "model '{model}' does not support reasoning")
            }
            Self::Unsupported { model, effort, supported } => {
                let supported = supported.iter().map(ToString::to_string).collect::<Vec<_>>().join(", ");
                write!(
                    formatter,
                    "model '{model}' does not support reasoning effort '{effort}'; supported: {supported}"
                )
            }
        }
    }
}

impl std::error::Error for ReasoningEffortError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::InvalidSpec(source) => Some(source),
            Self::Unsupported { .. } => None,
        }
    }
}

/// Validate `effort` against every model in a comma-separated model spec.
pub fn validate_reasoning_effort(
    model_spec: &str,
    effort: Option<ReasoningEffort>,
) -> Result<(), ReasoningEffortError> {
    if effort.is_none() {
        return Ok(());
    }
    let spec = model_spec.parse::<ModelSpec>().map_err(ReasoningEffortError::InvalidSpec)?;
    spec.validate_reasoning_effort(effort)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn control_states_validate_and_clamp_without_enabling_disabled() {
        for model in LlmModel::all() {
            assert!(model.validate_reasoning_effort(ReasoningEffort::Default).is_ok());
            assert_eq!(
                model.validate_reasoning_effort(ReasoningEffort::Disabled).is_ok(),
                model.supports_reasoning_off_transport()
            );
            assert_eq!(
                model.supports_reasoning_off(),
                model.reasoning_disabled_support() != crate::ReasoningDisabledSupport::Unsupported
            );
            assert!(!model.reasoning_levels().contains(&ReasoningEffort::Default));
        }
        for text in ["openai:gpt-4o-mini", "codex:gpt-5.4", "ollama:unknown", "openai:gpt-5.4,codex:gpt-5.4"] {
            let spec: ModelSpec = text.parse().unwrap();
            assert_eq!(spec.clamp_reasoning_effort(Some(ReasoningEffort::Disabled)), Some(ReasoningEffort::Disabled));
            assert!(spec.validate_reasoning_effort(Some(ReasoningEffort::Disabled)).is_err());
        }
        let spec: ModelSpec = "openai:gpt-5.4,openai:gpt-5.1".parse().unwrap();
        assert!(spec.reasoning_levels().contains(&ReasoningEffort::Disabled));
        assert!(spec.validate_reasoning_effort(Some(ReasoningEffort::Disabled)).is_ok());
    }

    #[test]
    fn parses_and_displays_alloyed_specs_canonically() {
        let spec: ModelSpec = " codex:gpt-5.6-sol , anthropic:claude-opus-4-6 ".parse().unwrap();
        assert_eq!(spec.models().len(), 2);
        assert_eq!(spec.to_string(), "codex:gpt-5.6-sol,anthropic:claude-opus-4-6");
    }

    #[test]
    fn parse_rejects_empty_and_invalid_specs() {
        assert_eq!("".parse::<ModelSpec>().unwrap_err(), ModelSpecError::Empty);
        assert_eq!("anthropic:claude-opus-4-6,".parse::<ModelSpec>().unwrap_err(), ModelSpecError::EmptyEntry);
        assert!(matches!("mystery:some-model".parse::<ModelSpec>().unwrap_err(), ModelSpecError::InvalidModel { .. }));
    }

    #[test]
    fn validates_reasoning_effort_for_single_and_alloyed_models() {
        assert!(validate_reasoning_effort("codex:gpt-5.6-sol", Some(ReasoningEffort::Max)).is_ok());
        assert!(validate_reasoning_effort("anthropic:claude-opus-4-6", Some(ReasoningEffort::Xhigh)).is_err());
        assert!(
            validate_reasoning_effort("codex:gpt-5.6-sol,anthropic:claude-opus-4-6", Some(ReasoningEffort::Xhigh))
                .is_err()
        );
    }

    #[test]
    fn reasoning_levels_intersect_across_alloyed_models() {
        let spec: ModelSpec = "codex:gpt-5.6-sol,anthropic:claude-opus-4-6".parse().unwrap();
        assert_eq!(
            spec.reasoning_levels(),
            vec![ReasoningEffort::Low, ReasoningEffort::Medium, ReasoningEffort::High, ReasoningEffort::Max]
        );
    }

    #[test]
    fn clamp_reasoning_effort_snaps_to_nearest_supported_level() {
        let opus: ModelSpec = "anthropic:claude-opus-4-6".parse().unwrap();
        assert_eq!(opus.clamp_reasoning_effort(Some(ReasoningEffort::Xhigh)), Some(ReasoningEffort::High));
        assert_eq!(opus.clamp_reasoning_effort(Some(ReasoningEffort::Max)), Some(ReasoningEffort::Max));
        assert_eq!(opus.clamp_reasoning_effort(None), None);

        let non_reasoning: ModelSpec = "openai:gpt-4o-mini".parse().unwrap();
        assert_eq!(non_reasoning.clamp_reasoning_effort(Some(ReasoningEffort::High)), None);
    }
}
