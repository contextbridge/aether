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
        ReasoningEffort::all()
            .iter()
            .filter(|level| self.0.iter().all(|model| model.reasoning_levels().contains(level)))
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
        let levels = self.reasoning_levels();
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

impl LlmModel {
    pub fn validate_reasoning_effort(&self, effort: ReasoningEffort) -> Result<(), ReasoningEffortError> {
        let supported = self.reasoning_levels();
        if supported.contains(&effort) {
            Ok(())
        } else {
            Err(ReasoningEffortError::Unsupported { model: self.to_string(), effort, supported: supported.to_vec() })
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
