use llm::TokenUsage;

/// Default threshold for triggering context compaction (85%)
pub const DEFAULT_COMPACTION_THRESHOLD: f64 = 0.85;

/// Tracks token usage from LLM API responses.
/// Uses real usage data from API, not estimation.
///
/// Cumulative totals are stored for the dimensions consumers care about today
/// (input/output, cache read/creation, reasoning). The `last_usage` field
/// preserves the full `TokenUsage` from the most recent API call, so audio /
/// video / prediction dimensions are still accessible without growing
/// dedicated accumulators until a consumer asks for them.
#[derive(Debug, Clone, Default)]
pub struct TokenTracker {
    total_input_tokens: u64,
    total_output_tokens: u64,
    total_cache_read_tokens: u64,
    total_cache_creation_tokens: u64,
    total_reasoning_tokens: u64,
    last_usage: TokenUsage,
    context_limit: Option<u32>,
}

impl TokenTracker {
    pub fn new(context_limit: Option<u32>) -> Self {
        Self { context_limit, ..Self::default() }
    }

    /// Record usage from an LLM API response.
    pub fn record_usage(&mut self, sample: TokenUsage) {
        self.total_input_tokens += u64::from(sample.input_tokens);
        self.total_output_tokens += u64::from(sample.output_tokens);
        self.total_cache_read_tokens += u64::from(sample.cache_read_tokens.unwrap_or(0));
        self.total_cache_creation_tokens += u64::from(sample.cache_creation_tokens.unwrap_or(0));
        self.total_reasoning_tokens += u64::from(sample.reasoning_tokens.unwrap_or(0));
        self.last_usage = sample;
    }

    /// Current context usage as a ratio (0.0 - 1.0)
    pub fn usage_ratio(&self) -> Option<f64> {
        let context_limit = self.context_limit?;
        if context_limit == 0 {
            return None;
        }
        Some(f64::from(self.last_usage.input_tokens) / f64::from(context_limit))
    }

    /// Whether current usage exceeds the given threshold
    pub fn exceeds_threshold(&self, threshold: f64) -> bool {
        self.usage_ratio().is_some_and(|ratio| ratio >= threshold)
    }

    /// Check if context should be compacted based on the given threshold.
    /// This is a convenience method that combines usage ratio check with
    /// a minimum context size requirement to avoid unnecessary compaction
    /// on small conversations.
    pub fn should_compact(&self, threshold: f64) -> bool {
        let Some(context_limit) = self.context_limit else {
            return false;
        };
        let min_tokens = std::cmp::max(context_limit / 10, 1000);
        self.last_usage.input_tokens >= min_tokens && self.exceeds_threshold(threshold)
    }

    /// Tokens remaining before hitting limit
    pub fn tokens_remaining(&self) -> Option<u32> {
        self.context_limit.map(|context_limit| context_limit.saturating_sub(self.last_usage.input_tokens))
    }

    /// Update the context limit (e.g. when switching models)
    pub fn set_context_limit(&mut self, limit: Option<u32>) {
        self.context_limit = limit;
    }

    /// Get the context limit
    pub fn context_limit(&self) -> Option<u32> {
        self.context_limit
    }

    /// Get last recorded input tokens (current context size)
    pub fn last_input_tokens(&self) -> u32 {
        self.last_usage.input_tokens
    }

    /// Get the full `TokenUsage` from the most recent API call. Returns the
    /// default (all zeros / `None`) before any call has been recorded.
    pub fn last_usage(&self) -> &TokenUsage {
        &self.last_usage
    }

    /// Get total input tokens across all calls
    pub fn total_input_tokens(&self) -> u64 {
        self.total_input_tokens
    }

    /// Get total output tokens across all calls
    pub fn total_output_tokens(&self) -> u64 {
        self.total_output_tokens
    }

    /// Get total cache-read tokens across all calls
    pub fn total_cache_read_tokens(&self) -> u64 {
        self.total_cache_read_tokens
    }

    /// Get total cache-creation tokens across all calls
    pub fn total_cache_creation_tokens(&self) -> u64 {
        self.total_cache_creation_tokens
    }

    /// Get total reasoning tokens across all calls
    pub fn total_reasoning_tokens(&self) -> u64 {
        self.total_reasoning_tokens
    }

    /// Reset current usage tracking after context compaction.
    /// Preserves cumulative totals for metrics while clearing `last_usage` to
    /// prevent immediate re-triggering of compaction.
    pub fn reset_current_usage(&mut self) {
        self.last_usage = TokenUsage::default();
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
        assert_eq!(tracker.tokens_remaining(), Some(300));
    }

