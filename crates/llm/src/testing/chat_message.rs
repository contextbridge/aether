use crate::types::IsoString;
use crate::{
    AssistantReply, ChatMessage, LlmModel, LlmResponse, MessageId, ToolCallError, ToolCallRequest, ToolCallResult,
};

pub fn assistant_message() -> AssistantMessageBuilder {
    AssistantMessageBuilder { message_id: MessageId::new(), model: None, reply: AssistantReply::default() }
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

#[derive(Clone)]
pub struct AssistantMessageBuilder {
    message_id: MessageId,
    model: Option<LlmModel>,
    reply: AssistantReply,
}

impl AssistantMessageBuilder {
    pub fn id(mut self, message_id: &str) -> Self {
        self.message_id = message_id.into();
        self
    }

    pub fn model(mut self, model: &str) -> Self {
        self.model = Some(model.parse().unwrap_or_else(|error| panic!("invalid model '{model}': {error}")));
        self
    }

    pub fn text(self, text: &str) -> Self {
        self.push(&LlmResponse::text(text))
    }

    pub fn tool_call(self, id: &str, name: &str, arguments: &str) -> Self {
        self.push(&LlmResponse::ToolRequestComplete {
            tool_call: ToolCallRequest { id: id.to_string(), name: name.to_string(), arguments: arguments.to_string() },
        })
    }

    pub fn reasoning(self, summary: &str) -> Self {
        self.push(&LlmResponse::reasoning(summary))
    }

    pub fn encrypted_reasoning(self, id: &str, content: &str) -> Self {
        self.push(&LlmResponse::encrypted_reasoning(id, content))
    }

    pub fn build(self) -> ChatMessage {
        ChatMessage::Assistant {
            message_id: self.message_id,
            model: self.model,
            content: self.reply,
            timestamp: IsoString::now(),
        }
    }

    pub fn build_reply(self) -> AssistantReply {
        self.reply
    }

    fn push(mut self, response: &LlmResponse) -> Self {
        self.reply.push(response);
        self
    }
}
