use acp_utils::config_meta::{ConfigOptionMeta, SelectOptionMeta};
use acp_utils::config_option_id::ConfigOptionId;
use aether_auth::OAuthCredentialStorage;
use aether_core::agent_spec::AgentSpec;
use agent_client_protocol::schema::v2::{self as acp, SessionConfigOption, SessionConfigOptionCategory};
use llm::ReasoningEffort;
use llm::catalog::{LlmModel, ModelSpec};
use std::collections::BTreeMap;
use std::ops::Deref;

fn needs_oauth_login(model: &LlmModel, store: &dyn OAuthCredentialStorage) -> bool {
    model.oauth_provider_id().is_some_and(|id| !store.contains(id))
}

pub(crate) fn unavailable_reason(model: &LlmModel, store: &dyn OAuthCredentialStorage) -> String {
    if needs_oauth_login(model, store) {
        return "Needs login".to_string();
    }
    model
        .required_env_var()
        .map_or_else(|| "Unavailable: provider is not configured".to_string(), |var| format!("Unavailable: set {var}"))
}

/// Parse a model spec, requiring every model in it to be in `available`.
pub(crate) fn parse_available_spec(available: &[LlmModel], model_str: &str) -> Option<ModelSpec> {
    model_str.parse::<ModelSpec>().ok().filter(|spec| spec.models().iter().all(|model| available.contains(model)))
}

/// Build the "Model" select config option with all models from all providers.
/// Display names use "Provider: `ModelName`" format.
/// Fully-unavailable providers are collapsed into a single summary line.
pub(crate) fn build_model_config_option(
    available: &[LlmModel],
    current_model: &str,
    all_models: &[LlmModel],
    credential_store: &dyn OAuthCredentialStorage,
) -> SessionConfigOption {
    let mut groups: BTreeMap<&str, Vec<&LlmModel>> = BTreeMap::new();
    for model in all_models {
        groups.entry(model.provider()).or_default().push(model);
    }

    let mut options: Vec<acp::SessionConfigSelectOption> = Vec::new();
    for (provider, models) in groups {
        let display = models[0].provider_display_name();
        if !models.iter().any(|model| available.contains(model)) {
            let count = models.len();
            let noun = if count == 1 { "model" } else { "models" };
            let reason = unavailable_reason(models[0], credential_store);
            options.push(
                acp::SessionConfigSelectOption::new(
                    format!("__unavailable:{provider}"),
                    format!("{display} ({count} {noun})"),
                )
                .description(reason),
            );
            continue;
        }

        for model in models {
            let needs_login = needs_oauth_login(model, credential_store);
            let login_suffix = if needs_login { " (needs login)" } else { "" };
            let name = format!("{display}: {}{login_suffix}", model.display_name());
            let mut option = acp::SessionConfigSelectOption::new(model.to_string(), name);
            let meta = SelectOptionMeta {
                reasoning_levels: model.effective_reasoning_levels(),
                supports_image: model.supports_image(),
                supports_audio: model.supports_audio(),
            };
            if meta != SelectOptionMeta::default() {
                option = option.meta(meta.into_meta());
            }
            if needs_login || !available.contains(model) {
                option = option.description(unavailable_reason(model, credential_store));
            }
            options.push(option);
        }
    }

    let meta = ConfigOptionMeta { multi_select: true };

    SessionConfigOption::select(ConfigOptionId::Model.as_str(), "Model", current_model.to_string(), options)
        .category(SessionConfigOptionCategory::Model)
        .meta(meta.into_meta())
}

