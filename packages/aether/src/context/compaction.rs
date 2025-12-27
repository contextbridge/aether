use std::fmt;
use std::sync::Arc;

use tokio_stream::StreamExt;

use crate::llm::{ChatMessage, Context, LlmResponse, StreamingModelProvider};
use crate::types::IsoString;

/// Result of a compaction operation
#[derive(Debug, Clone)]
pub struct CompactionResult {
    /// The summary text that replaced the compacted messages
    pub summary: String,
    /// Number of messages that were removed/compacted
    pub messages_removed: usize,
}

/// Errors that can occur during compaction
#[derive(Debug, Clone)]
pub enum CompactionError {
    /// The LLM failed to generate a summary
    SummarizationFailed(String),
    /// No messages to compact
    NothingToCompact,
}

impl fmt::Display for CompactionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CompactionError::SummarizationFailed(msg) => {
                write!(f, "summarization failed: {}", msg)
            }
            CompactionError::NothingToCompact => write!(f, "nothing to compact"),
        }
    }
}

impl std::error::Error for CompactionError {}

/// Configuration for context compaction
#[derive(Debug, Clone)]
pub struct CompactionConfig {
    /// Threshold (0.0-1.0) at which to trigger compaction
    pub threshold: f64,
    /// Whether to automatically compact when threshold is exceeded
    pub auto_compact: bool,
    /// Minimum number of messages before compaction is considered
    pub min_messages_for_compaction: usize,
}

impl Default for CompactionConfig {
    fn default() -> Self {
        Self {
            threshold: super::DEFAULT_COMPACTION_THRESHOLD,
            auto_compact: true,
            min_messages_for_compaction: 10,
        }
    }
}

impl CompactionConfig {
    /// Create a new compaction config with the given threshold
    pub fn with_threshold(threshold: f64) -> Self {
        Self {
            threshold,
            ..Default::default()
        }
    }

    /// Set minimum messages required before compaction
    pub fn min_messages(mut self, count: usize) -> Self {
        self.min_messages_for_compaction = count;
        self
    }

    /// Create a configuration with compaction disabled.
    pub fn disabled() -> Self {
        Self {
            auto_compact: false,
            ..Default::default()
        }
    }
}

/// The structured summarization prompt.
const SUMMARIZATION_PROMPT: &str = r#"You are a context compaction assistant. Your task is to create a structured summary of the conversation history that preserves critical information while significantly reducing token usage.

Create a summary with the following sections:

## Session Intent
What is the user trying to accomplish? State the main goal or objective.

## Accomplishments
What has been completed so far? List the key achievements in bullet points.

## File Modifications
List any files that were created, modified, or deleted:
- filename: brief description of changes

## Key Decisions
What important decisions were made during the conversation?
- Decision: reasoning

## Current State
What is currently in progress or was just completed?

## Next Steps
What remains to be done? What was the user's last request?

## Constraints
Any specific requirements or constraints the user mentioned?

---

Be concise but preserve all information needed to continue the task effectively. Do not include pleasantries or meta-commentary about the summary itself."#;

/// Compacts context by generating an LLM summary
pub struct Compactor<T: StreamingModelProvider> {
    llm: Arc<T>,
}

impl<T: StreamingModelProvider> Compactor<T> {
    pub fn new(llm: Arc<T>) -> Self {
        Self { llm }
    }

