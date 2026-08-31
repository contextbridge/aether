#![doc = include_str!("../docs/catalog.md")]

use crate::providers::local::discovery::discover_local_models;

mod bedrock;
mod model_spec;
mod pricing;
pub mod transport;

pub use bedrock::BedrockModel;
pub use model_spec::{ModelSpec, ModelSpecError, ReasoningEffortError, validate_reasoning_effort};
pub use pricing::ModelPricing;
pub use transport::{ModelTransport, TemplateError, expand_api_template};

include!(concat!(env!("OUT_DIR"), "/generated.rs"));

/// Returns models whose provider env var is set
pub fn available_models() -> Vec<LlmModel> {
    LlmModel::all()
        .iter()
        .filter(|m| m.required_env_var().is_none_or(|var| std::env::var(var).is_ok()))
        .cloned()
        .collect()
}

/// Returns available catalog models plus any locally discovered models.
pub async fn get_local_models() -> Vec<LlmModel> {
    let mut models = available_models();
    let local = discover_local_models().await;
    models.extend(local);
    models
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
            assert_eq!(model.context_window(), Some(372_000));
            assert_eq!(model.reasoning_levels(), expected_levels);
        }

        assert!("codex:gpt-5.6".parse::<LlmModel>().is_err());
        assert!("codex:gpt-5.1-codex".parse::<LlmModel>().is_err());
        assert!("openai:gpt-5.6".parse::<LlmModel>().is_ok());
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
        assert!(LlmModel::Ollama("llama3.2".into()).reasoning_levels().is_empty());
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
            ("openai.gpt-5.6-luna", "https://bedrock-mantle.${AWS_REGION}.api.aws/openai/v1"),
            ("openai.gpt-5.6-sol", "https://bedrock-mantle.${AWS_REGION}.api.aws/openai/v1"),
            ("openai.gpt-5.6-terra", "https://bedrock-mantle.${AWS_REGION}.api.aws/openai/v1"),
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
        for id in ["anthropic.claude-opus-5", "amazon.nova-lite-v1:0", "us.anthropic.claude-future-model-v99:0"] {
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
            let resolved = expand_api_template(base_url_template, |_| Some("test-region".to_string()))
                .unwrap_or_else(|e| panic!("{model} has an unresolvable endpoint template: {e}"));
            assert!(resolved.starts_with("https://"), "{model} resolved to '{resolved}'");
            assert!(!resolved.contains("${"), "{model} left a placeholder in '{resolved}'");
        }
    }

    #[test]
    fn bedrock_profile_fallback_parses_arbitrary_id() {
        let profile: LlmModel = "bedrock:us.anthropic.future-model-v99:0".parse().unwrap();
        assert_eq!(profile.context_window(), None);
        assert!(profile.reasoning_levels().is_empty());
        assert!(!profile.supports_prompt_caching());
    }
}
