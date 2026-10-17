#![doc = include_str!("../docs/catalog.md")]

use crate::providers::local::discovery::discover_local_models;
use crate::{ReasoningDisabledSupport, ReasoningEffort};
use std::borrow::Cow;
use std::fmt;
use std::str::FromStr;
use std::sync::LazyLock;

mod bedrock;
mod model_spec;
pub mod transport;

pub use crate::usage::ModelPricing;
pub use bedrock::BedrockModel;
pub use model_spec::{ModelSpec, ModelSpecError, ReasoningEffortError, validate_reasoning_effort};
pub use transport::ModelTransport;

include!(concat!(env!("OUT_DIR"), "/generated.rs"));

/// Returns models whose provider env var is set
pub fn available_models() -> Vec<LlmModel> {
    LlmModel::all()
        .iter()
        .filter(|m| m.required_env_var().is_none_or(|var| std::env::var(var).is_ok()))
        .cloned()
        .collect()
}

pub fn is_provider_env_var(key: &str) -> bool {
    LlmModel::ALL_REQUIRED_ENV_VARS.contains(&key) || OPTIONAL_PROVIDER_ENV_VARS.contains(&key)
}

/// Returns available catalog models plus any locally discovered models.
pub async fn get_local_models() -> Vec<LlmModel> {
    let mut models = available_models();
    let local = discover_local_models().await;
    models.extend(local);
    models
}

impl LlmModel {
    pub(crate) fn parse_for(provider: Provider, model_id: &str) -> Result<Self, String> {
        match provider {
            Provider::Ollama => Ok(Self::Ollama(model_id.to_string())),
            Provider::LlamaCpp => Ok(Self::LlamaCpp(model_id.to_string())),
            Provider::Bedrock => model_id.parse().map(Self::Bedrock),
            provider => Self::all()
                .iter()
                .find(|model| model.provider_enum() == provider && model.model_id() == model_id)
                .cloned()
                .ok_or_else(|| format!("Unknown {provider} model: '{model_id}'")),
        }
    }

