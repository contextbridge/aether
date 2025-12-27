/// Default threshold for triggering context compaction (85%)
pub const DEFAULT_COMPACTION_THRESHOLD: f64 = 0.85;

/// Tracks token usage from LLM API responses.
/// Uses real usage data from API, not estimation.
#[derive(Debug, Clone, Default)]
pub struct TokenTracker {
    /// Total input tokens across all API calls
    total_input_tokens: u64,
    /// Total output tokens across all API calls
    total_output_tokens: u64,
    /// Input tokens from the most recent API call (current context size)
    last_input_tokens: u32,
    /// Configured context limit for the current provider
    context_limit: u32,
}

impl TokenTracker {
    pub fn new(context_limit: u32) -> Self {
        Self {
            total_input_tokens: 0,
            total_output_tokens: 0,
            last_input_tokens: 0,
            context_limit,
        }
    }

    // Factory methods for common context limits
    pub fn claude_opus() -> Self {
        Self::new(200_000)
    }

    pub fn claude_sonnet() -> Self {
        Self::new(200_000)
    }

    pub fn claude_haiku() -> Self {
        Self::new(200_000)
    }

    pub fn gpt4o() -> Self {
        Self::new(128_000)
    }

    pub fn gpt4o_mini() -> Self {
        Self::new(128_000)
    }

    /// Record usage from an LLM API response
    pub fn record_usage(&mut self, input_tokens: u32, output_tokens: u32) {
        self.total_input_tokens += input_tokens as u64;
        self.total_output_tokens += output_tokens as u64;
        self.last_input_tokens = input_tokens;
    }

    /// Current context usage as a ratio (0.0 - 1.0)
    pub fn usage_ratio(&self) -> f64 {
        if self.context_limit == 0 {
            return 0.0;
        }
        self.last_input_tokens as f64 / self.context_limit as f64
    }

    /// Whether current usage exceeds the given threshold
    pub fn exceeds_threshold(&self, threshold: f64) -> bool {
        self.usage_ratio() >= threshold
    }

    /// Check if context should be compacted based on the given threshold.
    /// This is a convenience method that combines usage ratio check with
    /// a minimum context size requirement to avoid unnecessary compaction
    /// on small conversations.
    pub fn should_compact(&self, threshold: f64) -> bool {
        // Only consider compaction if we have meaningful context usage
        // (at least 10% of the limit, or at least 1000 tokens)
        let min_tokens = std::cmp::max(self.context_limit / 10, 1000);
        self.last_input_tokens >= min_tokens && self.exceeds_threshold(threshold)
    }

    /// Tokens remaining before hitting limit
    pub fn tokens_remaining(&self) -> u32 {
        self.context_limit.saturating_sub(self.last_input_tokens)
    }

    /// Get the context limit
    pub fn context_limit(&self) -> u32 {
        self.context_limit
    }

    /// Get last recorded input tokens (current context size)
    pub fn last_input_tokens(&self) -> u32 {
        self.last_input_tokens
    }

    /// Get total input tokens across all calls
    pub fn total_input_tokens(&self) -> u64 {
        self.total_input_tokens
    }

    /// Get total output tokens across all calls
    pub fn total_output_tokens(&self) -> u64 {
        self.total_output_tokens
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_usage_tracking() {
        let mut tracker = TokenTracker::new(1000);

        tracker.record_usage(500, 100);
        assert_eq!(tracker.usage_ratio(), 0.5);
        assert!(!tracker.exceeds_threshold(0.85));

        tracker.record_usage(900, 50);
        assert_eq!(tracker.usage_ratio(), 0.9);
        assert!(tracker.exceeds_threshold(0.85));
    }

    #[test]
    fn test_tokens_remaining() {
        let mut tracker = TokenTracker::new(1000);
        tracker.record_usage(700, 50);
        assert_eq!(tracker.tokens_remaining(), 300);
    }

    #[test]
    fn test_cumulative_totals() {
        let mut tracker = TokenTracker::new(1000);
        tracker.record_usage(100, 50);
        tracker.record_usage(200, 60);

        assert_eq!(tracker.total_input_tokens(), 300);
        assert_eq!(tracker.total_output_tokens(), 110);
        assert_eq!(tracker.last_input_tokens(), 200); // Only last call
    }

    #[test]
    fn test_zero_context_limit() {
        let tracker = TokenTracker::new(0);
        assert_eq!(tracker.usage_ratio(), 0.0);
    }

    #[test]
    fn test_factory_methods() {
        assert_eq!(TokenTracker::claude_opus().context_limit(), 200_000);
        assert_eq!(TokenTracker::claude_sonnet().context_limit(), 200_000);
        assert_eq!(TokenTracker::claude_haiku().context_limit(), 200_000);
        assert_eq!(TokenTracker::gpt4o().context_limit(), 128_000);
        assert_eq!(TokenTracker::gpt4o_mini().context_limit(), 128_000);
    }

    #[test]
    fn test_exceeds_threshold() {
        let mut tracker = TokenTracker::new(1000);

        tracker.record_usage(500, 100);
        assert!(!tracker.exceeds_threshold(0.6));
        assert!(tracker.exceeds_threshold(0.5));

        tracker.record_usage(850, 50);
        assert!(tracker.exceeds_threshold(0.8));
        assert!(tracker.exceeds_threshold(0.85));
    }

    #[test]
    fn test_should_compact() {
        let mut tracker = TokenTracker::new(10000);

        // Small context shouldn't trigger compaction even if ratio is high
        tracker.record_usage(500, 100);
        assert!(!tracker.should_compact(0.04)); // 5% usage but only 500 tokens

        // Large context at high ratio should trigger
        tracker.record_usage(9000, 100);
        assert!(tracker.should_compact(0.85)); // 90% usage with 9000 tokens

        // Large context below threshold shouldn't trigger
        tracker.record_usage(7000, 100);
        assert!(!tracker.should_compact(0.85)); // 70% usage
    }

    #[test]
    fn test_default_compaction_threshold() {
        use super::DEFAULT_COMPACTION_THRESHOLD;
        assert!((DEFAULT_COMPACTION_THRESHOLD - 0.85).abs() < 0.001);
    }
}
