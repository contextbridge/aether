use std::sync::Arc;

use tokio_stream::StreamExt;

use llm::types::IsoString;
use llm::{ChatMessage, Context, LlmResponse, StreamingModelProvider, TokenUsage};

const SUMMARIZATION_PROMPT: &str = include_str!("prompts/summarization.md");

/// Result of a compaction operation
#[derive(Debug, Clone)]
pub struct CompactionResult {
    /// The summary text that replaces the compacted messages
    pub summary: String,
    /// Number of messages that were removed/compacted
    pub messages_removed: usize,
    /// Token usage reported by the summarization LLM call, if any
    pub usage: Option<TokenUsage>,
}

/// Errors that can occur during compaction
#[derive(Debug, Clone, thiserror::Error)]
pub enum CompactionError {
    /// The LLM failed to generate a summary
    #[error("summarization failed: {0}")]
    SummarizationFailed(String),
    /// No messages to compact
    #[error("nothing to compact")]
    NothingToCompact,
}

/// Configuration for context compaction
#[derive(Debug, Clone)]
pub struct CompactionConfig {
    /// Threshold (0.0-1.0) at which to trigger compaction
    pub threshold: f64,
}

impl Default for CompactionConfig {
    fn default() -> Self {
        Self { threshold: super::DEFAULT_COMPACTION_THRESHOLD }
    }
}

impl CompactionConfig {
    /// Create a new compaction config with the given threshold
    pub fn with_threshold(threshold: f64) -> Self {
        Self { threshold }
    }
}

/// Compacts context by generating an LLM summary
pub struct Compactor {
    llm: Arc<dyn StreamingModelProvider>,
}

impl Compactor {
    pub fn new(llm: Arc<dyn StreamingModelProvider>) -> Self {
        Self { llm }
    }

    /// Generate a structured summary of the conversation.
    ///
    /// Takes the context snapshot by value since the caller applies the summary
    /// to its live context (which may have changed while the summarization
    /// request was in flight) via [`Context::with_compacted_summary`].
    pub async fn compact(&self, mut context: Context) -> Result<CompactionResult, CompactionError> {
        let messages_to_summarize = context.messages_for_summary();
        if messages_to_summarize.is_empty() {
            return Err(CompactionError::NothingToCompact);
        }

        let messages_removed = messages_to_summarize.len();

        context.add_message(ChatMessage::User {
            content: vec![llm::ContentBlock::text(format!(
                "{SUMMARIZATION_PROMPT}\n\nPlease perform a structured handoff of the conversation above."
            ))],
            timestamp: IsoString::now(),
        });

        let mut stream = self.llm.stream_response(&context);
        let mut summary = String::new();
        let mut usage = None;

        while let Some(result) = stream.next().await {
            match result {
                Ok(LlmResponse::Text { chunk }) => {
                    summary.push_str(&chunk);
                }
                Ok(LlmResponse::Usage { tokens }) => usage = Some(tokens),
                Ok(LlmResponse::Done { .. }) => break,
                Ok(LlmResponse::Error { message }) => {
                    return Err(CompactionError::SummarizationFailed(message));
                }
                Err(e) => {
                    return Err(CompactionError::SummarizationFailed(e.to_string()));
                }
                _ => {}
            }
        }

        if summary.is_empty() {
            return Err(CompactionError::SummarizationFailed("LLM returned empty summary".to_string()));
        }

        Ok(CompactionResult { summary, messages_removed, usage })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use llm::ChatMessage;
    use llm::types::IsoString;

    #[test]
    fn test_compaction_config_default() {
        let config = CompactionConfig::default();
        assert!((config.threshold - 0.85).abs() < 0.001);
    }

    #[test]
    fn test_compaction_config_with_threshold() {
        let config = CompactionConfig::with_threshold(0.9);
        assert!((config.threshold - 0.9).abs() < 0.001);
    }

    #[tokio::test]
    async fn test_compactor_generates_summary() {
        use llm::testing::FakeLlmProvider;

        let summary_response = vec![
            LlmResponse::start("msg-1"),
            LlmResponse::text(
                "## Primary Goal\nTest the compaction feature\n\n## Completed Work\n- Wrote initial tests\n\n## File Changes\n- `src/main.rs` — added entry point\n\n## Key Decisions\n- Use structured handoff — preserves context better\n\n## Current State\nRunning compaction tests\n\n## Next Steps\n1. Verify all tests pass\n\n## Open Questions\n(none)\n\n## Constraints\n(none)",
            ),
            LlmResponse::done(),
        ];

        let fake_llm = Arc::new(FakeLlmProvider::with_single_response(summary_response));
        let compactor = Compactor::new(fake_llm);

        let context = Context::new(
            vec![
                ChatMessage::System { content: "System".to_string(), timestamp: IsoString::now() },
                ChatMessage::User {
                    content: vec![llm::ContentBlock::text("Test message")],
                    timestamp: IsoString::now(),
                },
            ],
            vec![],
        );

        let result = compactor.compact(context).await;
        assert!(result.is_ok());

        let result = result.unwrap();
        assert!(result.summary.contains("Primary Goal"));
        assert!(result.summary.contains("File Changes"));
        assert!(result.summary.contains("Next Steps"));
        assert_eq!(result.messages_removed, 1);
    }

    #[tokio::test]
    async fn test_compactor_handles_error() {
        use llm::testing::FakeLlmProvider;

        let error_response = vec![LlmResponse::Error { message: "API error".to_string() }];

        let fake_llm = Arc::new(FakeLlmProvider::with_single_response(error_response));
        let compactor = Compactor::new(fake_llm);

        let context = Context::new(
            vec![
                ChatMessage::System { content: "System".to_string(), timestamp: IsoString::now() },
                ChatMessage::User { content: vec![llm::ContentBlock::text("Test")], timestamp: IsoString::now() },
            ],
            vec![],
        );

        let result = compactor.compact(context).await;
        assert!(matches!(result, Err(CompactionError::SummarizationFailed(_))));
    }

    #[tokio::test]
    async fn test_compactor_empty_context() {
        use llm::testing::FakeLlmProvider;

        let fake_llm = Arc::new(FakeLlmProvider::with_single_response(vec![]));
        let compactor = Compactor::new(fake_llm);

        let context = Context::new(
            vec![ChatMessage::System { content: "System".to_string(), timestamp: IsoString::now() }],
            vec![],
        );

        let result = compactor.compact(context).await;
        assert!(matches!(result, Err(CompactionError::NothingToCompact)));
    }
}