    /// Raw model ID (e.g. `claude-opus-4-6`, `llama3.2`)
    pub fn model_id(&self) -> Cow<'static, str> {
        match self.source() {
            ModelSource::Catalog(entry) => Cow::Borrowed(entry.id),
            ModelSource::Runtime(id) => Cow::Owned(id.to_string()),
        }
    }

    /// Human-readable display name (e.g. `Claude Opus 4.6`)
    pub fn display_name(&self) -> Cow<'static, str> {
        match self.source() {
            ModelSource::Catalog(entry) => Cow::Borrowed(entry.display_name),
            ModelSource::Runtime(id) => Cow::Owned(format!("{} {id}", self.provider_display_name())),
        }
    }

    /// Provider identifier (e.g. `anthropic`)
    pub fn provider(&self) -> &'static str {
        self.provider_enum().parser_name()
    }

    /// Human-readable provider name (e.g. `AWS Bedrock`)
    pub fn provider_display_name(&self) -> &'static str {
        self.provider_enum().display_name()
    }

    /// Context window size in tokens (None for dynamic providers)
    pub fn context_window(&self) -> Option<u32> {
        self.entry().map(|entry| entry.context_window)
    }

    /// Required env var for this model's provider (None for local providers)
    pub fn required_env_var(&self) -> Option<&'static str> {
        self.provider_enum().required_env_var()
    }

    /// OAuth provider ID if this model requires OAuth login (e.g. `"codex"`)
    pub fn oauth_provider_id(&self) -> Option<&'static str> {
        self.provider_enum().oauth_provider_id()
    }

    /// Reasoning levels supported by this model (empty if not a reasoning model)
    pub fn reasoning_levels(&self) -> &'static [ReasoningEffort] {
        self.entry().map_or(&[], |entry| entry.reasoning_levels)
    }

    pub fn reasoning_disabled_support(&self) -> ReasoningDisabledSupport {
        self.entry().map_or(ReasoningDisabledSupport::Unsupported, |entry| entry.disabled_support)
    }

    /// Whether this model advertises an explicit off selection.
    pub fn supports_reasoning_off(&self) -> bool {
        self.reasoning_levels().contains(&ReasoningEffort::Disabled)
    }

    /// Whether this model's provider transport can turn reasoning off.
    pub fn supports_reasoning_off_transport(&self) -> bool {
        if !self.supports_reasoning_off() {
            return false;
        }
        match self.provider_enum().info().reasoning_off {
            ReasoningOff::Always => true,
            ReasoningOff::Effort => self.reasoning_disabled_support() == ReasoningDisabledSupport::Effort,
            ReasoningOff::ModelTransport => self.transport().is_some(),
            ReasoningOff::Never => false,
        }
    }

    /// Explicit choices executable by the current adapter. Default is always valid.
    pub fn effective_reasoning_levels(&self) -> Vec<ReasoningEffort> {
        self.reasoning_levels()
            .iter()
            .copied()
            .filter(|effort| *effort != ReasoningEffort::Disabled || self.supports_reasoning_off_transport())
            .collect()
    }

    pub fn validate_reasoning_effort(&self, effort: ReasoningEffort) -> Result<(), ReasoningEffortError> {
        let supported = self.effective_reasoning_levels();
        if effort == ReasoningEffort::Default || supported.contains(&effort) {
            Ok(())
        } else {
            Err(ReasoningEffortError::Unsupported { model: self.to_string(), effort, supported })
        }
    }

    /// Whether this model supports reasoning/extended thinking
    pub fn supports_reasoning(&self) -> bool {
        self.reasoning_levels().iter().any(|effort| effort.is_enabled())
    }

    /// Whether this model supports provider-side prompt caching
    pub fn supports_prompt_caching(&self) -> bool {
        self.entry().is_some_and(|entry| entry.supports_prompt_caching)
    }

    pub fn pricing(&self) -> Option<ModelPricing> {
        self.entry().and_then(|entry| entry.pricing)
    }

    /// Whether this model supports image input
    pub fn supports_image(&self) -> bool {
        self.entry().is_some_and(|entry| entry.supports_image)
    }

    /// Whether this model supports audio input
    pub fn supports_audio(&self) -> bool {
        self.entry().is_some_and(|entry| entry.supports_audio)
    }

    /// Per-model transport override, when the model does not use its
    /// provider's default endpoint and wire protocol.
    pub fn transport(&self) -> Option<ModelTransport> {
        self.entry().and_then(|entry| entry.transport)
    }

    fn entry(&self) -> Option<&'static CatalogEntry> {
        match self.source() {
            ModelSource::Catalog(entry) => Some(entry),
            ModelSource::Runtime(_) => None,
        }
    }
}

impl fmt::Display for LlmModel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}", self.provider(), self.model_id())
    }
}

impl FromStr for LlmModel {
    type Err = String;

    /// Parse a `provider:model` string into an `LlmModel`
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let (provider, model_id) = s.split_once(':').unwrap_or((s, ""));
        Self::parse_for(provider.parse()?, model_id)
    }
}

impl Provider {
    /// Parser name used in `provider:model` strings (e.g. `"anthropic"`).
    pub fn parser_name(self) -> &'static str {
        self.info().parser_name
    }

    /// OpenTelemetry `GenAI` semantic-convention provider name.
    pub fn genai_provider_name(self) -> &'static str {
        self.info().genai_provider_name
    }

    /// Human-readable provider name (e.g. `"AWS Bedrock"`).
    pub fn display_name(self) -> &'static str {
        self.info().display_name
    }

    /// API-key env var the provider requires, if any.
    pub fn required_env_var(self) -> Option<&'static str> {
        self.info().required_env_var
    }

    /// OAuth provider ID if this provider authenticates via OAuth.
    pub fn oauth_provider_id(self) -> Option<&'static str> {
        self.info().oauth_provider_id
    }

    /// Local providers run models on the user's machine — there's no
    /// remote API to call and no env var to satisfy.
    pub fn is_local(self) -> bool {
        self.info().is_local
    }

    fn info(self) -> &'static ProviderInfo {
        &Self::INFO[self as usize]
    }
}

