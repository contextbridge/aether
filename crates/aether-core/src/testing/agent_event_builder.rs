use crate::events::{AgentEvent, StreamState, ToolEvent};
use crate::mcp::tool_bridge::encode_structured;
use llm::{ToolCallError, ToolCallRequest, ToolCallResult};
use serde::Serialize;

pub fn agent_event(message_id: &str) -> AgentEventBuilder {
    AgentEventBuilder::new(message_id)
}

pub struct AgentEventBuilder {
    message_id: String,
    chunks: Vec<AgentEvent>,
    full_text: String,
}

impl AgentEventBuilder {
    pub fn new(message_id: &str) -> Self {
        Self { message_id: message_id.to_string(), chunks: Vec::new(), full_text: String::new() }
    }

    pub fn text(mut self, chunks: &[&str]) -> Self {
        for chunk in chunks {
            self.chunks.push(AgentEvent::text(&self.message_id, chunk, StreamState::Partial));
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

        self.push_tool_call(tool_call_id, name, &request_json);

        self.chunks.push(AgentEvent::Tool(ToolEvent::Result {
            result: ToolCallResult {
                id: tool_call_id.to_string(),
                name: name.to_string(),
                arguments: request_json,
                result: encode_structured(&result_value),
            },
            result_meta: None,
        }));

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

        let error_result = format!("Tool execution error: {error_message}");

        self.push_tool_call(tool_call_id, name, &request_json);

        self.chunks.push(AgentEvent::Tool(ToolEvent::Error {
            error: ToolCallError {
                id: tool_call_id.to_string(),
                name: name.to_string(),
                arguments: Some(request_json),
                error: error_result,
            },
        }));

        self
    }

    pub fn build(mut self) -> Vec<AgentEvent> {
        self.chunks.push(AgentEvent::text(&self.message_id, &self.full_text, StreamState::Complete));

        self.chunks
    }

    fn push_tool_call(&mut self, tool_call_id: &str, name: &str, arguments: &str) {
        self.chunks.extend([
            AgentEvent::Tool(ToolEvent::InputStarted { id: tool_call_id.to_string(), name: name.to_string() }),
            AgentEvent::Tool(ToolEvent::InputDelta { id: tool_call_id.to_string(), chunk: arguments.to_string() }),
            AgentEvent::Tool(ToolEvent::Call {
                request: ToolCallRequest {
                    id: tool_call_id.to_string(),
                    name: name.to_string(),
                    arguments: arguments.to_string(),
                },
            }),
        ]);
    }
}
