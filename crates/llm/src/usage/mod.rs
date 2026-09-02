//! Token, cost, and session-usage types shared across the workspace.
//!
//! These are wire types. Agent crates produce them and protocol crates
//! serialize them, so they live here as the lowest common dependency. Keep
//! this module small: behavior belongs with the agents that produce the values.

mod call_purpose;
mod context_usage;
mod model_identity;
mod pricing;
mod session;
mod source;
mod tokens;
mod usd;

pub use call_purpose::LlmCallPurpose;
pub use context_usage::ContextUsage;
pub use model_identity::ModelIdentity;
pub use pricing::{ModelPricing, UsageCost};
pub use session::{SessionUsageEvent, SessionUsageTotals};
pub use source::UsageSource;
pub use tokens::{TokenUsage, Tokens};
pub use usd::Usd;

#[cfg(test)]
mod tests;
