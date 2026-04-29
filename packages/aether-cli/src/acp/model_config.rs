use acp_utils::config_meta::{ConfigOptionMeta, SelectOptionMeta};
use acp_utils::config_option_id::ConfigOptionId;
use aether_core::agent_spec::AgentSpec;
use agent_client_protocol::schema::{self as acp, SessionConfigOption, SessionConfigOptionCategory};
use llm::ReasoningEffort;
use llm::catalog::LlmModel;
use llm::oauth::OAuthCredentialStorage;
use std::collections::{BTreeMap, HashSet};

fn needs_oauth_login(model: &LlmModel, store: &impl OAuthCredentialStorage) -> bool {
    model.oauth_provider_id().is_some_and(|id| !store.has_credential(id))
}

pub(crate) fn supports_prompt_image(model: &LlmModel) -> bool {
    model.supports_image()
}

pub(crate) fn supports_prompt_audio(model: &LlmModel) -> bool {
    model.supports_audio()
}

pub(crate) fn unavailable_reason(model: &LlmModel, store: &impl OAuthCredentialStorage) -> String {
    if needs_oauth_login(model, store) {
        return "Needs login".to_string();
    }
    model
        .required_env_var()
        .map_or_else(|| "Unavailable: provider is not configured".to_string(), |var| format!("Unavailable: set {var}"))
}

pub(crate) fn model_exists(available: &[LlmModel], model_str: &str) -> bool {
    model_str.split(',').map(str::trim).all(|part| available.iter().any(|m| m.to_string() == part))
}

pub(crate) fn effective_model<'a>(active_model: &'a str, pending_model: Option<&'a str>) -> &'a str {
    pending_model.unwrap_or(active_model)
}

/// Build the "Model" select config option with all models from all providers.
/// Display names use "Provider: `ModelName`" format.
/// Fully-unavailable providers are collapsed into a single summary line.
struct ProviderGroup<'a> {
    models: Vec<&'a LlmModel>,
    available_count: usize,
}

