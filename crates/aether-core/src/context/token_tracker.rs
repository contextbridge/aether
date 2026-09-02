use llm::{ContextUsage, TokenUsage, Tokens};

/// Default threshold for triggering context compaction (85%)
pub const DEFAULT_COMPACTION_THRESHOLD: f64 = 0.85;

/// One agent's context window, tracked from real LLM API usage rather than
/// estimation. Owns the `ContextUsage` snapshot the agent publishes and
/// evaluates the compaction policy against it.
#[derive(Debug, Clone, Default)]
pub struct TokenTracker {
    snapshot: ContextUsage,
}

impl TokenTracker {
    pub fn new(context_limit: Option<u32>) -> Self {
        Self { snapshot: ContextUsage { context_limit: context_limit.map(Into::into), ..ContextUsage::default() } }
    }

    /// Record usage from an LLM API response.
    pub fn record_usage(&mut self, sample: TokenUsage) {
        self.snapshot.input_tokens = sample.input_tokens;
        self.refresh_ratio();
    }

    /// The current window as published on `ContextEvent::UsageUpdated`.
    pub fn snapshot(&self) -> &ContextUsage {
        &self.snapshot
    }

    /// Current context usage as a ratio (0.0 - 1.0)
    pub fn usage_ratio(&self) -> Option<f64> {
        self.snapshot.usage_ratio
    }

    /// Whether current usage exceeds the given threshold
    pub fn exceeds_threshold(&self, threshold: f64) -> bool {
        self.usage_ratio().is_some_and(|ratio| ratio >= threshold)
    }

    /// Whether the context needs compaction
    pub fn needs_compaction(&self, estimated_tokens: u32, threshold: f64) -> bool {
        self.snapshot.context_limit.is_some_and(|limit| {
            f64::from(self.snapshot.input_tokens.max(estimated_tokens.into())) >= f64::from(limit) * threshold
        })
    }

    /// Tokens remaining before hitting limit
    pub fn tokens_remaining(&self) -> Option<Tokens> {
        self.snapshot.context_limit.map(|limit| limit.saturating_sub(self.snapshot.input_tokens))
    }

    /// Update the context limit (e.g. when switching models)
    pub fn set_context_limit(&mut self, limit: Option<u32>) {
        self.snapshot.context_limit = limit.map(Into::into);
        self.refresh_ratio();
    }

    /// Forget the last call after context compaction so it cannot immediately
    /// re-trigger compaction.
    pub fn reset_current_usage(&mut self) {
        self.snapshot.input_tokens = Tokens::ZERO;
        self.refresh_ratio();
    }

    fn refresh_ratio(&mut self) {
        self.snapshot.usage_ratio = self
            .snapshot
            .context_limit
            .filter(|limit| !limit.is_zero())
            .map(|limit| f64::from(self.snapshot.input_tokens) / f64::from(limit));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_usage_tracking() {
        let mut tracker = TokenTracker::new(Some(1000));

        tracker.record_usage(TokenUsage::new(500, 100));
        assert_eq!(tracker.usage_ratio(), Some(0.5));
        assert!(!tracker.exceeds_threshold(0.85));

        tracker.record_usage(TokenUsage::new(900, 50));
        assert_eq!(tracker.usage_ratio(), Some(0.9));
        assert!(tracker.exceeds_threshold(0.85));
    }

    #[test]
    fn test_tokens_remaining() {
        let mut tracker = TokenTracker::new(Some(1000));
        tracker.record_usage(TokenUsage::new(700, 50));
        assert_eq!(tracker.tokens_remaining().map(Tokens::get), Some(300));
    }

    #[test]
    fn test_snapshot_reflects_last_call() {
        let mut tracker = TokenTracker::new(Some(1000));
        tracker.record_usage(TokenUsage::new(100, 50));
        tracker.record_usage(TokenUsage::new(200, 60));

        assert_eq!(tracker.snapshot().input_tokens.get(), 200);
    }

    #[test]
    fn test_unknown_context_limit() {
        let tracker = TokenTracker::new(None);
        assert_eq!(tracker.usage_ratio(), None);
        assert_eq!(tracker.tokens_remaining(), None);
        assert!(!tracker.needs_compaction(1_000_000, 0.85));
    }

    #[test]
    fn test_exceeds_threshold() {
        let mut tracker = TokenTracker::new(Some(1000));

        tracker.record_usage(TokenUsage::new(500, 100));
        assert!(!tracker.exceeds_threshold(0.6));
        assert!(tracker.exceeds_threshold(0.5));

        tracker.record_usage(TokenUsage::new(850, 50));
        assert!(tracker.exceeds_threshold(0.8));
        assert!(tracker.exceeds_threshold(0.85));
    }

    #[test]
    fn test_needs_compaction_from_recorded_usage() {
        let mut tracker = TokenTracker::new(Some(10000));

        tracker.record_usage(TokenUsage::new(9000, 100));
        assert!(tracker.needs_compaction(0, 0.85));

        tracker.record_usage(TokenUsage::new(7000, 100));
        assert!(!tracker.needs_compaction(0, 0.85));
    }

    #[test]
    fn test_needs_compaction_from_estimate_before_usage_recorded() {
        let tracker = TokenTracker::new(Some(10000));

        assert!(tracker.needs_compaction(9000, 0.85));
        assert!(!tracker.needs_compaction(1000, 0.85));
    }

    #[test]
    fn test_default_compaction_threshold() {
        use super::DEFAULT_COMPACTION_THRESHOLD;
        assert!((DEFAULT_COMPACTION_THRESHOLD - 0.85).abs() < 0.001);
    }

    #[test]
    fn test_set_context_limit() {
        let mut tracker = TokenTracker::new(Some(200_000));
        assert_eq!(tracker.snapshot().context_limit.map(Tokens::get), Some(200_000));

        tracker.set_context_limit(Some(128_000));
        assert_eq!(tracker.snapshot().context_limit.map(Tokens::get), Some(128_000));

        tracker.record_usage(TokenUsage::new(100_000, 50));
        let expected_ratio = 100_000.0 / 128_000.0;
        assert!((tracker.usage_ratio().unwrap_or_default() - expected_ratio).abs() < 0.001);
    }

    #[test]
    fn test_reset_current_usage() {
        let mut tracker = TokenTracker::new(Some(10000));
        tracker.record_usage(TokenUsage::new(9000, 100));

        assert!(tracker.needs_compaction(0, 0.85));

        tracker.reset_current_usage();

        assert!(tracker.snapshot().input_tokens.is_zero());
        assert_eq!(tracker.usage_ratio(), Some(0.0));
        assert!(!tracker.needs_compaction(0, 0.85));
    }
}
