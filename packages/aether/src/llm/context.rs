use serde::{Deserialize, Serialize};

use crate::types::IsoString;

use super::{ChatMessage, ToolDefinition};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Context {
    messages: Vec<ChatMessage>,
    tools: Vec<ToolDefinition>,
}

impl Context {
    pub fn new(messages: Vec<ChatMessage>, tools: Vec<ToolDefinition>) -> Self {
        Self { messages, tools }
    }

    pub fn add_message(&mut self, message: ChatMessage) {
        self.messages.push(message);
    }

    pub fn set_tools(&mut self, tools: Vec<ToolDefinition>) {
        self.tools = tools;
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

    /// Retain only messages that match the predicate
    pub fn retain_messages<F>(&mut self, predicate: F)
    where
        F: FnMut(&ChatMessage) -> bool,
    {
        self.messages.retain(predicate);
    }

    /// Clear tool call results from the context, keeping only the most recent ones.
    /// This is a light-touch compaction strategy that reduces context size
    /// while preserving the structure of the conversation.
    ///
    /// Returns the number of tool results that were removed.
    pub fn clear_old_tool_results(&mut self, keep_recent: usize) -> usize {
        // Find indices of all tool results
        let tool_result_indices: Vec<usize> = self
            .messages
            .iter()
            .enumerate()
            .filter(|(_, msg)| msg.is_tool_result())
            .map(|(idx, _)| idx)
            .collect();

        let total_results = tool_result_indices.len();
        if total_results <= keep_recent {
            return 0;
        }

        // Determine which indices to remove (older ones)
        let remove_count = total_results - keep_recent;
        let indices_to_remove: std::collections::HashSet<usize> =
            tool_result_indices.into_iter().take(remove_count).collect();

        // Remove those messages
        let mut idx = 0;
        self.messages.retain(|_| {
            let keep = !indices_to_remove.contains(&idx);
            idx += 1;
            keep
        });

        remove_count
    }

    /// Replace all messages (except system prompt) with a summary message.
    /// This is a full compaction that significantly reduces context size.
    ///
    /// Returns the number of messages that were compacted.
    pub fn compact(&mut self, summary: &str) -> usize {
        // Separate system prompt from other messages
        let (system_messages, other_messages): (Vec<_>, Vec<_>) =
            self.messages.drain(..).partition(|msg| msg.is_system());

        let compacted_count = other_messages.len();

        // Restore system messages
        self.messages = system_messages;

        // Add the summary message
        if compacted_count > 0 {
            self.messages.push(ChatMessage::Summary {
                content: summary.to_string(),
                timestamp: IsoString::now(),
                messages_compacted: compacted_count,
            });
        }

        compacted_count
    }

    /// Get all non-system messages for summarization
    pub fn messages_for_summary(&self) -> Vec<&ChatMessage> {
        self.messages
            .iter()
            .filter(|msg| !msg.is_system())
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::ToolCallResult;

    fn create_test_context() -> Context {
        let messages = vec![
            ChatMessage::System {
                content: "You are a helpful assistant.".to_string(),
                timestamp: IsoString::now(),
            },
            ChatMessage::User {
                content: "Hello".to_string(),
                timestamp: IsoString::now(),
            },
            ChatMessage::Assistant {
                content: "Hi there!".to_string(),
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
    fn test_clear_old_tool_results_keeps_recent() {
        let mut ctx = create_test_context();
        let removed = ctx.clear_old_tool_results(2);

        assert_eq!(removed, 1);
        assert_eq!(ctx.message_count(), 5);

        // Should have kept the last 2 tool results
        let tool_results: Vec<_> = ctx.messages().iter().filter(|m| m.is_tool_result()).collect();
        assert_eq!(tool_results.len(), 2);
    }

    #[test]
    fn test_clear_old_tool_results_nothing_to_remove() {
        let mut ctx = create_test_context();
        let removed = ctx.clear_old_tool_results(10);

        assert_eq!(removed, 0);
        assert_eq!(ctx.message_count(), 6);
    }

    #[test]
    fn test_compact_preserves_system_prompt() {
        let mut ctx = create_test_context();
        let compacted = ctx.compact("This is a summary of previous conversation.");

        assert_eq!(compacted, 5); // Everything except system prompt
        assert_eq!(ctx.message_count(), 2); // System + Summary

        // Check system prompt is preserved
        assert!(ctx.messages()[0].is_system());
        assert!(ctx.messages()[1].is_summary());
    }

    #[test]
    fn test_compact_empty_context() {
        let mut ctx = Context::new(
            vec![ChatMessage::System {
                content: "System".to_string(),
                timestamp: IsoString::now(),
            }],
            vec![],
        );
        let compacted = ctx.compact("Summary");

        assert_eq!(compacted, 0);
        assert_eq!(ctx.message_count(), 1); // Only system prompt
    }

    #[test]
    fn test_messages_for_summary() {
        let ctx = create_test_context();
        let msgs = ctx.messages_for_summary();

        assert_eq!(msgs.len(), 5); // All except system prompt
        assert!(msgs.iter().all(|m| !m.is_system()));
    }

    #[test]
    fn test_retain_messages() {
        let mut ctx = create_test_context();
        ctx.retain_messages(|msg| !msg.is_tool_result());

        assert_eq!(ctx.message_count(), 3); // System, User, Assistant
    }
}
