use aether_core::agent_spec::AgentSpec;
use aether_project::{AetherSettings, AgentCatalog, SettingsError};
use llm::{ProviderConnectionOverrides, ReasoningEffort};
use std::path::Path;
use thiserror::Error;

const FALLBACK_MODEL: &str = "anthropic:claude-sonnet-4-5";

#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) enum InitialSessionSelection {
    #[default]
    Default,
    Agent(String),
    Model {
        model: String,
        reasoning_effort: Option<ReasoningEffort>,
    },
}

#[derive(Debug, Error)]
pub enum SelectionConflict {
    #[error("an agent and a model cannot both be selected")]
    AgentAndModel,
    #[error("a reasoning effort requires a model")]
    ReasoningEffortWithoutModel,
}

impl InitialSessionSelection {
    pub(crate) fn from_flags(
        agent: Option<String>,
        model: Option<String>,
        reasoning_effort: Option<ReasoningEffort>,
    ) -> Result<Self, SelectionConflict> {
        match (agent, model) {
            (Some(_), Some(_)) => Err(SelectionConflict::AgentAndModel),
            (_, None) if reasoning_effort.is_some() => Err(SelectionConflict::ReasoningEffortWithoutModel),
            (Some(agent), None) => Ok(Self::Agent(agent)),
            (None, Some(model)) => Ok(Self::Model { model, reasoning_effort }),
            (None, None) => Ok(Self::Default),
        }
    }
}

pub(crate) struct ResolvedAgentSelection {
    pub(crate) spec: AgentSpec,
    pub(crate) catalog: AgentCatalog,
}

#[derive(Debug, Error)]
pub enum AgentSelectionError {
    #[error(transparent)]
    Settings(#[from] SettingsError),
    #[error("{0}")]
    Agent(SettingsError),
    #[error("Model error: {0}")]
    Model(String),
}

pub(crate) fn resolve_agent_from_settings(
    cwd: &Path,
    settings: AetherSettings,
    provider_connections: ProviderConnectionOverrides,
    selection: &InitialSessionSelection,
) -> Result<ResolvedAgentSelection, AgentSelectionError> {
    let catalog = AgentCatalog::from_settings_or_empty(cwd, settings)?.with_provider_connections(provider_connections);
    resolve_agent_from_catalog(catalog, selection)
}

pub(crate) fn resolve_agent_from_catalog(
    catalog: AgentCatalog,
    selection: &InitialSessionSelection,
) -> Result<ResolvedAgentSelection, AgentSelectionError> {
    let spec = match selection {
        InitialSessionSelection::Agent(name) => catalog.resolve(name).map_err(AgentSelectionError::Agent)?,
        InitialSessionSelection::Model { model, reasoning_effort } => {
            catalog.default_spec(&model.parse().map_err(AgentSelectionError::Model)?, *reasoning_effort)
        }
        InitialSessionSelection::Default => match catalog.default_agent() {
            Some(spec) => spec.clone(),
            None => catalog.default_spec(&FALLBACK_MODEL.parse().map_err(AgentSelectionError::Model)?, None),
        },
    };

    Ok(ResolvedAgentSelection { spec, catalog })
}

pub fn resolve_agent_spec(
    catalog: &AgentCatalog,
    agent_name: Option<&str>,
) -> Result<AgentSpec, crate::error::CliError> {
    let selection =
        agent_name.map_or(InitialSessionSelection::Default, |name| InitialSessionSelection::Agent(name.to_string()));
    Ok(resolve_agent_from_catalog(catalog.clone(), &selection)?.spec)
}
