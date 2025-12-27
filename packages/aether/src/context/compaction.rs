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
    /// The type of compaction that was performed
    pub strategy: CompactionStrategy,
}

/// Types of compaction strategies
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompactionStrategy {
    /// Removed old tool results to reduce context size
    ToolResultClearing,
    /// Generated an LLM summary of the conversation
    LlmSummarization,
    /// A combination of strategies was used
    Hybrid,
}

impl fmt::Display for CompactionStrategy {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CompactionStrategy::ToolResultClearing => write!(f, "tool_result_clearing"),
            CompactionStrategy::LlmSummarization => write!(f, "llm_summarization"),
            CompactionStrategy::Hybrid => write!(f, "hybrid"),
        }
    }
}

/// Errors that can occur during compaction
#[derive(Debug, Clone)]
pub enum CompactionError {
    /// The LLM failed to generate a summary
    SummarizationFailed(String),
    /// No messages to compact
    NothingToCompact,
    /// Compaction would not reduce context size enough
    InsufficientReduction,
}

impl fmt::Display for CompactionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CompactionError::SummarizationFailed(msg) => {
                write!(f, "summarization failed: {}", msg)
            }
            CompactionError::NothingToCompact => write!(f, "nothing to compact"),
            CompactionError::InsufficientReduction => {
                write!(f, "compaction would not reduce context size sufficiently")
            }
        }
    }
}

impl std::error::Error for CompactionError {}

/// Configuration for context compaction
#[derive(Debug, Clone)]
pub struct CompactionConfig {
    /// Threshold (0.0-1.0) at which to trigger compaction
    pub threshold: f64,
    /// Number of recent tool results to keep during tool result clearing
    pub keep_recent_tool_results: usize,
    /// Whether to automatically compact when threshold is exceeded
    pub auto_compact: bool,
    /// Minimum number of messages before compaction is considered
    pub min_messages_for_compaction: usize,
}

