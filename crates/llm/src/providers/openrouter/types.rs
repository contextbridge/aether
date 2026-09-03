use serde::{Deserialize, Serialize};

use crate::providers::openai_compatible::types::CompatibleChatRequest;

/// OpenRouter-specific usage configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct OpenRouterUsage {
    #[serde(rename = "include")]
    pub include: bool,
}

/// Cache control marker for `OpenRouter` prompt caching.
/// Enables automatic prefix caching and sticky routing.
/// See: <https://openrouter.ai/docs/guides/best-practices/prompt-caching>
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct CacheControl {
    #[serde(rename = "type")]
    pub cache_type: String,
}

impl CacheControl {
    pub fn ephemeral() -> Self {
        Self { cache_type: "ephemeral".to_string() }
    }
}

/// Custom request type for `OpenRouter` that includes the usage parameter
///
/// `OpenRouter` requires a specific `usage` parameter in the request body to enable
/// token usage tracking. See: <https://openrouter.ai/docs/use-cases/usage-accounting>
#[derive(Debug, Clone, Serialize)]
pub(crate) struct OpenRouterChatRequest {
    #[serde(flatten)]
    request: CompatibleChatRequest,
    usage: OpenRouterUsage,
    cache_control: CacheControl,
    #[serde(skip_serializing_if = "Option::is_none")]
    session_id: Option<String>,
}

impl OpenRouterChatRequest {
    pub(crate) fn from_compatible(request: CompatibleChatRequest, session_id: Option<&str>) -> Self {
        Self {
            request,
            usage: OpenRouterUsage { include: true },
            cache_control: CacheControl::ephemeral(),
            session_id: session_id.map(String::from),
        }
    }
}
