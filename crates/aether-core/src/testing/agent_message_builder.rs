use crate::events::AgentMessage;
use llm::{ToolCallError, ToolCallRequest, ToolCallResult};
use serde::Serialize;

pub fn agent_message(message_id: &str) -> AgentMessageBuilder {
    AgentMessageBuilder::new(message_id)
}

pub struct AgentMessageBuilder {
    message_id: String,
    model_name: String,
    chunks: Vec<AgentMessage>,
    full_text: String,
}

impl AgentMessageBuilder {
    pub fn new(message_id: &str) -> Self {
        Self {
            message_id: message_id.to_string(),
            model_name: "Fake LLM".to_string(),
            chunks: Vec::new(),
            full_text: String::new(),
        }
    }

    pub fn text(mut self, chunks: &[&str]) -> Self {
        for chunk in chunks {
            self.chunks.push(AgentMessage::text(&self.message_id, chunk, false, &self.model_name));
            self.full_text.push_str(chunk);
        }
        self
    }

    pub fn tool_call<T: Serialize, U: Serialize>(
        mut self,
        tool_call_id: &str,
        name: &str,
        request: &T,
        result: &U,
    ) -> Self {
        let request_json = serde_json::to_string(request).expect("Failed to serialize request");
        let result_value = serde_json::to_value(result).expect("Failed to serialize result");
        let result_yaml = serde_yml::to_string(&result_value).unwrap_or_else(|_| result_value.to_string());

        self.push_tool_call_start(tool_call_id, name);
        self.push_tool_call_chunk(tool_call_id, &request_json);

        self.chunks.push(AgentMessage::ToolResult {
            result: ToolCallResult {
                id: tool_call_id.to_string(),
                name: name.to_string(),
                arguments: request_json,
                result: result_yaml,
            },
            result_meta: None,
            model_name: self.model_name.clone(),
        });

        self
    }

    pub fn tool_call_with_error<T: Serialize>(
        mut self,
        tool_call_id: &str,
        name: &str,
        request: &T,
        error_message: &str,
    ) -> Self {
        let request_json = serde_json::to_string(request).expect("Failed to serialize request");

        let error_result = format!(
            "Tool execution error: Annotated {{ raw: Text(RawTextContent {{ text: \"{error_message}\", meta: None }}), annotations: None }}"
        );

        self.push_tool_call_start(tool_call_id, name);
        self.push_tool_call_chunk(tool_call_id, &request_json);

        self.chunks.push(AgentMessage::ToolError {
            error: ToolCallError {
                id: tool_call_id.to_string(),
                name: name.to_string(),
                arguments: Some(request_json),
                error: error_result,
            },
            model_name: self.model_name.clone(),
        });

        self
    }

    pub fn build(mut self) -> Vec<AgentMessage> {
        self.chunks.push(AgentMessage::text(&self.message_id, &self.full_text, true, &self.model_name));

        self.chunks
    }

    fn push_tool_call_start(&mut self, tool_call_id: &str, name: &str) {
        self.chunks.push(AgentMessage::ToolCall {
            request: ToolCallRequest { id: tool_call_id.to_string(), name: name.to_string(), arguments: String::new() },
            model_name: self.model_name.clone(),
        });
    }

    fn push_tool_call_chunk(&mut self, tool_call_id: &str, chunk: &str) {
        self.chunks.push(AgentMessage::ToolCallUpdate {
            tool_call_id: tool_call_id.to_string(),
            chunk: chunk.to_string(),
            model_name: self.model_name.clone(),
        });
    }
}