impl Default for CompactionConfig {
    fn default() -> Self {
        Self {
            threshold: super::DEFAULT_COMPACTION_THRESHOLD,
            keep_recent_tool_results: 5,
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

    /// Set the number of recent tool results to keep
    pub fn keep_recent_tool_results(mut self, count: usize) -> Self {
        self.keep_recent_tool_results = count;
        self
    }

    /// Enable or disable automatic compaction
    pub fn auto_compact(mut self, enabled: bool) -> Self {
        self.auto_compact = enabled;
        self
    }

    /// Set minimum messages required before compaction
    pub fn min_messages(mut self, count: usize) -> Self {
        self.min_messages_for_compaction = count;
        self
    }
}

/// A compactor that clears old tool results to reduce context size.
/// This is a lightweight compaction strategy that preserves conversation structure.
#[derive(Debug, Clone)]
pub struct ToolResultClearer {
    keep_recent: usize,
}

impl ToolResultClearer {
    pub fn new(keep_recent: usize) -> Self {
        Self { keep_recent }
    }

    /// Perform tool result clearing on the context.
    /// Returns the number of messages removed, or None if nothing was removed.
    pub fn compact(&self, context: &mut Context) -> Option<CompactionResult> {
        let removed = context.clear_old_tool_results(self.keep_recent);
        if removed > 0 {
            Some(CompactionResult {
                summary: format!("Cleared {} old tool results", removed),
                messages_removed: removed,
                strategy: CompactionStrategy::ToolResultClearing,
            })
        } else {
            None
        }
    }
}

impl Default for ToolResultClearer {
    fn default() -> Self {
        Self::new(5)
    }
}

/// The structured summarization prompt following Factory.ai's anchored iterative approach.
/// This prompt instructs the LLM to create a structured summary with dedicated sections.
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

/// A compactor that uses an LLM to generate structured summaries.
/// This provides the highest quality compaction but incurs API cost and latency.
pub struct LlmCompactor<T: StreamingModelProvider> {
    llm: Arc<T>,
}

impl<T: StreamingModelProvider> LlmCompactor<T> {
    pub fn new(llm: Arc<T>) -> Self {
        Self { llm }
    }

    /// Generate a structured summary of the conversation history.
    pub async fn compact(&self, context: &Context) -> Result<CompactionResult, CompactionError> {
        let messages_to_summarize = context.messages_for_summary();
        if messages_to_summarize.is_empty() {
            return Err(CompactionError::NothingToCompact);
        }

        let messages_count = messages_to_summarize.len();

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

        Ok(CompactionResult {
            summary,
            messages_removed: messages_count,
            strategy: CompactionStrategy::LlmSummarization,
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

/// Truncate long tool results to avoid overwhelming the summarization
fn truncate_result(result: &str, max_len: usize) -> String {
    if result.len() <= max_len {
        result.to_string()
    } else {
        format!("{}... [truncated, {} chars total]", &result[..max_len], result.len())
    }
}

/// A hybrid compactor that first tries tool result clearing,
/// then falls back to LLM summarization if still over threshold.
pub struct HybridCompactor<T: StreamingModelProvider> {
    tool_clearer: ToolResultClearer,
    llm_compactor: LlmCompactor<T>,
}

impl<T: StreamingModelProvider> HybridCompactor<T> {
    pub fn new(llm: Arc<T>, keep_recent_tool_results: usize) -> Self {
        Self {
            tool_clearer: ToolResultClearer::new(keep_recent_tool_results),
            llm_compactor: LlmCompactor::new(llm),
        }
    }

    /// Try tool result clearing first, then LLM summarization if needed.
    /// The `still_over_threshold` function is called after tool clearing to check
    /// if further compaction is needed.
    pub async fn compact<F>(
        &self,
        context: &mut Context,
        still_over_threshold: F,
    ) -> Result<CompactionResult, CompactionError>
    where
        F: Fn() -> bool,
    {
        // Phase 1: Try tool result clearing
        let tool_result = self.tool_clearer.compact(context);
        let did_tool_clearing = tool_result.is_some();

        if let Some(result) = tool_result {
            if !still_over_threshold() {
                // Tool clearing was sufficient
                return Ok(result);
            }
        }

        // Phase 2: Need full LLM summarization
        let llm_result = self.llm_compactor.compact(context).await?;

        // Apply the summary to the context
        let compacted = context.compact(&llm_result.summary);

        Ok(CompactionResult {
            summary: llm_result.summary,
            messages_removed: compacted,
            strategy: if did_tool_clearing {
                CompactionStrategy::Hybrid
            } else {
                CompactionStrategy::LlmSummarization
            },
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::{ChatMessage, ToolCallResult};
    use crate::types::IsoString;

    fn create_context_with_tool_results(count: usize) -> Context {
        let mut messages = vec![
            ChatMessage::System {
                content: "System prompt".to_string(),
                timestamp: IsoString::now(),
            },
            ChatMessage::User {
                content: "Hello".to_string(),
                timestamp: IsoString::now(),
            },
        ];

        for i in 0..count {
            messages.push(ChatMessage::ToolCallResult(Ok(ToolCallResult {
                id: format!("tool_{}", i),
                name: format!("tool_{}", i),
                arguments: "{}".to_string(),
                result: format!("Result {}", i),
            })));
        }

        Context::new(messages, vec![])
    }

    #[test]
    fn test_tool_result_clearer_removes_old_results() {
        let mut ctx = create_context_with_tool_results(10);
        let clearer = ToolResultClearer::new(3);

        let result = clearer.compact(&mut ctx);

        assert!(result.is_some());
        let result = result.unwrap();
        assert_eq!(result.messages_removed, 7);
        assert_eq!(result.strategy, CompactionStrategy::ToolResultClearing);

        // Should have 2 (system + user) + 3 (kept tool results) = 5 messages
        assert_eq!(ctx.message_count(), 5);
    }

    #[test]
    fn test_tool_result_clearer_nothing_to_remove() {
        let mut ctx = create_context_with_tool_results(2);
        let clearer = ToolResultClearer::new(5);

        let result = clearer.compact(&mut ctx);

        assert!(result.is_none());
        assert_eq!(ctx.message_count(), 4); // 2 + 2 tool results
    }

    #[test]
    fn test_compaction_config_default() {
        let config = CompactionConfig::default();
        assert!((config.threshold - 0.85).abs() < 0.001);
        assert_eq!(config.keep_recent_tool_results, 5);
        assert!(config.auto_compact);
        assert_eq!(config.min_messages_for_compaction, 10);
    }

    #[test]
    fn test_compaction_config_builder() {
        let config = CompactionConfig::with_threshold(0.9)
            .keep_recent_tool_results(3)
            .auto_compact(false)
            .min_messages(20);

        assert!((config.threshold - 0.9).abs() < 0.001);
        assert_eq!(config.keep_recent_tool_results, 3);
        assert!(!config.auto_compact);
        assert_eq!(config.min_messages_for_compaction, 20);
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

    #[tokio::test]
    async fn test_llm_compactor_generates_summary() {
        use crate::testing::FakeLlmProvider;

        let summary_response = vec![
            LlmResponse::start("msg-1"),
            LlmResponse::text("## Session Intent\nTest the compaction feature"),
            LlmResponse::Done,
        ];

        let fake_llm = Arc::new(FakeLlmProvider::with_single_response(summary_response));
        let compactor = LlmCompactor::new(fake_llm);

        let context = Context::new(
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

        let result = compactor.compact(&context).await;
        assert!(result.is_ok());

        let result = result.unwrap();
        assert!(result.summary.contains("Session Intent"));
        assert_eq!(result.messages_removed, 1); // Only user message (system excluded)
        assert_eq!(result.strategy, CompactionStrategy::LlmSummarization);
    }

    #[tokio::test]
    async fn test_llm_compactor_handles_error() {
        use crate::testing::FakeLlmProvider;

        let error_response = vec![LlmResponse::Error {
            message: "API error".to_string(),
        }];

        let fake_llm = Arc::new(FakeLlmProvider::with_single_response(error_response));
        let compactor = LlmCompactor::new(fake_llm);

        let context = Context::new(
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

        let result = compactor.compact(&context).await;
        assert!(matches!(result, Err(CompactionError::SummarizationFailed(_))));
    }

    #[tokio::test]
    async fn test_llm_compactor_empty_context() {
        use crate::testing::FakeLlmProvider;

        let fake_llm = Arc::new(FakeLlmProvider::with_single_response(vec![]));
        let compactor = LlmCompactor::new(fake_llm);

        // Context with only system message
        let context = Context::new(
            vec![ChatMessage::System {
                content: "System".to_string(),
                timestamp: IsoString::now(),
            }],
            vec![],
        );

        let result = compactor.compact(&context).await;
        assert!(matches!(result, Err(CompactionError::NothingToCompact)));
    }
}
