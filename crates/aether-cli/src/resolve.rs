use aether_core::agent_spec::AgentSpec;
use aether_project::{AetherSettings, AgentCatalog, SettingsError};
use llm::{ProviderConnectionOverrides, ReasoningEffort};
use std::path::Path;
use thiserror::Error;

const FALLBACK_MODEL: &str = "anthropic:claude-sonnet-4-5";

#[derive(Clone, Debug, Default)]
pub(crate) enum InitialSessionSelection {
    #[default]
    Default,
    Agent(String),
    Model {
        model: String,
        reasoning_effort: Option<ReasoningEffort>,
    },
}

impl InitialSessionSelection {
    pub(crate) fn agent(name: String) -> Self {
        Self::Agent(name)
    }

    pub(crate) fn model(model: String, reasoning_effort: Option<ReasoningEffort>) -> Self {
        Self::Model { model, reasoning_effort }
    }
}

pub(crate) struct ResolvedAgentSelection {
    pub(crate) spec: AgentSpec,
    pub(crate) catalog: AgentCatalog,
}

#[derive(Debug, Error)]
pub(crate) enum AgentSelectionError {
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
    resolve_agent_from_catalog(catalog.clone(), &selection).map(|resolved| resolved.spec).map_err(|error| match error {
        AgentSelectionError::Settings(error) | AgentSelectionError::Agent(error) => {
            crate::error::CliError::AgentError(error.to_string())
        }
        AgentSelectionError::Model(error) => crate::error::CliError::ModelError(error),
    })
}