fn build_reasoning_effort_config_option(
    current_effort: Option<ReasoningEffort>,
    levels: &[ReasoningEffort],
) -> Option<SessionConfigOption> {
    if levels.is_empty() {
        return None;
    }

    let current = ReasoningEffort::config_str(current_effort).to_string();

    let mut options = vec![acp::SessionConfigSelectOption::new("default", "Default")];
    options.extend(levels.iter().map(|e| {
        let value = e.as_str();
        let mut label = value.to_string();
        label[..1].make_ascii_uppercase();
        acp::SessionConfigSelectOption::new(value, label)
    }));

    Some(
        SessionConfigOption::select(ConfigOptionId::ReasoningEffort.as_str(), "Reasoning Effort", current, options)
            .category(SessionConfigOptionCategory::ThoughtLevel),
    )
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ValidatedMode {
    pub(crate) name: String,
    pub(crate) model: String,
    pub(crate) reasoning_effort: Option<ReasoningEffort>,
}

/// The user-invocable modes for a session: agent specs whose model is currently
/// available, projected to the `(name, model, reasoning_effort)` a session needs
/// for selection and config-option rendering. Owns all mode lookup/rendering.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Modes(Vec<ValidatedMode>);

impl Deref for Modes {
    type Target = [ValidatedMode];

    fn deref(&self) -> &[ValidatedMode] {
        &self.0
    }
}

impl Modes {
    #[cfg(any(test, feature = "testing"))]
    pub(crate) fn new(modes: Vec<ValidatedMode>) -> Self {
        Self(modes)
    }

    pub(crate) fn from_specs(specs: &[AgentSpec], available: &[LlmModel]) -> Self {
        Self(
            specs
                .iter()
                .filter(|spec| spec.exposure.user_invocable)
                .filter_map(|spec| {
                    let model = spec.model.clone();
                    parse_available_spec(available, &model).map(|_| ValidatedMode {
                        name: spec.name.clone(),
                        model,
                        reasoning_effort: spec.reasoning_effort,
                    })
                })
                .collect(),
        )
    }

    /// The model and reasoning effort backing a mode, if it exists.
    pub(crate) fn resolve(&self, mode_name: &str) -> Option<(String, Option<ReasoningEffort>)> {
        self.iter().find(|mode| mode.name == mode_name).map(|mode| (mode.model.clone(), mode.reasoning_effort))
    }

    fn config_option(&self, selected_mode: Option<&str>) -> Option<SessionConfigOption> {
        if self.is_empty() {
            return None;
        }

        let options: Vec<_> =
            self.iter().map(|mode| acp::SessionConfigSelectOption::new(mode.name.clone(), mode.name.clone())).collect();

        let current = selected_mode
            .filter(|selected| self.iter().any(|mode| mode.name == *selected))
            .map(ToOwned::to_owned)
            .or_else(|| self.first().map(|mode| mode.name.clone()))?;

        Some(
            SessionConfigOption::select(ConfigOptionId::Mode.as_str(), "Mode", current, options)
                .category(SessionConfigOptionCategory::Mode),
        )
    }

    pub(crate) fn config_options(
        &self,
        available: &[LlmModel],
        selected_mode: Option<&str>,
        current_model: &str,
        reasoning_effort: Option<ReasoningEffort>,
        all_models: &[LlmModel],
        credential_store: &dyn OAuthCredentialStorage,
    ) -> Vec<SessionConfigOption> {
        let mut options = Vec::new();

        if let Some(mode_option) = self.config_option(selected_mode) {
            options.push(mode_option);
        }

        options.push(build_model_config_option(available, current_model, all_models, credential_store));

        let levels = current_model.parse::<ModelSpec>().map(|spec| spec.reasoning_levels()).unwrap_or_default();

        if let Some(opt) = build_reasoning_effort_config_option(reasoning_effort, &levels) {
            options.push(opt);
        }

        options
    }
}

/// Pick a default model from the available list.
/// Prefers Claude Sonnet 4.5 (latest alias), then first available.
pub(crate) fn pick_default_model(available: &[LlmModel]) -> Option<&LlmModel> {
    // Prefer claude-sonnet-4-5 (latest alias)
    available.iter().find(|m| m.model_id() == "claude-sonnet-4-5").or_else(|| available.first())
}

pub(crate) fn get_all_models(discovered: &[LlmModel]) -> Vec<LlmModel> {
    let mut all = LlmModel::all().to_vec();
    for m in discovered {
        if !all.contains(m) {
            all.push(m.clone());
        }
    }
    all
}

#[cfg(test)]
mod tests {
    use super::*;
    use aether_auth::MemoryCredentialStore;
    use aether_core::agent_spec::AgentSpecExposure;
    use agent_client_protocol::schema::v2::{SessionConfigKind, SessionConfigSelectOption, SessionConfigSelectOptions};
    use llm::catalog::{AnthropicModel, BedrockFoundationModel, BedrockModel, DeepSeekModel, GeminiModel};

    fn test_models() -> Vec<LlmModel> {
        vec![
            LlmModel::Anthropic(AnthropicModel::ClaudeSonnet45),
            LlmModel::Anthropic(AnthropicModel::ClaudeOpus46),
            LlmModel::DeepSeek(DeepSeekModel::DeepseekV4Flash),
            LlmModel::Bedrock(BedrockModel::Foundation(BedrockFoundationModel::AnthropicClaudeSonnet4520250929V10)),
            LlmModel::Gemini(GeminiModel::Gemini25Pro),
        ]
    }

    fn spec(name: &str, model: &str, effort: Option<ReasoningEffort>) -> AgentSpec {
        AgentSpec {
            name: name.to_string(),
            description: name.to_lowercase(),
            model: model.to_string(),
            reasoning_effort: effort,
            exposure: AgentSpecExposure::both(),
            ..AgentSpec::default()
        }
    }

    fn test_specs_with_modes() -> Vec<AgentSpec> {
        vec![
            spec("Planner", "anthropic:claude-sonnet-4-5", Some(ReasoningEffort::High)),
            spec("Coder", "deepseek:deepseek-v4-flash", None),
        ]
    }

    fn test_validated_modes() -> Modes {
        Modes::from_specs(&test_specs_with_modes(), &test_models())
    }

    fn select_options(opt: &SessionConfigOption) -> &[SessionConfigSelectOption] {
        let SessionConfigKind::Select(ref select) = opt.kind else {
            panic!("Expected Select kind");
        };
        let SessionConfigSelectOptions::Ungrouped(ref options) = select.options else {
            panic!("Expected Ungrouped options");
        };
        options
    }

    fn select_current(opt: &SessionConfigOption) -> &str {
        let SessionConfigKind::Select(ref select) = opt.kind else {
            panic!("Expected Select kind");
        };
        select.current_value.0.as_ref()
    }

    fn has_option_id(opts: &[SessionConfigOption], id: &str) -> bool {
        opts.iter().any(|o| o.config_id.0.as_ref() == id)
    }

    fn find_option<'a>(opts: &'a [SessionConfigOption], id: &str) -> &'a SessionConfigOption {
        opts.iter().find(|o| o.config_id.0.as_ref() == id).unwrap_or_else(|| panic!("option '{id}' not found"))
    }

    fn fake_store() -> MemoryCredentialStore {
        MemoryCredentialStore::new()
    }

    fn model_option() -> SessionConfigOption {
        build_model_config_option(&test_models(), "anthropic:claude-sonnet-4-5", LlmModel::all(), &fake_store())
    }

    fn config_opts(model: &str, effort: Option<ReasoningEffort>) -> Vec<SessionConfigOption> {
        test_validated_modes().config_options(&test_models(), None, model, effort, LlmModel::all(), &fake_store())
    }

    #[test]
    fn config_options_include_mode_option_with_mode_category() {
        let options = config_opts("anthropic:claude-sonnet-4-5", Some(ReasoningEffort::High));
        assert_eq!(find_option(&options, "mode").category, Some(SessionConfigOptionCategory::Mode));
    }

    #[test]
    fn resolve_rejects_unknown_mode() {
        assert!(test_validated_modes().resolve("Unknown").is_none());
    }

    #[test]
    fn config_options_returns_single_model_option_without_modes() {
        let opts = Modes::default().config_options(
            &test_models(),
            None,
            "deepseek:deepseek-v4-flash",
            None,
            LlmModel::all(),
            &fake_store(),
        );
        assert_eq!(opts.len(), 2);

        let model_opt = find_option(&opts, "model");
        assert_eq!(select_current(model_opt), "deepseek:deepseek-v4-flash");

        let options = select_options(model_opt);
        for prefix in ["anthropic:", "deepseek:"] {
            assert!(options.iter().any(|o| o.value.0.starts_with(prefix)), "missing {prefix}");
        }
    }

    #[test]
    fn parse_available_spec_known_and_unknown() {
        let models = test_models();
        for (input, expected) in [
            ("anthropic:claude-sonnet-4-5", true),
            ("deepseek:deepseek-v4-flash", true),
            ("anthropic:not-real", false),
            ("mystery:some-model", false),
            ("anthropic:claude-sonnet-4-5,deepseek:deepseek-v4-flash", true),
            ("anthropic:claude-sonnet-4-5,mystery:nope", false),
        ] {
            assert_eq!(parse_available_spec(&models, input).is_some(), expected, "parse_available_spec({input})");
        }
    }

    #[test]
    fn from_specs_keeps_agent_with_bedrock_foundation_model() {
        let bedrock_model = "bedrock:anthropic.claude-sonnet-4-5-20250929-v1:0";
        let specs = vec![spec("BedrockAgent", bedrock_model, None)];
        let modes = Modes::from_specs(&specs, &test_models());
        assert_eq!(modes.len(), 1);
        assert_eq!(modes[0].name, "BedrockAgent");
        assert_eq!(modes[0].model, bedrock_model);
    }

    #[test]
    fn build_model_config_option_includes_multi_select_meta() {
        let opt = model_option();
        assert!(ConfigOptionMeta::from_meta(opt.meta.as_ref()).multi_select);
    }

    #[test]
    fn collapsed_entry_for_fully_unavailable_provider() {
        let opt = model_option();
        let options = select_options(&opt);

        let moonshot = options
            .iter()
            .find(|o| o.value.0.as_ref() == "__unavailable:moonshot")
            .expect("expected collapsed moonshot entry");

        assert!(moonshot.name.starts_with("Moonshot ("), "got: {}", moonshot.name);
        assert!(moonshot.name.ends_with("models)"));
        assert!(moonshot.description.as_deref().is_some_and(|d| d.starts_with("Unavailable:")));
    }

    #[test]
    fn reasoning_option_presence_depends_on_model() {
        let with = config_opts("anthropic:claude-opus-4-6", Some(ReasoningEffort::High));
        assert!(has_option_id(&with, "reasoning_effort"), "should be present for opus");
        assert_eq!(select_current(find_option(&with, "reasoning_effort")), "high");

        let deepseek = config_opts("deepseek:deepseek-v4-flash", None);
        assert!(has_option_id(&deepseek, "reasoning_effort"), "should be present for reasoning-capable deepseek");
    }

    #[test]
    fn mixed_provider_lists_models_individually() {
        let opt = model_option();
        let options = select_options(&opt);

        assert!(
            !options.iter().any(|o| o.value.0.as_ref() == "__unavailable:gemini"),
            "Gemini should not be collapsed when it has available models"
        );
        assert!(options.iter().any(|o| o.value.0.starts_with("gemini:") && o.description.is_none()));
        assert!(options.iter().any(|o| o.value.0.starts_with("gemini:")
            && o.description.as_deref().is_some_and(|d| d.starts_with("Unavailable:"))));
    }
}
