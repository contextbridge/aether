use acp_utils::notifications::ToolResultMeta;
use agent_client_protocol::schema as acp;
use std::collections::HashMap;

use crate::components::tool_call_status_view::ToolCallStatus;
use tui::DiffPreview;

#[derive(Clone)]
pub(crate) struct TrackedToolCall {
    pub(crate) name: String,
    pub(crate) arguments: String,
    pub(crate) display_value: Option<String>,
    pub(crate) diff_preview: Option<DiffPreview>,
    pub(crate) status: ToolCallStatus,
}

impl TrackedToolCall {
    pub(crate) fn new_running(name: impl Into<String>, arguments: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            arguments: arguments.into(),
            display_value: None,
            diff_preview: None,
            status: ToolCallStatus::Running,
        }
    }

    pub(crate) fn update_name(&mut self, name: &str) {
        if !name.is_empty() {
            self.name.clear();
            self.name.push_str(name);
        }
    }

    pub(crate) fn append_arguments(&mut self, fragment: &str) {
        self.arguments.push_str(fragment);
    }

    pub(crate) fn apply_result_meta(&mut self, meta: ToolResultMeta) {
        self.name.clone_from(&meta.display.title);
        self.display_value = Some(meta.display.value);
    }

    pub(crate) fn apply_status(&mut self, status: acp::ToolCallStatus) {
        match status {
            acp::ToolCallStatus::Completed => self.status = ToolCallStatus::Success,
            acp::ToolCallStatus::Failed => {
                self.status = ToolCallStatus::Error("failed".to_string());
            }
            acp::ToolCallStatus::InProgress | acp::ToolCallStatus::Pending => {
                self.status = ToolCallStatus::Running;
            }
            _ => {}
        }
    }
}

pub(crate) fn raw_input_fragment(raw_input: &serde_json::Value) -> String {
    raw_input.as_str().map_or_else(|| raw_input.to_string(), str::to_string)
}

pub(crate) fn upsert_tracked_tool_call<'a>(
    tool_order: &mut Vec<String>,
    tool_calls: &'a mut HashMap<String, TrackedToolCall>,
    id: &str,
    default_name: &str,
    default_arguments: String,
) -> &'a mut TrackedToolCall {
    if !tool_calls.contains_key(id) {
        tool_order.push(id.to_string());
    }

    tool_calls.entry(id.to_string()).or_insert_with(|| TrackedToolCall::new_running(default_name, default_arguments))
}