    /// Generate a structured summary of the conversation and apply it to the context.
    pub async fn compact(&self, context: &mut Context) -> Result<CompactionResult, CompactionError> {
        let messages_to_summarize = context.messages_for_summary();
        if messages_to_summarize.is_empty() {
            return Err(CompactionError::NothingToCompact);
        }

        // Format the conversation history for summarization
        let conversation_text = format_messages_for_summary(&messages_to_summarize);

        // Create a context for the summarization request
        let summary_context = Context::new(
            vec![
                ChatMessage::System {
                    content: SUMMARIZATION_PROMPT.to_string(),
                    timestamp: IsoString::now(),
                },
                ChatMessage::User {
                    content: format!(
                        "Please create a structured summary of the following conversation:\n\n{}",
                        conversation_text
                    ),
                    timestamp: IsoString::now(),
                },
            ],
            vec![], // No tools for summarization
        );

        // Stream the response and collect the summary
        let mut stream = self.llm.stream_response(&summary_context);
        let mut summary = String::new();

        while let Some(result) = stream.next().await {
            match result {
                Ok(LlmResponse::Text { chunk }) => {
                    summary.push_str(&chunk);
                }
                Ok(LlmResponse::Done) => break,
                Ok(LlmResponse::Error { message }) => {
                    return Err(CompactionError::SummarizationFailed(message));
                }
                Err(e) => {
                    return Err(CompactionError::SummarizationFailed(e.to_string()));
                }
                _ => {} // Ignore other response types
            }
        }

        if summary.is_empty() {
            return Err(CompactionError::SummarizationFailed(
                "LLM returned empty summary".to_string(),
            ));
        }

        // Apply the summary to the context
        let messages_removed = context.compact(&summary);

        Ok(CompactionResult {
            summary,
            messages_removed,
        })
    }
}

/// Format messages for the summarization prompt
fn format_messages_for_summary(messages: &[&ChatMessage]) -> String {
    let mut output = String::new();

    for msg in messages {
        match msg {
            ChatMessage::User { content, .. } => {
                output.push_str(&format!("USER: {}\n\n", content));
            }
            ChatMessage::Assistant {
                content,
                tool_calls,
                ..
            } => {
                output.push_str(&format!("ASSISTANT: {}\n", content));
                if !tool_calls.is_empty() {
                    output.push_str("Tool calls:\n");
                    for tc in tool_calls {
                        output.push_str(&format!("  - {} ({})\n", tc.name, tc.id));
                    }
                }
                output.push('\n');
            }
            ChatMessage::ToolCallResult(Ok(result)) => {
                output.push_str(&format!(
                    "TOOL RESULT [{}]: {}\n\n",
                    result.name,
                    truncate_result(&result.result, 500)
                ));
            }
            ChatMessage::ToolCallResult(Err(error)) => {
                output.push_str(&format!(
                    "TOOL ERROR [{}]: {}\n\n",
                    error.name, error.error
                ));
            }
            ChatMessage::Summary { content, .. } => {
                output.push_str(&format!("PREVIOUS SUMMARY:\n{}\n\n", content));
            }
            ChatMessage::Error { message, .. } => {
                output.push_str(&format!("ERROR: {}\n\n", message));
            }
            ChatMessage::System { .. } => {
                // System messages are excluded from summarization
            }
        }
    }

    output
}

