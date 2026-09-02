use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::Tokens;

/// One agent's context window after its most recent LLM call.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct ContextUsage {
    /// `input_tokens` as a fraction of `context_limit`, when the limit is known.
    pub usage_ratio: Option<f64>,
    pub context_limit: Option<Tokens>,
    /// Input tokens on the most recent call, i.e. the current size of the context.
    pub input_tokens: Tokens,
}
