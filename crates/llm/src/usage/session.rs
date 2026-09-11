use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::{LlmCallPurpose, ModelIdentity, TokenUsage, UsageCost, UsageSource, Usd};

/// Running token totals and cost estimate across every call an agent has seen.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct SessionUsageTotals {
    pub tokens: TokenUsage,
    /// Sum of every priced call's estimated cost.
    pub estimated_usd: Usd,
    /// Cumulative estimated cost of non-cached input tokens, in USD.
    pub estimated_input_usd: Usd,
    /// Cumulative estimated cost of output tokens, in USD.
    pub estimated_output_usd: Usd,
    /// Cumulative estimated cost of cache-read tokens, in USD.
    pub estimated_cache_read_usd: Usd,
    /// Cumulative estimated cost of cache-creation tokens, in USD.
    pub estimated_cache_creation_usd: Usd,
    /// Calls with nonzero usage but no catalog pricing, which `estimated_usd`
    /// therefore leaves out.
    pub unpriced_calls: u64,
}

impl SessionUsageTotals {
    pub fn add(&mut self, tokens: TokenUsage, estimated_cost: Option<UsageCost>) {
        if tokens.is_zero() {
            return;
        }
        self.tokens += tokens;
        match estimated_cost {
            Some(cost) => {
                self.estimated_usd += cost.total_usd;
                self.estimated_input_usd += cost.input_usd;
                self.estimated_output_usd += cost.output_usd;
                self.estimated_cache_read_usd += cost.cache_read_usd;
                self.estimated_cache_creation_usd += cost.cache_creation_usd;
            }
            None => self.unpriced_calls += 1,
        }
    }

    /// Whether `estimated_usd` accounts for every call with nonzero usage.
    pub fn is_fully_priced(&self) -> bool {
        self.unpriced_calls == 0
    }
}

/// One provider usage sample, its estimated cost, and the totals after it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct SessionUsageEvent {
    /// Sequence assigned by the emitting tracker, including folded child samples.
    pub sequence: u64,
    /// Attribution of this sample, not the owner of the cumulative totals.
    pub source: UsageSource,
    pub purpose: LlmCallPurpose,
    pub model: ModelIdentity,
    pub tokens: TokenUsage,
    pub estimated_cost: Option<UsageCost>,
    pub totals: SessionUsageTotals,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn cumulative_cost_components_include_each_priced_sample_once() {
        let mut totals = SessionUsageTotals::default();
        let cost = UsageCost {
            input_usd: Usd::new(0.125),
            output_usd: Usd::new(0.25),
            cache_read_usd: Usd::new(0.0625),
            cache_creation_usd: Usd::new(0.0625),
            total_usd: Usd::new(0.5),
        };
        totals.add(TokenUsage::new(10, 2), Some(cost));
        totals.add(TokenUsage::new(20, 4), Some(cost));
        totals.add(TokenUsage::new(5, 1), None);
        totals.add(TokenUsage::default(), Some(cost));
        totals.add(TokenUsage::default(), None);

        let serialized = serde_json::to_value(&totals).unwrap();
        assert_eq!(serialized["estimated_input_usd"], json!(0.25));
        assert_eq!(serialized["estimated_output_usd"], json!(0.5));
        assert_eq!(serialized["estimated_cache_read_usd"], json!(0.125));
        assert_eq!(serialized["estimated_cache_creation_usd"], json!(0.125));
        assert_eq!(totals.estimated_usd, Usd::new(1.0));
        assert_eq!(totals.tokens, TokenUsage::new(35, 7));
        assert_eq!(totals.unpriced_calls, 1);
        assert!(!totals.is_fully_priced());
        assert_eq!(serde_json::from_value::<SessionUsageTotals>(serialized).unwrap(), totals);
    }
}
