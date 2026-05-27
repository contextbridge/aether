use serde::{Deserialize, Serialize};

use crate::catalog::LlmModel;
use crate::chat_message::AssistantReasoning;
use crate::reasoning::ReasoningEffort;
use crate::types::IsoString;

use super::{ChatMessage, ToolDefinition};

#[doc = include_str!("docs/context.md")]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Context {
    messages: Vec<ChatMessage>,
    tools: Vec<ToolDefinition>,
    #[serde(skip)]
    reasoning_effort: Option<ReasoningEffort>,
    #[serde(skip)]
    prompt_cache_key: Option<String>,
}

impl Context {
    pub fn new(messages: Vec<ChatMessage>, tools: Vec<ToolDefinition>) -> Self {
        Self { messages, tools, reasoning_effort: None, prompt_cache_key: None }
    }

    pub fn prompt_cache_key(&self) -> Option<&str> {
        self.prompt_cache_key.as_deref()
    }

    pub fn set_prompt_cache_key(&mut self, key: Option<String>) {
        self.prompt_cache_key = key;
    }

    pub fn reasoning_effort(&self) -> Option<ReasoningEffort> {
        self.reasoning_effort
    }

    pub fn set_reasoning_effort(&mut self, effort: Option<ReasoningEffort>) {
        self.reasoning_effort = effort;
    }

    pub fn add_message(&mut self, message: ChatMessage) {
        self.messages.push(message);
    }

    pub fn set_tools(&mut self, tools: Vec<ToolDefinition>) {
        self.tools = tools;
    }

    pub fn set_system_content(&mut self, content: String) {
        if let Some(ChatMessage::System { content: existing, .. }) = self.messages.first_mut() {
            *existing = content;
        } else {
            self.messages.insert(0, ChatMessage::System { content, timestamp: IsoString::now() });
        }
    }

    pub fn messages(&self) -> &Vec<ChatMessage> {
        &self.messages
    }

    pub fn tools(&self) -> &Vec<ToolDefinition> {
        &self.tools
    }

    /// Returns the number of messages in the context
    pub fn message_count(&self) -> usize {
        self.messages.len()
    }

    /// Estimate total token count using the ~4 bytes/token heuristic.
    /// Includes messages and tool definitions. Used for pre-flight overflow detection.
    pub fn estimated_token_count(&self) -> u32 {
        let message_bytes: usize = self.messages.iter().map(ChatMessage::estimated_bytes).sum();
        let tool_bytes: usize =
            self.tools.iter().map(|t| t.name.len() + t.description.len() + t.parameters.len()).sum();
        let total_bytes = message_bytes + tool_bytes;
        u32::try_from(total_bytes / 4).unwrap_or(u32::MAX)
    }

    /// Build an assistant turn and its tool call results and append them to messages.
    pub fn push_assistant_turn(
        &mut self,
        content: &str,
        reasoning: AssistantReasoning,
        completed_tools: Vec<Result<super::ToolCallResult, super::ToolCallError>>,
    ) {
        let tool_requests: Vec<_> = completed_tools
            .iter()
            .map(|result| match result {
                Ok(r) => {
                    super::ToolCallRequest { id: r.id.clone(), name: r.name.clone(), arguments: r.arguments.clone() }
                }
                Err(e) => super::ToolCallRequest {
                    id: e.id.clone(),
                    name: e.name.clone(),
                    arguments: e.arguments.clone().unwrap_or_default(),
                },
            })
            .collect();

        self.messages.push(ChatMessage::Assistant {
            content: content.to_string(),
            reasoning,
            timestamp: IsoString::now(),
            tool_calls: tool_requests,
        });

        for result in completed_tools {
            self.messages.push(ChatMessage::ToolCallResult(result));
        }
    }

    /// Return a copy with encrypted reasoning filtered for the given model.
    /// Encrypted content is kept only when its source model matches.
    pub fn filter_encrypted_reasoning(&self, model: &LlmModel) -> Self {
        let messages = self
            .messages
            .iter()
            .map(|msg| match msg {
                ChatMessage::Assistant { content, reasoning, timestamp, tool_calls } => ChatMessage::Assistant {
                    content: content.clone(),
                    reasoning: AssistantReasoning {
                        summary_text: reasoning.summary_text.clone(),
                        encrypted_content: reasoning
                            .encrypted_content
                            .as_ref()
                            .filter(|ec| &ec.model == model)
                            .cloned(),
                    },
                    timestamp: timestamp.clone(),
                    tool_calls: tool_calls.clone(),
                },
                other => other.clone(),
            })
            .collect();
        Context {
            messages,
            tools: self.tools.clone(),
            reasoning_effort: self.reasoning_effort,
            prompt_cache_key: self.prompt_cache_key.clone(),
        }
    }

    /// Clear all non-system messages, retaining only system prompts.
    pub fn clear_conversation(&mut self) {
        self.messages.retain(super::chat_message::ChatMessage::is_system);
    }