impl fmt::Display for Provider {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.parser_name())
    }
}

impl FromStr for Provider {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::ALL
            .iter()
            .copied()
            .find(|provider| provider.parser_name() == s)
            .ok_or_else(|| format!("Unknown provider: '{s}'"))
    }
}

const OPTIONAL_PROVIDER_ENV_VARS: &[&str] = &[
    "OLLAMA_HOST",
    "AWS_ACCESS_KEY_ID",
    "AWS_BEARER_TOKEN_BEDROCK",
    "AWS_DEFAULT_REGION",
    "AWS_PROFILE",
    "AWS_REGION",
    "AWS_SECRET_ACCESS_KEY",
    "AWS_SESSION_TOKEN",
];

/// Where a model's metadata comes from.
enum ModelSource<'a> {
    /// A model in the generated catalog.
    Catalog(&'static CatalogEntry),
    /// A user-supplied model ID the catalog knows nothing about.
    Runtime(&'a str),
}

/// Generated metadata for one catalog model.
struct CatalogEntry {
    id: &'static str,
    display_name: &'static str,
    context_window: u32,
    reasoning_levels: &'static [ReasoningEffort],
    disabled_support: ReasoningDisabledSupport,
    supports_prompt_caching: bool,
    pricing: Option<ModelPricing>,
    supports_image: bool,
    supports_audio: bool,
    transport: Option<ModelTransport>,
}

/// Generated metadata for one provider.
struct ProviderInfo {
    parser_name: &'static str,
    genai_provider_name: &'static str,
    display_name: &'static str,
    required_env_var: Option<&'static str>,
    oauth_provider_id: Option<&'static str>,
    is_local: bool,
    reasoning_off: ReasoningOff,
}

/// Which models a provider's transport can turn reasoning off for.
enum ReasoningOff {
    /// Every model that advertises an off selection.
    Always,
    /// Models that turn reasoning off with a `none` effort.
    Effort,
    /// Models served from a dedicated transport.
    ModelTransport,
    /// None: the provider cannot turn reasoning off.
    Never,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_fromstr_roundtrip_all_catalog_models() {
        for model in LlmModel::all() {
            let s = model.to_string();
            let parsed: LlmModel = s.parse().unwrap_or_else(|e| panic!("Failed to parse '{s}' back to LlmModel: {e}"));
            assert_eq!(&parsed, model, "roundtrip failed for '{s}'");
        }
    }

    #[test]
    fn display_fromstr_roundtrip_dynamic_providers() {
        let cases = [LlmModel::Ollama("llama3.2".to_string()), LlmModel::LlamaCpp("my-model".to_string())];
        for model in &cases {
            let s = model.to_string();
            let parsed: LlmModel = s.parse().unwrap();
            assert_eq!(&parsed, model);
        }
    }

    #[test]
    fn model_pricing_comes_from_provider_catalog() {
        let model: LlmModel = "anthropic:claude-sonnet-4-5".parse().unwrap();

        assert_eq!(
            model.pricing(),
            Some(ModelPricing {
                input_per_million: 3.0,
                output_per_million: 15.0,
                cache_read_per_million: Some(0.3),
                cache_write_per_million: Some(3.75),
            })
        );
    }

    #[test]
    fn models_without_usage_pricing_do_not_inherit_unrelated_prices() {
        let codex: LlmModel = "codex:gpt-5.5".parse().unwrap();
        let local: LlmModel = "ollama:local-model".parse().unwrap();
        let bedrock_profile: LlmModel = "bedrock:us.anthropic.claude-future-model-v99:0".parse().unwrap();

        assert_eq!(codex.pricing(), None);
        assert_eq!(local.pricing(), None);
        assert_eq!(bedrock_profile.pricing(), None);
    }

    #[test]
    fn codex_gpt56_subscription_models_have_expected_metadata() {
        let expected_levels = &[
            ReasoningEffort::Low,
            ReasoningEffort::Medium,
            ReasoningEffort::High,
            ReasoningEffort::Xhigh,
            ReasoningEffort::Max,
        ];
        for id in ["gpt-5.6-sol", "gpt-5.6-terra", "gpt-5.6-luna"] {
            let model: LlmModel = format!("codex:{id}").parse().unwrap();
            assert_eq!(model.model_id(), id);
            assert_eq!(model.context_window(), Some(272_000));
            assert_eq!(model.reasoning_levels(), expected_levels);
        }

        assert!("codex:gpt-5.6".parse::<LlmModel>().is_err());
        assert!("codex:gpt-5.1-codex".parse::<LlmModel>().is_err());
        assert!("openai:gpt-5.6".parse::<LlmModel>().is_ok());
    }

    #[test]
    fn codex_gpt6_models_are_available_with_subscription_metadata() {
        let expected_levels = &[
            ReasoningEffort::Low,
            ReasoningEffort::Medium,
            ReasoningEffort::High,
            ReasoningEffort::Xhigh,
            ReasoningEffort::Max,
        ];
        for id in ["gpt-6.1-sol", "gpt-6-sol", "gpt-6-astra", "gpt-6-luna"] {
            let model: LlmModel = format!("codex:{id}").parse().unwrap();
            assert_eq!(model.model_id(), id);
            assert_eq!(model.context_window(), Some(272_000));
            assert_eq!(model.reasoning_levels(), expected_levels);
            assert_eq!(model.oauth_provider_id(), Some("codex"));
        }
    }

    #[test]
    fn codex_gpt55_uses_subscription_context_window() {
        let model: LlmModel = "codex:gpt-5.5".parse().unwrap();
        assert_eq!(model.context_window(), Some(272_000));
    }

    #[test]
    fn openai_gpt55_keeps_api_context_window() {
        let model: LlmModel = "openai:gpt-5.5".parse().unwrap();
        assert_eq!(model.context_window(), Some(1_050_000));
    }

    #[test]
    fn bedrock_foundation_model_parses() {
        let model: LlmModel = "bedrock:anthropic.claude-sonnet-4-5-20250929-v1:0".parse().unwrap();

        assert_eq!(model.to_string(), "bedrock:anthropic.claude-sonnet-4-5-20250929-v1:0");
        assert_eq!(model.context_window(), Some(200_000));
    }

    #[test]
    fn bedrock_prompt_caching_support_comes_from_catalog() {
        let claude: LlmModel = "bedrock:anthropic.claude-sonnet-4-5-20250929-v1:0".parse().unwrap();
        let nova: LlmModel = "bedrock:amazon.nova-lite-v1:0".parse().unwrap();
        let profile: LlmModel = "bedrock:us.anthropic.claude-future-model-v99:0".parse().unwrap();

        assert!(claude.supports_prompt_caching());
        assert!(nova.supports_prompt_caching());
        assert!(!profile.supports_prompt_caching());
    }

    #[test]
    fn provider_display_name_returns_human_readable() {
        let anthropic: LlmModel = "anthropic:claude-opus-4-6".parse().unwrap();
        assert_eq!(anthropic.provider_display_name(), "Anthropic");

        let bedrock: LlmModel = "bedrock:anthropic.claude-haiku-4-5-20251001-v1:0".parse().unwrap();
        assert_eq!(bedrock.provider_display_name(), "AWS Bedrock");

        let zai: LlmModel = "zai:glm-4.5".parse().unwrap();
        assert_eq!(zai.provider_display_name(), "ZAI");

        let ollama = LlmModel::Ollama("llama3.2".to_string());
        assert_eq!(ollama.provider_display_name(), "Ollama");
    }

    #[test]
    fn oauth_provider_id_is_codex_only_for_codex_provider() {
        let codex: LlmModel = "codex:gpt-5.5".parse().unwrap();
        assert_eq!(codex.oauth_provider_id(), Some("codex"));

        for non_oauth in
            ["anthropic:claude-opus-4-6", "openai:gpt-5.5", "bedrock:anthropic.claude-sonnet-4-5-20250929-v1:0"]
        {
            let model: LlmModel = non_oauth.parse().unwrap();
            assert_eq!(model.oauth_provider_id(), None, "{non_oauth} should not have OAuth");
        }
        assert_eq!(LlmModel::Ollama("foo".into()).oauth_provider_id(), None);
    }

    #[test]
    fn required_env_var_matches_provider() {
        let cases = [
            ("anthropic:claude-opus-4-6", Some("ANTHROPIC_API_KEY")),
            ("openai:gpt-5.5", Some("OPENAI_API_KEY")),
            ("deepseek:deepseek-v4-flash", Some("DEEPSEEK_API_KEY")),
            ("gemini:gemini-2.5-pro", Some("GEMINI_API_KEY")),
            ("openrouter:anthropic/claude-opus-4.6", Some("OPENROUTER_API_KEY")),
            ("zai:glm-4.5", Some("ZAI_API_KEY")),
            ("xiaomi:mimo-v2.6-pro", Some("XIAOMI_API_KEY")),
            ("codex:gpt-5.5", None),
            ("bedrock:anthropic.claude-sonnet-4-5-20250929-v1:0", None),
        ];
        for (input, expected) in cases {
            let model: LlmModel = input.parse().unwrap();
            assert_eq!(model.required_env_var(), expected, "{input}");
        }
        assert_eq!(LlmModel::Ollama("foo".into()).required_env_var(), None);
        assert_eq!(LlmModel::LlamaCpp("foo".into()).required_env_var(), None);
    }

    #[test]
    fn xiaomi_models_parse_with_catalog_metadata() {
        let model: LlmModel = "xiaomi:mimo-v2.6-pro".parse().unwrap();
        assert_eq!(model.model_id(), "mimo-v2.6-pro");
        assert_eq!(model.context_window(), Some(1_048_576));
        assert_eq!(model.provider_display_name(), "Xiaomi");
        assert!(model.supports_reasoning());
    }

    #[test]
    fn codex_reasoning_models_include_xhigh_level() {
        let codex: LlmModel = "codex:gpt-5.5".parse().unwrap();
        assert_eq!(
            codex.reasoning_levels(),
            &[ReasoningEffort::Low, ReasoningEffort::Medium, ReasoningEffort::High, ReasoningEffort::Xhigh]
        );
        assert!(codex.supports_reasoning());
    }

    #[test]
    fn anthropic_reasoning_models_use_catalog_levels() {
        let claude: LlmModel = "anthropic:claude-opus-4-6".parse().unwrap();
        assert_eq!(
            claude.reasoning_levels(),
            &[ReasoningEffort::Low, ReasoningEffort::Medium, ReasoningEffort::High, ReasoningEffort::Max,]
        );
    }

    #[test]
    fn dynamic_provider_models_have_no_reasoning_levels() {
        assert_eq!(LlmModel::Ollama("llama3.2".into()).reasoning_levels(), []);
        assert!(!LlmModel::Ollama("llama3.2".into()).supports_reasoning());
    }

    #[test]
    fn supports_reasoning_matches_reasoning_levels_emptiness() {
        for model in LlmModel::all() {
            assert_eq!(model.supports_reasoning(), !model.reasoning_levels().is_empty(), "{model}");
        }
    }

    #[test]
    fn dynamic_providers_have_no_context_window() {
        assert_eq!(LlmModel::Ollama("foo".into()).context_window(), None);
        assert_eq!(LlmModel::LlamaCpp("foo".into()).context_window(), None);
    }

    #[test]
    fn bedrock_responses_models_carry_their_endpoint_and_wire_shape() {
        let cases = [
            ("openai.gpt-6.1-sol", "https://bedrock-mantle.${AWS_REGION}.api.aws/openai/v1"),
            ("us.openai.gpt-6.1-sol", "https://bedrock-runtime.${AWS_REGION}.amazonaws.com/openai/v1"),
            ("openai.gpt-5.6-luna", "https://bedrock-mantle.${AWS_REGION}.api.aws/openai/v1"),
            ("openai.gpt-5.6-sol", "https://bedrock-mantle.${AWS_REGION}.api.aws/openai/v1"),
            ("openai.gpt-5.6-terra", "https://bedrock-mantle.${AWS_REGION}.api.aws/openai/v1"),
            ("openai.gpt-6-astra", "https://bedrock-mantle.${AWS_REGION}.api.aws/openai/v1"),
            ("us.openai.gpt-6-luna", "https://bedrock-runtime.${AWS_REGION}.amazonaws.com/openai/v1"),
            ("us.openai.gpt-6-sol", "https://bedrock-runtime.${AWS_REGION}.amazonaws.com/openai/v1"),
            ("us.openai.gpt-6-astra", "https://bedrock-runtime.${AWS_REGION}.amazonaws.com/openai/v1"),
            ("openai.gpt-5.5", "https://bedrock-mantle.${AWS_REGION}.api.aws/openai/v1"),
            ("openai.gpt-5.4", "https://bedrock-mantle.${AWS_REGION}.api.aws/openai/v1"),
            ("xai.grok-4.3", "https://bedrock-mantle.${AWS_REGION}.api.aws/openai/v1"),
            ("openai.gpt-oss-120b", "https://bedrock-mantle.${AWS_REGION}.api.aws/v1"),
            ("openai.gpt-oss-20b", "https://bedrock-mantle.${AWS_REGION}.api.aws/v1"),
        ];

        for (id, expected_api) in cases {
            let model: LlmModel = format!("bedrock:{id}").parse().unwrap();
            assert_eq!(
                model.transport(),
                Some(ModelTransport::OpenAiResponses { base_url_template: expected_api }),
                "{id}"
            );
        }
    }

    #[test]
    fn converse_models_and_profiles_declare_no_transport_override() {
        for id in [
            "anthropic.claude-opus-5",
            "amazon.nova-lite-v1:0",
            "us.anthropic.claude-future-model-v99:0",
            "global.openai.gpt-6-luna",
            "global.openai.gpt-6-sol",
            "global.openai.gpt-6.1-sol",
            "global.openai.gpt-6-astra",
        ] {
            let model: LlmModel = format!("bedrock:{id}").parse().unwrap();
            assert_eq!(model.transport(), None, "{id}");
        }
        assert_eq!(LlmModel::Ollama("llama3.2".into()).transport(), None);
        assert_eq!("anthropic:claude-opus-4-6".parse::<LlmModel>().unwrap().transport(), None);
    }

    #[test]
    fn every_declared_transport_template_resolves() {
        for model in LlmModel::all() {
            let Some(ModelTransport::OpenAiResponses { base_url_template }) = model.transport() else {
                continue;
            };
            let resolved = utils::variables::Vars::new()
                .with_env_lookup(|_| Some("test-region".to_string()))
                .expand(base_url_template)
                .unwrap_or_else(|e| panic!("{model} has an unresolvable endpoint template: {e}"));
            assert!(resolved.starts_with("https://"), "{model} resolved to '{resolved}'");
            assert!(!resolved.contains("${"), "{model} left a placeholder in '{resolved}'");
        }
    }

    #[test]
    fn bedrock_profile_fallback_parses_arbitrary_id() {
        let profile: LlmModel = "bedrock:us.anthropic.future-model-v99:0".parse().unwrap();
        assert_eq!(profile.context_window(), None);
        assert_eq!(profile.reasoning_levels(), []);
        assert!(!profile.supports_prompt_caching());
    }
}
