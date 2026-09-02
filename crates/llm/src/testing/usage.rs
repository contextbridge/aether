use crate::{LlmCallPurpose, LlmModel, ModelIdentity, SessionUsageEvent, SessionUsageTotals, TokenUsage, UsageSource};

pub fn session_usage_event(sequence: u64, tokens: TokenUsage) -> SessionUsageEvent {
    let mut totals = SessionUsageTotals::default();
    totals.add(tokens, None);
    SessionUsageEvent {
        sequence,
        source: UsageSource::new("root"),
        purpose: LlmCallPurpose::Chat,
        model: ModelIdentity::default(),
        tokens,
        estimated_cost: None,
        totals,
    }
}

pub fn priced_model() -> LlmModel {
    LlmModel::all().iter().find(|model| model.pricing().is_some()).cloned().expect("catalog has a priced model")
}