    /// Get all non-system messages for summarization
    pub fn messages_for_summary(&self) -> Vec<&ChatMessage> {
        self.messages.iter().filter(|msg| !msg.is_system()).collect()
    }

    /// Create a new context with all messages replaced by a summary.
    /// Preserves the system prompt and tools.
    pub fn with_compacted_summary(&self, summary: &str) -> Context {
        let system_messages: Vec<_> = self.messages.iter().filter(|msg| msg.is_system()).cloned().collect();

        let non_system_count = self.messages.len() - system_messages.len();

        let mut messages = system_messages;
        if non_system_count > 0 {
            messages.push(ChatMessage::Summary {
                content: summary.to_string(),
                timestamp: IsoString::now(),
                messages_compacted: non_system_count,
            });
        }

        Context {
            messages,
            tools: self.tools.clone(),
            reasoning_effort: self.reasoning_effort,
            prompt_cache_key: self.prompt_cache_key.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ContentBlock;
    use crate::ToolCallResult;
    use crate::catalog::LlmModel;

    fn create_test_context() -> Context {
        let messages = vec![
            ChatMessage::System { content: "You are a helpful assistant.".to_string(), timestamp: IsoString::now() },
            ChatMessage::User { content: vec![ContentBlock::text("Hello")], timestamp: IsoString::now() },
            ChatMessage::Assistant {
                content: "Hi there!".to_string(),
                reasoning: AssistantReasoning::default(),
                timestamp: IsoString::now(),
                tool_calls: vec![],
            },
            ChatMessage::ToolCallResult(Ok(ToolCallResult {
                id: "1".to_string(),
                name: "tool1".to_string(),
                arguments: "{}".to_string(),
                result: "Result 1".to_string(),
            })),
            ChatMessage::ToolCallResult(Ok(ToolCallResult {
                id: "2".to_string(),
                name: "tool2".to_string(),
                arguments: "{}".to_string(),
                result: "Result 2".to_string(),
            })),
            ChatMessage::ToolCallResult(Ok(ToolCallResult {
                id: "3".to_string(),
                name: "tool3".to_string(),
                arguments: "{}".to_string(),
                result: "Result 3".to_string(),
            })),
        ];
        Context::new(messages, vec![])
    }

    #[test]
    fn test_message_count() {
        let ctx = create_test_context();
        assert_eq!(ctx.message_count(), 6);
    }

    #[test]
    fn test_with_compacted_summary_preserves_system_prompt() {
        let ctx = create_test_context();
        let compacted = ctx.with_compacted_summary("This is a summary of previous conversation.");

        assert_eq!(compacted.message_count(), 2);
        assert!(compacted.messages()[0].is_system());
        assert!(compacted.messages()[1].is_summary());
    }

    #[test]
    fn test_with_compacted_summary_empty_context() {
        let ctx = Context::new(
            vec![ChatMessage::System { content: "System".to_string(), timestamp: IsoString::now() }],
            vec![],
        );
        let compacted = ctx.with_compacted_summary("Summary");

        assert_eq!(compacted.message_count(), 1);
    }

    #[test]
    fn test_messages_for_summary() {
        let ctx = create_test_context();
        let msgs = ctx.messages_for_summary();

        assert_eq!(msgs.len(), 5);
        assert!(msgs.iter().all(|m| !m.is_system()));
    }

    #[test]
    fn test_prompt_cache_key_default_is_none() {
        let ctx = create_test_context();
        assert_eq!(ctx.prompt_cache_key(), None);
    }

    #[test]
    fn test_prompt_cache_key_set_and_get() {
        let mut ctx = create_test_context();
        ctx.set_prompt_cache_key(Some("session-123".to_string()));
        assert_eq!(ctx.prompt_cache_key(), Some("session-123"));

        ctx.set_prompt_cache_key(None);
        assert_eq!(ctx.prompt_cache_key(), None);
    }

    #[test]
    fn test_prompt_cache_key_preserved_through_compaction() {
        let mut ctx = create_test_context();
        ctx.set_prompt_cache_key(Some("session-abc".to_string()));
        let compacted = ctx.with_compacted_summary("Summary");
        assert_eq!(compacted.prompt_cache_key(), Some("session-abc"));
    }

    #[test]
    fn test_prompt_cache_key_preserved_through_projection() {
        let model: LlmModel = "anthropic:claude-opus-4-6".parse().unwrap();
        let mut ctx = Context::new(
            vec![ChatMessage::User { content: vec![ContentBlock::text("Hello")], timestamp: IsoString::now() }],
            vec![],
        );
        ctx.set_prompt_cache_key(Some("session-xyz".to_string()));
        let projected = ctx.filter_encrypted_reasoning(&model);
        assert_eq!(projected.prompt_cache_key(), Some("session-xyz"));
    }

    #[test]
    fn test_reasoning_effort_default_is_none() {
        let ctx = create_test_context();
        assert_eq!(ctx.reasoning_effort(), None);
    }

    #[test]
    fn test_reasoning_effort_set_and_get() {
        let mut ctx = create_test_context();
        ctx.set_reasoning_effort(Some(crate::ReasoningEffort::High));
        assert_eq!(ctx.reasoning_effort(), Some(crate::ReasoningEffort::High));

        ctx.set_reasoning_effort(None);
        assert_eq!(ctx.reasoning_effort(), None);
    }

    #[test]
    fn test_reasoning_effort_preserved_through_compaction() {
        let mut ctx = create_test_context();
        ctx.set_reasoning_effort(Some(crate::ReasoningEffort::Medium));
        let compacted = ctx.with_compacted_summary("Summary");
        assert_eq!(compacted.reasoning_effort(), Some(crate::ReasoningEffort::Medium));
    }

    #[test]
    fn test_estimated_token_count() {
        use crate::ToolDefinition;

        // "You are a helpful assistant." = 28 bytes
        // "Hello" = 5 bytes
        // "Hi there!" = 9 bytes (assistant, no reasoning, no tool calls)
        // 3 tool results: "Result 1" (8) + "tool1" (5) + "{}" (2) = 15 each = 45 total
        // Total message bytes = 28 + 5 + 9 + 45 = 87
        let ctx = create_test_context();
        let base_estimate = ctx.estimated_token_count();

        // With no tools, estimate = message_bytes / 4
        assert_eq!(base_estimate, 87 / 4);

        // Now add a tool definition and verify it increases
        let tool = ToolDefinition {
            name: "read_file".to_string(),           // 9
            description: "Reads a file".to_string(), // 12
            parameters: "{}".to_string(),            // 2
            server: None,
        };
        let ctx_with_tools = Context::new(ctx.messages().clone(), vec![tool]);
        let with_tools_estimate = ctx_with_tools.estimated_token_count();
        assert_eq!(with_tools_estimate, (87 + 9 + 12 + 2) / 4);
        assert!(with_tools_estimate > base_estimate);
    }

    #[test]
    fn compaction_drops_encrypted_reasoning() {
        let model: LlmModel = "anthropic:claude-opus-4-6".parse().unwrap();
        let ctx = Context::new(
            vec![
                ChatMessage::User { content: vec![ContentBlock::text("Hello")], timestamp: IsoString::now() },
                ChatMessage::Assistant {
                    content: "I see.".to_string(),
                    reasoning: AssistantReasoning {
                        summary_text: Some("thinking".to_string()),
                        encrypted_content: Some(crate::EncryptedReasoningContent {
                            id: "r_test".to_string(),
                            model,
                            content: "blob".to_string(),
                        }),
                    },
                    timestamp: IsoString::now(),
                    tool_calls: vec![],
                },
            ],
            vec![],
        );
        let compacted = ctx.with_compacted_summary("Summary of conversation");

        for msg in compacted.messages() {
            if let ChatMessage::Assistant { reasoning, .. } = msg {
                assert!(reasoning.encrypted_content.is_none(), "compaction should drop encrypted reasoning");
            }
        }
    }

    #[test]
    fn projected_for_keeps_matching_model() {
        let model: LlmModel = "anthropic:claude-opus-4-6".parse().unwrap();
        let ctx = Context::new(
            vec![ChatMessage::Assistant {
                content: "reply".to_string(),
                reasoning: AssistantReasoning {
                    summary_text: Some("think".to_string()),
                    encrypted_content: Some(crate::EncryptedReasoningContent {
                        id: "r_test".to_string(),
                        model: model.clone(),
                        content: "blob".to_string(),
                    }),
                },
                timestamp: IsoString::now(),
                tool_calls: vec![],
            }],
            vec![],
        );
        let projected = ctx.filter_encrypted_reasoning(&model);
        if let ChatMessage::Assistant { reasoning, .. } = &projected.messages()[0] {
            assert!(reasoning.encrypted_content.is_some());
            assert_eq!(reasoning.summary_text.as_deref(), Some("think"));
        } else {
            panic!("expected assistant message");
        }
    }

    #[test]
    fn projected_for_strips_non_matching_model() {
        let model_a: LlmModel = "anthropic:claude-opus-4-6".parse().unwrap();
        let model_b: LlmModel = "anthropic:claude-sonnet-4-5-20250929".parse().unwrap();
        let ctx = Context::new(
            vec![ChatMessage::Assistant {
                content: "reply".to_string(),
                reasoning: AssistantReasoning {
                    summary_text: Some("think".to_string()),
                    encrypted_content: Some(crate::EncryptedReasoningContent {
                        id: "r_test".to_string(),
                        model: model_a,
                        content: "blob".to_string(),
                    }),
                },
                timestamp: IsoString::now(),
                tool_calls: vec![],
            }],
            vec![],
        );
        let projected = ctx.filter_encrypted_reasoning(&model_b);
        if let ChatMessage::Assistant { reasoning, .. } = &projected.messages()[0] {
            assert!(reasoning.encrypted_content.is_none());
            assert_eq!(reasoning.summary_text.as_deref(), Some("think"));
        } else {
            panic!("expected assistant message");
        }
    }
}
