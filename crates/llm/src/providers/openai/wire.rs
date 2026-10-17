use crate::{Context, TokenUsage};
use serde::Deserialize;
use utils::ReasoningEffort;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PromptCacheKeySource {
    Omit,
    Prefix,
    SessionAffinity,
}

impl PromptCacheKeySource {
    pub(crate) fn resolve(self, context: &Context) -> Option<&str> {
        match self {
            Self::Omit => None,
            Self::Prefix => context.prompt_cache_key(),
            Self::SessionAffinity => context.session_affinity_key(),
        }
    }
}

pub(crate) fn reasoning_effort_param(effort: ReasoningEffort) -> Option<&'static str> {
    match effort {
        ReasoningEffort::Default => None,
        ReasoningEffort::Disabled => Some("none"),
        effort => Some(effort.as_str()),
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
#[allow(clippy::struct_field_names)]
pub(crate) struct PromptTokensDetails {
    pub cached_tokens: Option<u32>,
    pub cache_write_tokens: Option<u32>,
    pub audio_tokens: Option<u32>,
    pub video_tokens: Option<u32>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[allow(clippy::struct_field_names)]
pub(crate) struct CompletionTokensDetails {
    pub reasoning_tokens: Option<u32>,
    pub audio_tokens: Option<u32>,
    pub accepted_prediction_tokens: Option<u32>,
    pub rejected_prediction_tokens: Option<u32>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub(crate) struct Usage {
    #[serde(alias = "input_tokens")]
    pub prompt_tokens: i64,
    #[serde(alias = "output_tokens")]
    pub completion_tokens: i64,
    #[serde(alias = "input_tokens_details")]
    pub prompt_tokens_details: Option<PromptTokensDetails>,
    #[serde(alias = "output_tokens_details")]
    pub completion_tokens_details: Option<CompletionTokensDetails>,
}

impl From<Usage> for TokenUsage {
    fn from(usage: Usage) -> Self {
        let prompt = usage.prompt_tokens_details.unwrap_or_default();
        let completion = usage.completion_tokens_details.unwrap_or_default();
        TokenUsage {
            input_tokens: u32::try_from(usage.prompt_tokens.max(0)).unwrap_or(0).into(),
            output_tokens: u32::try_from(usage.completion_tokens.max(0)).unwrap_or(0).into(),
            cache_read_tokens: prompt.cached_tokens.map(Into::into),
            cache_creation_tokens: prompt.cache_write_tokens.map(Into::into),
            input_audio_tokens: prompt.audio_tokens.map(Into::into),
            input_video_tokens: prompt.video_tokens.map(Into::into),
            reasoning_tokens: completion.reasoning_tokens.map(Into::into),
            output_audio_tokens: completion.audio_tokens.map(Into::into),
            accepted_prediction_tokens: completion.accepted_prediction_tokens.map(Into::into),
            rejected_prediction_tokens: completion.rejected_prediction_tokens.map(Into::into),
        }
    }
}
