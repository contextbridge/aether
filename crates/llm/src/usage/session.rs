use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::{LlmCallPurpose, ModelIdentity, TokenUsage, UsageCost, UsageSource, Usd};

/// Running token totals and cost estimate across every call an agent has seen.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct SessionUsageTotals {
    pub tokens: TokenUsage,
    /// Sum of every priced call's estimated cost.
    pub estimated_usd: Usd,
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
            Some(cost) => self.estimated_usd += cost.total_usd,
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
    pub sequence: u64,
    pub source: UsageSource,
    pub purpose: LlmCallPurpose,
    pub model: ModelIdentity,
    pub tokens: TokenUsage,
    pub estimated_cost: Option<UsageCost>,
    pub totals: SessionUsageTotals,
}
