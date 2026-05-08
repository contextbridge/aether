use acp_utils::notifications::SubAgentProgressParams;
use agent_client_protocol::schema as acp;
use serde_json::{Value, json};
use wisp::components::tool_call_status_view::{ToolCallStatus, ToolCallStatusView};

pub struct ToolCallFactory {
    id: String,
    name: String,
    raw_input: Option<Value>,
}

pub enum SubAgentEvent {
    ToolCall { id: String, name: String, arguments: String },
    ToolCallUpdate { id: String, chunk: String },
    ToolResult { id: String, name: String, result: String, result_meta: Option<Value> },
    ToolError { id: String, name: String, error: String },
    Done,
}

impl ToolCallFactory {
    pub fn id(mut self, id: &str) -> Self {
        self.id = id.to_string();
        self
    }

    pub fn name(mut self, name: &str) -> Self {
        self.name = name.to_string();
        self
    }

    pub fn raw_input_json(mut self, raw_input: &str) -> Self {
        self.raw_input = Some(serde_json::from_str(raw_input).expect("valid tool call raw input fixture"));
        self
    }

    pub fn build(self) -> acp::ToolCall {
        let mut tool_call = acp::ToolCall::new(self.id, self.name);
        if let Some(raw_input) = self.raw_input {
            tool_call = tool_call.raw_input(raw_input);
        }
        tool_call
    }
}

impl Default for ToolCallFactory {
    fn default() -> Self {
        Self { id: "tool-1".to_string(), name: "Read".to_string(), raw_input: None }
    }
}

impl SubAgentEvent {
    fn into_json_value(self) -> Value {
        match self {
            Self::ToolCall { id, name, arguments } => json!({
                "ToolCall": {
                    "request": { "id": id, "name": name, "arguments": arguments },
                    "model_name": "m"
                }
            }),
            Self::ToolCallUpdate { id, chunk } => json!({
                "ToolCallUpdate": { "update": { "id": id, "chunk": chunk } }
            }),
            Self::ToolResult { id, name, result, result_meta } => {
                let mut result_value = json!({ "id": id, "name": name, "arguments": "{}", "result": result });
                if let Some(meta) = result_meta {
                    result_value["result_meta"] = meta;
                }
                json!({ "ToolResult": { "result": result_value, "model_name": "m" } })
            }
            Self::ToolError { id, name, error } => json!({
                "ToolError": {
                    "error": { "id": id, "name": name, "arguments": "{}", "error": error },
                    "model_name": "m"
                }
            }),
            Self::Done => json!("Done"),
        }
    }
}

pub fn tool_call_update(id: &str, status: acp::ToolCallStatus) -> acp::ToolCallUpdate {
    acp::ToolCallUpdate::new(id.to_string(), acp::ToolCallUpdateFields::new().status(status))
}

pub fn completed_tool_call_update(id: &str) -> acp::ToolCallUpdate {
    tool_call_update(id, acp::ToolCallStatus::Completed)
}

pub fn failed_tool_call_update(id: &str) -> acp::ToolCallUpdate {
    tool_call_update(id, acp::ToolCallStatus::Failed)
}

pub fn tool_call_status_view(status: &ToolCallStatus) -> ToolCallStatusView<'_> {
    ToolCallStatusView { name: "TestTool", arguments: "", display_value: None, diff_preview: None, status, tick: 0 }
}

pub fn sub_agent_tool_call(id: &str, name: &str) -> SubAgentEvent {
    SubAgentEvent::ToolCall { id: id.to_string(), name: name.to_string(), arguments: "{}".to_string() }
}

pub fn sub_agent_tool_call_with_args(id: &str, name: &str, arguments: impl Into<Value>) -> SubAgentEvent {
    SubAgentEvent::ToolCall { id: id.to_string(), name: name.to_string(), arguments: arguments.into().to_string() }
}

pub fn sub_agent_tool_update(id: &str, chunk: impl Into<Value>) -> SubAgentEvent {
    SubAgentEvent::ToolCallUpdate { id: id.to_string(), chunk: chunk.into().to_string() }
}

pub fn sub_agent_tool_result(id: &str, name: &str) -> SubAgentEvent {
    SubAgentEvent::ToolResult {
        id: id.to_string(),
        name: name.to_string(),
        result: "ok".to_string(),
        result_meta: None,
    }
}

pub fn sub_agent_tool_result_with_display_meta(id: &str, name: &str, title: &str, value: &str) -> SubAgentEvent {
    SubAgentEvent::ToolResult {
        id: id.to_string(),
        name: name.to_string(),
        result: "ok".to_string(),
        result_meta: Some(json!({ "display": { "title": title, "value": value } })),
    }
}

pub fn sub_agent_tool_error(id: &str, name: &str) -> SubAgentEvent {
    SubAgentEvent::ToolError { id: id.to_string(), name: name.to_string(), error: "not found".to_string() }
}

pub fn sub_agent_done() -> SubAgentEvent {
    SubAgentEvent::Done
}

pub fn sub_agent_progress(parent_tool_id: &str, agent_name: &str, event: SubAgentEvent) -> SubAgentProgressParams {
    sub_agent_progress_with_task_id(parent_tool_id, agent_name, agent_name, event)
}

pub fn sub_agent_progress_with_task_id(
    parent_tool_id: &str,
    task_id: &str,
    agent_name: &str,
    event: SubAgentEvent,
) -> SubAgentProgressParams {
    serde_json::from_value(json!({
        "parent_tool_id": parent_tool_id,
        "task_id": task_id,
        "agent_name": agent_name,
        "event": event.into_json_value(),
    }))
    .expect("valid sub-agent progress fixture")
}
