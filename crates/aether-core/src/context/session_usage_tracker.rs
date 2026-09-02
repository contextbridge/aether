use llm::{LlmCallPurpose, ModelIdentity, SessionUsageEvent, SessionUsageTotals, TokenUsage, UsageCost, UsageSource};

/// Billable usage for one agent session. The root agent's session tracker captures totals for all agents (e.g. sub-agents).
#[derive(Clone, Debug)]
pub struct SessionUsageTracker {
    source: UsageSource,
    sequence: u64,
    totals: SessionUsageTotals,
}

impl SessionUsageTracker {
    pub fn new(agent_name: impl Into<String>) -> Self {
        Self { source: UsageSource::new(agent_name), sequence: 0, totals: SessionUsageTotals::default() }
    }

    /// Continue from the last persisted event so a resumed session keeps its totals.
    pub fn resume_from(&mut self, last: &SessionUsageEvent) {
        self.sequence = last.sequence;
        self.totals = last.totals.clone();
    }

    pub fn source(&self) -> &UsageSource {
        &self.source
    }

    /// Record one of this agent's own calls, priced from the model's catalog entry.
    pub fn record(&mut self, purpose: LlmCallPurpose, model: ModelIdentity, tokens: TokenUsage) -> SessionUsageEvent {
        let estimated_cost = model.pricing.map(|pricing| pricing.estimate_cost(tokens));
        self.push(self.source.clone(), purpose, model, tokens, estimated_cost)
    }

    /// Fold a sub-agent's sample into these totals. The child's model and cost
    /// stand; its lineage is filled in when the child did not know it.
    pub fn record_child(&mut self, task_id: &str, child: SessionUsageEvent) -> SessionUsageEvent {
        let UsageSource { agent_id, parent_agent_id, task_id: child_task_id, agent_name } = child.source;
        let source = UsageSource {
            agent_id,
            parent_agent_id: parent_agent_id.or_else(|| Some(self.source.agent_id.clone())),
            task_id: child_task_id.or_else(|| Some(task_id.to_string())),
            agent_name,
        };
        self.push(source, child.purpose, child.model, child.tokens, child.estimated_cost)
    }

    fn push(
        &mut self,
        source: UsageSource,
        purpose: LlmCallPurpose,
        model: ModelIdentity,
        tokens: TokenUsage,
        estimated_cost: Option<UsageCost>,
    ) -> SessionUsageEvent {
        self.sequence = self.sequence.saturating_add(1);
        self.totals.add(tokens, estimated_cost);
        SessionUsageEvent {
            sequence: self.sequence,
            source,
            purpose,
            model,
            tokens,
            estimated_cost,
            totals: self.totals.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use llm::testing::{priced_model, session_usage_event};

    #[test]
    fn own_calls_are_sequenced_priced_and_totalled() {
        let mut tracker = SessionUsageTracker::new("root");
        let first = tracker.record(LlmCallPurpose::Chat, ModelIdentity::default(), TokenUsage::new(2, 3));
        let second =
            tracker.record(LlmCallPurpose::Chat, ModelIdentity::of(Some(&priced_model())), TokenUsage::new(5, 7));

        assert_eq!((first.sequence, second.sequence), (1, 2));
        assert_eq!(first.source, *tracker.source());
        assert!(first.estimated_cost.is_none());
        assert!(second.estimated_cost.is_some());
        assert_eq!(second.totals.tokens.input_tokens.get(), 7);
        assert_eq!(second.totals.tokens.output_tokens.get(), 10);
        assert_eq!(second.totals.unpriced_calls, 1);
        assert!(second.totals.estimated_usd.get() > 0.0);
    }

    #[test]
    fn zero_token_samples_without_pricing_stay_fully_priced() {
        let mut tracker = SessionUsageTracker::new("root");
        let event = tracker.record(LlmCallPurpose::Chat, ModelIdentity::default(), TokenUsage::default());
        assert!(event.totals.is_fully_priced());
    }

    #[test]
    fn resumed_tracker_continues_sequence_and_totals() {
        let mut tracker = SessionUsageTracker::new("root");
        let last = tracker.record(LlmCallPurpose::Chat, ModelIdentity::default(), TokenUsage::new(4, 6));

        let mut resumed = SessionUsageTracker::new("root");
        resumed.resume_from(&last);
        let next = resumed.record(LlmCallPurpose::Compaction, ModelIdentity::default(), TokenUsage::new(1, 1));
        assert_eq!(next.sequence, 2);
        assert_eq!(next.totals.tokens.input_tokens.get(), 5);
        assert_eq!(next.totals.tokens.output_tokens.get(), 7);
        assert_eq!(next.totals.unpriced_calls, 2);
    }

    #[test]
    fn child_samples_are_resequenced_totalled_and_given_lineage() {
        let mut tracker = SessionUsageTracker::new("root");
        let mut child = session_usage_event(9, TokenUsage::new(8, 4));
        child.source = UsageSource::new("explorer");

        let folded = tracker.record_child("task_0", child.clone());
        let own = tracker.record(LlmCallPurpose::Chat, ModelIdentity::default(), TokenUsage::new(1, 1));

        assert_eq!(folded.sequence, 1);
        assert_eq!(folded.source.agent_id, child.source.agent_id);
        assert_eq!(folded.source.agent_name, "explorer");
        assert_eq!(folded.source.parent_agent_id.as_deref(), Some(tracker.source().agent_id.as_str()));
        assert_eq!(folded.source.task_id.as_deref(), Some("task_0"));
        assert_eq!(folded.tokens, child.tokens);
        assert_eq!(own.sequence, 2);
        assert_eq!(own.totals.tokens.input_tokens.get(), 9);
        assert_eq!(own.totals.unpriced_calls, 2);
    }

    #[test]
    fn child_samples_keep_lineage_they_already_carry() {
        let mut tracker = SessionUsageTracker::new("root");
        let mut grandchild = session_usage_event(1, TokenUsage::new(1, 1));
        grandchild.source.parent_agent_id = Some("middle".to_string());
        grandchild.source.task_id = Some("task_7".to_string());

        let folded = tracker.record_child("task_0", grandchild);
        assert_eq!(folded.source.parent_agent_id.as_deref(), Some("middle"));
        assert_eq!(folded.source.task_id.as_deref(), Some("task_7"));
    }
}