pub(crate) fn build_model_config_option(
    available: &[LlmModel],
    current_model: &str,
    all_models: &[LlmModel],
    credential_store: &impl OAuthCredentialStorage,
) -> SessionConfigOption {
    let available_models: HashSet<String> = available.iter().map(ToString::to_string).collect();

    // Phase 1: Group models by provider, counting available models per provider
    let mut groups: BTreeMap<&str, ProviderGroup<'_>> = BTreeMap::new();
    for m in all_models {
        let value = m.to_string();
        let is_available = available_models.contains(&value);
        let group =
            groups.entry(m.provider()).or_insert_with(|| ProviderGroup { models: Vec::new(), available_count: 0 });
        group.models.push(m);
        if is_available {
            group.available_count += 1;
        }
    }

    // Phase 2: Emit options per group
    let mut options: Vec<acp::SessionConfigSelectOption> = Vec::new();
    for group in groups.values() {
        let display = group.models[0].provider_display_name();
        if group.available_count == 0 {
            // Fully unavailable — emit one collapsed entry
            let provider_key = group.models[0].provider();
            let count = group.models.len();
            let noun = if count == 1 { "model" } else { "models" };
            let name = format!("{display} ({count} {noun})");
            let value = format!("__unavailable:{provider_key}");
            let reason = unavailable_reason(group.models[0], credential_store);
            options.push(acp::SessionConfigSelectOption::new(value, name).description(reason));
        } else {
            // Mixed or fully available — list each model individually
            for m in &group.models {
                let value = m.to_string();
                let is_available = available_models.contains(&value);
                let needs_login = needs_oauth_login(m, credential_store);
                let name = if needs_login {
                    format!("{display}: {} (needs login)", m.display_name())
                } else {
                    format!("{display}: {}", m.display_name())
                };
                let mut option = acp::SessionConfigSelectOption::new(value, name);
                let meta = SelectOptionMeta {
                    reasoning_levels: m.reasoning_levels().to_vec(),
                    supports_image: supports_prompt_image(m),
                    supports_audio: supports_prompt_audio(m),
                };
                if meta != SelectOptionMeta::default() {
                    option = option.meta(meta.into_meta());
                }
                if is_available && !needs_login {
                    options.push(option);
                } else {
                    options.push(option.description(unavailable_reason(m, credential_store)));
                }
            }
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

    let current = current_effort.map_or("none".to_string(), |e| e.as_str().to_string());

    let mut options = vec![acp::SessionConfigSelectOption::new("none", "None")];
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

pub(crate) fn validated_modes_from_specs(specs: &[AgentSpec], available: &[LlmModel]) -> Vec<ValidatedMode> {
    specs
        .iter()
        .filter(|spec| spec.exposure.user_invocable)
        .filter_map(|spec| {
            let model = spec.model.clone();
            if !model_exists(available, &model) {
                return None;
            }

            Some(ValidatedMode { name: spec.name.clone(), model, reasoning_effort: spec.reasoning_effort })
        })
        .collect()
}

pub(crate) fn build_mode_config_option_from_modes(
    validated_modes: &[ValidatedMode],
    selected_mode: Option<&str>,
) -> Option<SessionConfigOption> {
    if validated_modes.is_empty() {
        return None;
    }

    let options: Vec<_> = validated_modes
        .iter()
        .map(|mode| acp::SessionConfigSelectOption::new(mode.name.clone(), mode.name.clone()))
        .collect();

    let current = selected_mode
        .filter(|selected| validated_modes.iter().any(|mode| mode.name == *selected))
        .map(ToOwned::to_owned)
        .or_else(|| validated_modes.first().map(|mode| mode.name.clone()))?;

    Some(
        SessionConfigOption::select(ConfigOptionId::Mode.as_str(), "Mode", current, options)
            .category(SessionConfigOptionCategory::Mode),
    )
}

pub(crate) fn resolve_mode_from_modes(
    validated_modes: &[ValidatedMode],
    mode_name: &str,
) -> Option<(String, Option<ReasoningEffort>)> {
    validated_modes.iter().find(|mode| mode.name == mode_name).map(|mode| (mode.model.clone(), mode.reasoning_effort))
}

pub(crate) fn mode_name_for_state_from_modes(
    validated_modes: &[ValidatedMode],
    model: &str,
    reasoning_effort: Option<ReasoningEffort>,
) -> Option<String> {
    validated_modes
        .iter()
        .find(|mode| mode.model == model && mode.reasoning_effort == reasoning_effort)
        .map(|mode| mode.name.clone())
}

pub(crate) fn build_config_options_from_modes(
    validated_modes: &[ValidatedMode],
    available: &[LlmModel],
    selected_mode: Option<&str>,
    current_model: &str,
    reasoning_effort: Option<ReasoningEffort>,
    all_models: &[LlmModel],
    credential_store: &impl OAuthCredentialStorage,
) -> Vec<SessionConfigOption> {
    let mut options = Vec::new();

    if let Some(mode_option) = build_mode_config_option_from_modes(validated_modes, selected_mode) {
        options.push(mode_option);
    }

    options.push(build_model_config_option(available, current_model, all_models, credential_store));

    let levels = intersect_reasoning_levels(current_model);

    if let Some(opt) = build_reasoning_effort_config_option(reasoning_effort, &levels) {
        options.push(opt);
    }

    options
}

/// Compute the intersection of reasoning levels across all selected models.
/// If any model doesn't support reasoning, the intersection naturally becomes empty.
fn intersect_reasoning_levels(current_model: &str) -> Vec<ReasoningEffort> {
    let mut models = current_model.split(',').map(str::trim).filter_map(|m| m.parse::<LlmModel>().ok());

    let Some(first) = models.next() else {
        return Vec::new();
    };

    let mut result: Vec<ReasoningEffort> = first.reasoning_levels().to_vec();
    for m in models {
        result.retain(|level| m.reasoning_levels().contains(level));
    }
    result
}

/// Pick a default model from the available list.
/// Prefers Claude Sonnet 4.5 (latest alias), then first available.
pub(crate) fn pick_default_model(available: &[LlmModel]) -> Option<&LlmModel> {
    // Prefer claude-sonnet-4-5 (latest alias)
    available.iter().find(|m| m.model_id() == "claude-sonnet-4-5").or_else(|| available.first())
}

#[cfg(test)]
mod tests {
    use super::*;
    use aether_core::agent_spec::{AgentSpecExposure, ToolFilter};
    use agent_client_protocol::schema::{SessionConfigKind, SessionConfigSelectOption, SessionConfigSelectOptions};
    use llm::catalog::{AnthropicModel, DeepSeekModel, GeminiModel};
    use llm::testing::FakeOAuthCredentialStore;

    fn test_models() -> Vec<LlmModel> {
        vec![
            LlmModel::Anthropic(AnthropicModel::ClaudeSonnet45),
            LlmModel::Anthropic(AnthropicModel::ClaudeOpus46),
            LlmModel::DeepSeek(DeepSeekModel::DeepseekChat),
            LlmModel::Gemini(GeminiModel::Gemini25Pro),
        ]
    }

    fn spec(name: &str, model: &str, effort: Option<ReasoningEffort>) -> AgentSpec {
        AgentSpec {
            name: name.to_string(),
            description: name.to_lowercase(),
            model: model.to_string(),
            reasoning_effort: effort,
            prompts: vec![],
            mcp_config_sources: Vec::new(),
            exposure: AgentSpecExposure::both(),
            tools: ToolFilter::default(),
        }
    }

    fn test_specs_with_modes() -> Vec<AgentSpec> {
        vec![
            spec("Planner", "anthropic:claude-sonnet-4-5", Some(ReasoningEffort::High)),
            spec("Coder", "deepseek:deepseek-chat", None),
        ]
    }

    fn test_validated_modes() -> Vec<ValidatedMode> {
        validated_modes_from_specs(&test_specs_with_modes(), &test_models())
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
        opts.iter().any(|o| o.id.0.as_ref() == id)
    }

    fn find_option<'a>(opts: &'a [SessionConfigOption], id: &str) -> &'a SessionConfigOption {
        opts.iter().find(|o| o.id.0.as_ref() == id).unwrap_or_else(|| panic!("option '{id}' not found"))
    }

    fn fake_store() -> FakeOAuthCredentialStore {
        FakeOAuthCredentialStore::new()
    }

    fn config_opts(model: &str, effort: Option<ReasoningEffort>) -> Vec<SessionConfigOption> {
        let validated = test_validated_modes();
        build_config_options_from_modes(&validated, &test_models(), None, model, effort, LlmModel::all(), &fake_store())
    }

    #[test]
    fn build_mode_config_option_from_modes_has_mode_category() {
        let option = build_mode_config_option_from_modes(&test_validated_modes(), Some("Planner"))
            .expect("mode option should exist");
        assert_eq!(option.id.0.as_ref(), "mode");
        assert_eq!(option.category, Some(SessionConfigOptionCategory::Mode));
    }

    #[test]
    fn resolve_mode_from_modes_rejects_unknown_mode() {
        assert!(resolve_mode_from_modes(&test_validated_modes(), "Unknown").is_none());
    }

    #[test]
    fn mode_name_for_state_from_modes_matches_valid_tuple() {
        let selected = mode_name_for_state_from_modes(
            &test_validated_modes(),
            "anthropic:claude-sonnet-4-5",
            Some(ReasoningEffort::High),
        );
        assert_eq!(selected.as_deref(), Some("Planner"));
    }

    #[test]
    fn build_config_options_from_modes_includes_mode_option_when_configured() {
        let modes = test_validated_modes();
        let options = build_config_options_from_modes(
            &modes,
            &test_models(),
            Some("Planner"),
            "anthropic:claude-sonnet-4-5",
            Some(ReasoningEffort::High),
            LlmModel::all(),
            &fake_store(),
        );
        assert!(has_option_id(&options, "mode"));
    }

    #[test]
    fn build_config_options_from_modes_returns_single_model_option() {
        let opts = build_config_options_from_modes(
            &[],
            &test_models(),
            None,
            "deepseek:deepseek-chat",
            None,
            LlmModel::all(),
            &fake_store(),
        );
        assert_eq!(opts.len(), 1);

        let model_opt = find_option(&opts, "model");
        assert_eq!(select_current(model_opt), "deepseek:deepseek-chat");

        let options = select_options(model_opt);
        for prefix in ["anthropic:", "deepseek:"] {
            assert!(options.iter().any(|o| o.value.0.starts_with(prefix)), "missing {prefix}");
        }
    }

    #[test]
    fn model_exists_known_and_unknown() {
        let models = test_models();
        for (input, expected) in [
            ("anthropic:claude-sonnet-4-5", true),
            ("deepseek:deepseek-chat", true),
            ("anthropic:not-real", false),
            ("mystery:some-model", false),
            ("anthropic:claude-sonnet-4-5,deepseek:deepseek-chat", true),
            ("anthropic:claude-sonnet-4-5,mystery:nope", false),
        ] {
            assert_eq!(model_exists(&models, input), expected, "model_exists({input})");
        }
    }

    #[test]
    fn build_model_config_option_includes_multi_select_meta() {
        let opt =
            build_model_config_option(&test_models(), "anthropic:claude-sonnet-4-5", LlmModel::all(), &fake_store());
        assert!(ConfigOptionMeta::from_meta(opt.meta.as_ref()).multi_select);
    }

    #[test]
    fn effective_model_prefers_pending_falls_back_to_active() {
        for (active, pending, expected) in [
            ("anthropic:claude-sonnet-4-5", Some("deepseek:deepseek-chat"), "deepseek:deepseek-chat"),
            ("anthropic:claude-sonnet-4-5", None, "anthropic:claude-sonnet-4-5"),
        ] {
            assert_eq!(effective_model(active, pending), expected);
        }
    }

    #[test]
    fn collapsed_entry_for_fully_unavailable_provider() {
        let opt =
            build_model_config_option(&test_models(), "anthropic:claude-sonnet-4-5", LlmModel::all(), &fake_store());
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

        let without = config_opts("deepseek:deepseek-chat", None);
        assert!(!has_option_id(&without, "reasoning_effort"), "should be absent for deepseek");
    }

    #[test]
    fn mixed_provider_lists_models_individually() {
        let opt =
            build_model_config_option(&test_models(), "anthropic:claude-sonnet-4-5", LlmModel::all(), &fake_store());
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
