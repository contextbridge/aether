use llm::ReasoningEffort;
use llm::catalog::{
    AnthropicModel, BedrockFoundationModel, BedrockModel, CodexModel, LlmModel, OpenRouterModel, OpenaiModel, Provider,
    ZAiModel,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ModelWithReasoning {
    pub model: LlmModel,
    pub reasoning_effort: Option<ReasoningEffort>,
}

impl ModelWithReasoning {
    pub fn new(model: impl Into<LlmModel>, effort: Option<ReasoningEffort>) -> Self {
        let model = model.into();
        let effort = effort.filter(|e| model.reasoning_levels().contains(e));
        Self { model, reasoning_effort: effort }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ProviderRecommendations {
    pub plan: ModelWithReasoning,
    pub build: ModelWithReasoning,
    pub explore: ModelWithReasoning,
}

pub(crate) fn recommended_for_provider(provider: Provider) -> Option<ProviderRecommendations> {
    match provider {
        Provider::Codex => Some(ProviderRecommendations {
            plan: ModelWithReasoning::new(CodexModel::Gpt55, Some(ReasoningEffort::Xhigh)),
            build: ModelWithReasoning::new(CodexModel::Gpt55, Some(ReasoningEffort::Medium)),
            explore: ModelWithReasoning::new(CodexModel::Gpt54Mini, Some(ReasoningEffort::High)),
        }),

        Provider::Openai => Some(ProviderRecommendations {
            plan: ModelWithReasoning::new(OpenaiModel::Gpt55, Some(ReasoningEffort::High)),
            build: ModelWithReasoning::new(OpenaiModel::Gpt55, Some(ReasoningEffort::Medium)),
            explore: ModelWithReasoning::new(OpenaiModel::Gpt5Mini, Some(ReasoningEffort::High)),
        }),

        Provider::Anthropic => Some(ProviderRecommendations {
            plan: ModelWithReasoning::new(AnthropicModel::ClaudeOpus47, Some(ReasoningEffort::High)),
            build: ModelWithReasoning::new(AnthropicModel::ClaudeSonnet46, Some(ReasoningEffort::High)),
            explore: ModelWithReasoning::new(AnthropicModel::ClaudeHaiku45, Some(ReasoningEffort::High)),
        }),

        Provider::Bedrock => Some(ProviderRecommendations {
            plan: ModelWithReasoning::new(
                bedrock(BedrockFoundationModel::AnthropicClaudeOpus47),
                Some(ReasoningEffort::High),
            ),
            build: ModelWithReasoning::new(
                bedrock(BedrockFoundationModel::AnthropicClaudeSonnet46),
                Some(ReasoningEffort::High),
            ),
            explore: ModelWithReasoning::new(
                bedrock(BedrockFoundationModel::AnthropicClaudeHaiku4520251001V10),
                Some(ReasoningEffort::High),
            ),
        }),

        Provider::ZAi => Some(ProviderRecommendations {
            plan: ModelWithReasoning::new(ZAiModel::Glm51, Some(ReasoningEffort::High)),
            build: ModelWithReasoning::new(ZAiModel::Glm51, Some(ReasoningEffort::Medium)),
            explore: ModelWithReasoning::new(ZAiModel::Glm47, Some(ReasoningEffort::High)),
        }),

        Provider::OpenRouter => Some(ProviderRecommendations {
            plan: ModelWithReasoning::new(OpenRouterModel::OpenaiGpt55, Some(ReasoningEffort::High)),
            build: ModelWithReasoning::new(OpenRouterModel::AnthropicClaudeOpus47, Some(ReasoningEffort::High)),
            explore: ModelWithReasoning::new(OpenRouterModel::DeepseekDeepseekV4Flash, Some(ReasoningEffort::High)),
        }),

        Provider::AzureFoundry
        | Provider::DeepSeek
        | Provider::Fireworks
        | Provider::Gemini
        | Provider::Moonshot
        | Provider::Ollama
        | Provider::LlamaCpp => None,
    }
}

fn bedrock(foundation: BedrockFoundationModel) -> BedrockModel {
    BedrockModel::Foundation(foundation)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SUPPORTED: &[Provider] = &[
        Provider::Codex,
        Provider::Openai,
        Provider::Anthropic,
        Provider::Bedrock,
        Provider::ZAi,
        Provider::OpenRouter,
    ];

    #[test]
    fn every_supported_provider_has_recommendations() {
        for provider in SUPPORTED {
            assert!(recommended_for_provider(*provider).is_some(), "missing presets for {provider}");
        }
    }

    #[test]
    fn local_and_unsupported_providers_return_none() {
        assert!(recommended_for_provider(Provider::Ollama).is_none());
        assert!(recommended_for_provider(Provider::LlamaCpp).is_none());
        assert!(recommended_for_provider(Provider::AzureFoundry).is_none());
        assert!(recommended_for_provider(Provider::Fireworks).is_none());
        assert!(recommended_for_provider(Provider::DeepSeek).is_none());
        assert!(recommended_for_provider(Provider::Moonshot).is_none());
    }

    #[test]
    fn recommended_models_match_their_provider_key() {
        for provider in SUPPORTED {
            let recs = recommended_for_provider(*provider).unwrap();
            for role in [&recs.plan, &recs.build, &recs.explore] {
                assert_eq!(role.model.provider_enum(), *provider, "model {} should belong to {provider}", role.model);
            }
        }
    }

    #[test]
    fn reasoning_efforts_are_supported_by_their_model() {
        for provider in SUPPORTED {
            let recs = recommended_for_provider(*provider).unwrap();
            for role in [&recs.plan, &recs.build, &recs.explore] {
                if let Some(effort) = role.reasoning_effort {
                    assert!(
                        role.model.reasoning_levels().contains(&effort),
                        "{} does not support {effort}",
                        role.model
                    );
                }
            }
        }
    }

    #[test]
    fn codex_plan_uses_xhigh_when_supported() {
        let recs = recommended_for_provider(Provider::Codex).unwrap();
        assert_eq!(recs.plan.reasoning_effort, Some(ReasoningEffort::Xhigh));
    }

    #[test]
    fn anthropic_plan_falls_back_to_high_because_xhigh_unsupported() {
        let recs = recommended_for_provider(Provider::Anthropic).unwrap();
        assert_eq!(recs.plan.reasoning_effort, Some(ReasoningEffort::High));
    }
}
