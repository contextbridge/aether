use std::time::Duration;

/// Backoff/retry policy for transient LLM provider failures.
///
/// On a retryable `LlmError` (5xx, 429, timeout, network drop, mid-stream
/// interruption), the agent waits `delay` and re-issues the same request.
/// Each successful turn resets the attempt counter.
///
/// Backoff doubles each attempt: `base_delay`, `2 * base_delay`, `4 * base_delay`,
/// ... capped at `max_delay`.
#[derive(Debug, Clone, Copy)]
pub struct RetryConfig {
    pub max_attempts: u32,
    pub base_delay: Duration,
    pub max_delay: Duration,
}

impl Default for RetryConfig {
    fn default() -> Self {
        Self { max_attempts: 5, base_delay: Duration::from_millis(200), max_delay: Duration::from_secs(30) }
    }
}

impl RetryConfig {
    pub fn disabled() -> Self {
        Self { max_attempts: 0, ..Self::default() }
    }

    /// Compute the delay before the next retry. Exponential backoff (×2 per
    /// attempt) starting from `base_delay`, capped at `max_delay`.
    pub(crate) fn compute_delay(&self, attempt: u32) -> Duration {
        let multiplier = 2u32.saturating_pow(attempt.saturating_sub(1));
        self.base_delay.saturating_mul(multiplier).min(self.max_delay)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exponential_backoff_doubles_per_attempt() {
        let config =
            RetryConfig { max_attempts: 5, base_delay: Duration::from_millis(100), max_delay: Duration::from_secs(30) };
        assert_eq!(config.compute_delay(1), Duration::from_millis(100));
        assert_eq!(config.compute_delay(2), Duration::from_millis(200));
        assert_eq!(config.compute_delay(3), Duration::from_millis(400));
        assert_eq!(config.compute_delay(4), Duration::from_millis(800));
    }

    #[test]
    fn backoff_is_capped_by_max_delay() {
        let config = RetryConfig {
            max_attempts: 10,
            base_delay: Duration::from_millis(100),
            max_delay: Duration::from_millis(500),
        };
        assert_eq!(config.compute_delay(10), Duration::from_millis(500));
    }

    #[test]
    fn extreme_attempt_count_does_not_panic() {
        let config = RetryConfig {
            max_attempts: 100,
            base_delay: Duration::from_millis(100),
            max_delay: Duration::from_secs(30),
        };
        assert_eq!(config.compute_delay(99), Duration::from_secs(30));
    }

    #[test]
    fn disabled_config_has_zero_max_attempts() {
        assert_eq!(RetryConfig::disabled().max_attempts, 0);
    }
}
