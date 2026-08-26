use agent_client_protocol::schema::v1 as acp;
use llm::ReasoningEffort;
use llm::catalog::{LlmModel, validate_reasoning_effort};
use tracing::error;

use super::config_setting::ConfigSetting;
use super::model::{Modes, parse_available_spec};

#[derive(Debug)]
pub(crate) enum Switch {
    None,
    Agent(String),
    Model(String),
}

/// A selection change captured by a config request that must be applied at the
/// next prompt boundary rather than mid-turn. Mode and model switches are
/// mutually exclusive by construction.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Pending {
    Mode(String),
    Model(String),
}

/// Tracks a session's mode/model selection and any pending switch that should be
/// applied at the next prompt boundary.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct SessionConfigState {
    pub(crate) active_model: String,
    pub(crate) selected_mode: Option<String>,
    pub(crate) pending: Option<Pending>,
    pub(crate) reasoning_effort: Option<ReasoningEffort>,
}

impl SessionConfigState {
    pub(crate) fn with_selection(
        active_model: String,
        selected_mode: Option<String>,
        reasoning_effort: Option<ReasoningEffort>,
    ) -> Self {
        Self { active_model, selected_mode, pending: None, reasoning_effort }
    }

    pub(crate) fn effective_model(&self, modes: &Modes) -> String {
        match &self.pending {
            Some(Pending::Model(target)) => target.clone(),
            Some(Pending::Mode(agent)) => {
                modes.resolve(agent).map_or_else(|| self.active_model.clone(), |(model, _)| model)
            }
            None => self.active_model.clone(),
        }
    }

    pub(crate) fn begin_prompt(&mut self, modes: &Modes) -> Switch {
        match self.pending.take() {
            Some(Pending::Mode(agent)) => {
                if let Some((mode_model, _)) = modes.resolve(&agent) {
                    self.active_model = mode_model;
                }
                self.selected_mode = Some(agent.clone());
                Switch::Agent(agent)
            }
            Some(Pending::Model(target)) if target != self.active_model => {
                self.active_model.clone_from(&target);
                Switch::Model(target)
            }
            _ => Switch::None,
        }
    }

    pub(crate) fn take_agent_switch(&mut self, modes: &Modes) -> Switch {
        if matches!(self.pending, Some(Pending::Mode(_))) { self.begin_prompt(modes) } else { Switch::None }
    }

    pub(crate) fn apply_config_change(
        &mut self,
        modes: &Modes,
        available: &[LlmModel],
        setting: &ConfigSetting,
    ) -> Result<(), acp::Error> {
        match setting {
            ConfigSetting::Mode(value) => {
                let Some((_mode_model, mode_reasoning_effort)) = modes.resolve(value) else {
                    error!("Unknown or invalid mode: {}", value);
                    return Err(acp::Error::invalid_params());
                };

                self.pending =
                    (self.selected_mode.as_deref() != Some(value.as_str())).then(|| Pending::Mode(value.clone()));
                self.reasoning_effort = mode_reasoning_effort;
                self.selected_mode = Some(value.clone());
            }
            ConfigSetting::Model(value) => {
                let Some(spec) = parse_available_spec(available, value) else {
                    error!("Unknown model in set_session_config_option: {}", value);
                    return Err(acp::Error::invalid_params());
                };
                self.pending = (self.active_model != *value).then(|| Pending::Model(value.clone()));
                self.reasoning_effort = spec.clamp_reasoning_effort(self.reasoning_effort);
            }
            ConfigSetting::ReasoningEffort(effort) => {
                let model_id = self.effective_model(modes);
                if let Err(error) = validate_reasoning_effort(&model_id, *effort) {
                    error!("Invalid reasoning effort in set_session_config_option: {error}");
                    return Err(acp::Error::invalid_params());
                }
                self.reasoning_effort = *effort;
            }
        }

        Ok(())
    }
}
