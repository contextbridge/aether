use crate::types::IsoString;
use crate::{
    AssistantReasoning, ChatMessage, EncryptedReasoningContent, MessageId, ToolCallError, ToolCallRequest,
    ToolCallResult,
};

pub fn assistant_message() -> AssistantMessageBuilder {
    AssistantMessageBuilder::default()
}

pub fn tool_result(id: &str, name: &str, arguments: &str, result: &str) -> ChatMessage {
    ChatMessage::ToolCallResult(Ok(ToolCallResult {
        id: id.to_string(),
        name: name.to_string(),
        arguments: arguments.to_string(),
        result: result.to_string(),
    }))
}

pub fn tool_error(id: &str, name: &str, error: &str) -> ChatMessage {
    ChatMessage::ToolCallResult(Err(ToolCallError {
        id: id.to_string(),
        name: name.to_string(),
        arguments: None,
        error: error.to_string(),
    }))
}

#[derive(Default)]
pub struct AssistantMessageBuilder {
    content: String,
    reasoning: AssistantReasoning,
    tool_calls: Vec<ToolCallRequest>,
}

impl AssistantMessageBuilder {
    pub fn text(mut self, content: &str) -> Self {
        self.content = content.to_string();
        self
    }

    pub fn tool_call(mut self, id: &str, name: &str, arguments: &str) -> Self {
        self.tool_calls.push(ToolCallRequest {
            id: id.to_string(),
            name: name.to_string(),
            arguments: arguments.to_string(),
        });
        self
    }

    pub fn reasoning_summary(mut self, summary: &str) -> Self {
        self.reasoning.summary_text = Some(summary.to_string());
        self
    }

    pub fn encrypted_reasoning(mut self, id: &str, model: &str, content: &str) -> Self {
        self.reasoning.encrypted_content = Some(EncryptedReasoningContent {
            id: id.to_string(),
            model: model.parse().unwrap_or_else(|error| panic!("invalid model '{model}': {error}")),
            content: content.to_string(),
        });
        self
    }

    pub fn build(self) -> ChatMessage {
        ChatMessage::Assistant {
            message_id: MessageId::new(),
            content: self.content,
            reasoning: self.reasoning,
            timestamp: IsoString::now(),
            tool_calls: self.tool_calls,
        }
    }
}
