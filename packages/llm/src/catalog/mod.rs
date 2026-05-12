#![doc = include_str!("../docs/catalog.md")]

use crate::providers::local::discovery::discover_local_models;

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
}
