use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::{TokenUsage, Usd};

/// Provider/model prices sourced from models.dev, denominated in USD per million tokens.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ModelPricing {
    pub input_per_million: f64,
    pub output_per_million: f64,
    pub cache_read_per_million: Option<f64>,
    pub cache_write_per_million: Option<f64>,
}

impl ModelPricing {
    /// Estimated cost of token usage
    pub fn estimate_cost(self, usage: TokenUsage) -> UsageCost {
        let cache_read = usage.cache_read_tokens.unwrap_or_default();
        let cache_creation = usage.cache_creation_tokens.unwrap_or_default();
        let regular_input = usage.input_tokens.saturating_sub(cache_read + cache_creation);
        let per_million = 1_000_000.0;
        let input_usd = Usd::new(f64::from(regular_input) * self.input_per_million / per_million);
        let output_usd = Usd::new(f64::from(usage.output_tokens) * self.output_per_million / per_million);
        let cache_read_usd = Usd::new(
            f64::from(cache_read) * self.cache_read_per_million.unwrap_or(self.input_per_million) / per_million,
        );
        let cache_creation_usd = Usd::new(
            f64::from(cache_creation) * self.cache_write_per_million.unwrap_or(self.input_per_million) / per_million,
        );
        UsageCost {
            input_usd,
            output_usd,
            cache_read_usd,
            cache_creation_usd,
            total_usd: input_usd + output_usd + cache_read_usd + cache_creation_usd,
        }
    }
}

/// Estimated USD cost of one call, split by token pool.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct UsageCost {
    pub input_usd: Usd,
    pub output_usd: Usd,
    pub cache_read_usd: Usd,
    pub cache_creation_usd: Usd,
    pub total_usd: Usd,
}