/// Truncate long tool results to avoid overwhelming the summarization.
/// Uses char_indices to ensure we don't split multi-byte UTF-8 characters.
fn truncate_result(result: &str, max_len: usize) -> String {
    if result.len() <= max_len {
        return result.to_string();
    }

    // Find a safe UTF-8 boundary at or before max_len
    let truncate_at = result
        .char_indices()
        .take_while(|(idx, _)| *idx < max_len)
        .last()
        .map(|(idx, c)| idx + c.len_utf8())
        .unwrap_or(0);

    format!(
        "{}... [truncated, {} chars total]",
        &result[..truncate_at],
        result.chars().count()
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::ChatMessage;
    use crate::types::IsoString;

    #[test]
    fn test_compaction_config_default() {
        let config = CompactionConfig::default();
        assert!((config.threshold - 0.85).abs() < 0.001);
        assert!(config.auto_compact);
        assert_eq!(config.min_messages_for_compaction, 10);
    }

    #[test]
    fn test_compaction_config_with_threshold() {
        let config = CompactionConfig::with_threshold(0.9).min_messages(20);

        assert!((config.threshold - 0.9).abs() < 0.001);
        assert!(config.auto_compact);
        assert_eq!(config.min_messages_for_compaction, 20);
    }

    #[test]
    fn test_compaction_config_disabled() {
        let config = CompactionConfig::disabled();
        assert!(!config.auto_compact);
        assert!((config.threshold - 0.85).abs() < 0.001);
    }

    #[test]
    fn test_format_messages_for_summary() {
        let messages = vec![
            ChatMessage::User {
                content: "Hello, can you help me?".to_string(),
                timestamp: IsoString::now(),
            },
            ChatMessage::Assistant {
                content: "Of course!".to_string(),
                timestamp: IsoString::now(),
                tool_calls: vec![],
            },
        ];

        let refs: Vec<&ChatMessage> = messages.iter().collect();
        let formatted = format_messages_for_summary(&refs);

        assert!(formatted.contains("USER: Hello, can you help me?"));
        assert!(formatted.contains("ASSISTANT: Of course!"));
    }

    #[test]
    fn test_truncate_result() {
        let short = "short result";
        assert_eq!(truncate_result(short, 100), short);

        let long = "a".repeat(1000);
        let truncated = truncate_result(&long, 100);
        assert!(truncated.len() < long.len());
        assert!(truncated.contains("[truncated"));
        assert!(truncated.contains("1000 chars total"));
    }

    #[test]
    fn test_truncate_result_utf8_safety() {
        // Multi-byte UTF-8 characters: 日本語 (each is 3 bytes)
        let japanese = "日本語テスト"; // 6 chars, 18 bytes

        // Truncate at byte 5 - should not panic and should find safe boundary
        let truncated = truncate_result(japanese, 5);
        assert!(truncated.contains("[truncated"));
        // Should truncate to "日" (3 bytes) since "日本" would be 6 bytes > 5
        assert!(truncated.starts_with("日"));

        // Emoji test: 🦀 is 4 bytes
        let emoji = "🦀🦀🦀🦀🦀"; // 5 chars, 20 bytes
        let truncated = truncate_result(emoji, 10);
        assert!(truncated.contains("[truncated"));
        // Should include 2 crabs (8 bytes) since 3 would be 12 > 10
        assert!(truncated.starts_with("🦀🦀"));
    }

    #[tokio::test]
    async fn test_compactor_generates_summary() {
        use crate::testing::FakeLlmProvider;

        let summary_response = vec![
            LlmResponse::start("msg-1"),
            LlmResponse::text("## Session Intent\nTest the compaction feature"),
            LlmResponse::Done,
        ];

        let fake_llm = Arc::new(FakeLlmProvider::with_single_response(summary_response));
        let compactor = Compactor::new(fake_llm);

        let mut context = Context::new(
            vec![
                ChatMessage::System {
                    content: "System".to_string(),
                    timestamp: IsoString::now(),
                },
                ChatMessage::User {
                    content: "Test message".to_string(),
                    timestamp: IsoString::now(),
                },
            ],
            vec![],
        );

        let result = compactor.compact(&mut context).await;
        assert!(result.is_ok());

        let result = result.unwrap();
        assert!(result.summary.contains("Session Intent"));
        assert_eq!(result.messages_removed, 1); // Only user message (system excluded)
    }

    #[tokio::test]
    async fn test_compactor_handles_error() {
        use crate::testing::FakeLlmProvider;

        let error_response = vec![LlmResponse::Error {
            message: "API error".to_string(),
        }];

        let fake_llm = Arc::new(FakeLlmProvider::with_single_response(error_response));
        let compactor = Compactor::new(fake_llm);

        let mut context = Context::new(
            vec![
                ChatMessage::System {
                    content: "System".to_string(),
                    timestamp: IsoString::now(),
                },
                ChatMessage::User {
                    content: "Test".to_string(),
                    timestamp: IsoString::now(),
                },
            ],
            vec![],
        );

        let result = compactor.compact(&mut context).await;
        assert!(matches!(result, Err(CompactionError::SummarizationFailed(_))));
    }

    #[tokio::test]
    async fn test_compactor_empty_context() {
        use crate::testing::FakeLlmProvider;

        let fake_llm = Arc::new(FakeLlmProvider::with_single_response(vec![]));
        let compactor = Compactor::new(fake_llm);

        // Context with only system message
        let mut context = Context::new(
            vec![ChatMessage::System {
                content: "System".to_string(),
                timestamp: IsoString::now(),
            }],
            vec![],
        );

        let result = compactor.compact(&mut context).await;
        assert!(matches!(result, Err(CompactionError::NothingToCompact)));
    }
}