    #[test]
    fn test_cumulative_totals() {
        let mut tracker = TokenTracker::new(Some(1000));
        tracker.record_usage(TokenUsage::new(100, 50));
        tracker.record_usage(TokenUsage::new(200, 60));

        assert_eq!(tracker.total_input_tokens(), 300);
        assert_eq!(tracker.total_output_tokens(), 110);
        assert_eq!(tracker.last_input_tokens(), 200);
    }

    #[test]
    fn test_unknown_context_limit() {
        let tracker = TokenTracker::new(None);
        assert_eq!(tracker.usage_ratio(), None);
        assert_eq!(tracker.tokens_remaining(), None);
        assert!(!tracker.should_compact(0.85));
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
    fn test_should_compact() {
        let mut tracker = TokenTracker::new(Some(10000));

        tracker.record_usage(TokenUsage::new(500, 100));
        assert!(!tracker.should_compact(0.04));

        tracker.record_usage(TokenUsage::new(9000, 100));
        assert!(tracker.should_compact(0.85));

        tracker.record_usage(TokenUsage::new(7000, 100));
        assert!(!tracker.should_compact(0.85));
    }

    #[test]
    fn test_default_compaction_threshold() {
        use super::DEFAULT_COMPACTION_THRESHOLD;
        assert!((DEFAULT_COMPACTION_THRESHOLD - 0.85).abs() < 0.001);
    }

    #[test]
    fn test_set_context_limit() {
        let mut tracker = TokenTracker::new(Some(200_000));
        assert_eq!(tracker.context_limit(), Some(200_000));

        tracker.set_context_limit(Some(128_000));
        assert_eq!(tracker.context_limit(), Some(128_000));

        // Verify usage ratio recalculates against new limit
        tracker.record_usage(TokenUsage::new(100_000, 50));
        let expected_ratio = 100_000.0 / 128_000.0;
        assert!((tracker.usage_ratio().unwrap_or_default() - expected_ratio).abs() < 0.001);
    }

    #[test]
    fn test_reset_current_usage() {
        let mut tracker = TokenTracker::new(Some(10000));
        tracker.record_usage(TokenUsage::new(9000, 100));

        assert!(tracker.should_compact(0.85));

        tracker.reset_current_usage();

        assert_eq!(tracker.last_input_tokens(), 0);
        assert!(!tracker.should_compact(0.85));
        assert_eq!(tracker.total_input_tokens(), 9000);
        assert_eq!(tracker.total_output_tokens(), 100);
    }

    #[test]
    fn test_cache_and_reasoning_totals_accumulate() {
        let mut tracker = TokenTracker::new(Some(10000));

        tracker.record_usage(TokenUsage {
            input_tokens: 500,
            output_tokens: 100,
            cache_read_tokens: Some(200),
            cache_creation_tokens: Some(50),
            reasoning_tokens: Some(30),
            ..TokenUsage::default()
        });
        tracker.record_usage(TokenUsage {
            input_tokens: 600,
            output_tokens: 80,
            cache_read_tokens: Some(300),
            cache_creation_tokens: None,
            reasoning_tokens: Some(20),
            ..TokenUsage::default()
        });

        assert_eq!(tracker.total_cache_read_tokens(), 500);
        assert_eq!(tracker.total_cache_creation_tokens(), 50);
        assert_eq!(tracker.total_reasoning_tokens(), 50);
    }

    #[test]
    fn test_last_usage_exposes_full_token_usage() {
        let mut tracker = TokenTracker::new(Some(10000));
        let sample = TokenUsage {
            input_tokens: 500,
            output_tokens: 100,
            cache_read_tokens: Some(200),
            cache_creation_tokens: Some(50),
            reasoning_tokens: Some(30),
            input_audio_tokens: Some(5),
            ..TokenUsage::default()
        };

        tracker.record_usage(sample);

        assert_eq!(*tracker.last_usage(), sample);
    }

    #[test]
    fn test_reset_clears_last_usage_but_keeps_cache_totals() {
        let mut tracker = TokenTracker::new(Some(10000));
        tracker.record_usage(TokenUsage {
            input_tokens: 500,
            output_tokens: 100,
            cache_read_tokens: Some(200),
            cache_creation_tokens: Some(50),
            reasoning_tokens: Some(30),
            ..TokenUsage::default()
        });

        tracker.reset_current_usage();

        assert_eq!(*tracker.last_usage(), TokenUsage::default());
        assert_eq!(tracker.total_cache_read_tokens(), 200);
        assert_eq!(tracker.total_cache_creation_tokens(), 50);
        assert_eq!(tracker.total_reasoning_tokens(), 30);
    }
}
