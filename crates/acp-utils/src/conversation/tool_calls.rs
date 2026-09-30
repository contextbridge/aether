use crate::notifications::{SubAgentEvent, SubAgentProgressParams};
use agent_client_protocol::schema::{MaybeUndefined, v2 as acp};
use schemars::JsonSchema;
use serde::Serialize;

/// Per-sub-agent state: tracks its tool calls in arrival order.
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct SubAgentState {
    pub task_id: String,
    pub agent_name: String,
    pub done: bool,
    pub tool_calls: Vec<ToolCall>,
}

/// A tool call as the merge of every update the agent sent for it.
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ToolCall {
    pub status: ToolStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    pub sub_agents: Vec<SubAgentState>,
    #[serde(rename = "toolCall")]
    protocol: Box<acp::ToolCallUpdate>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ToolStatus {
    Running,
    Success,
    Cancelled,
    Failed,
}

impl ToolCall {
    pub fn id(&self) -> &str {
        &self.protocol.tool_call_id.0
    }

    pub fn title(&self) -> &str {
        self.protocol.title.value().map_or("", String::as_str)
    }

    pub fn raw_input(&self) -> String {
        self.protocol.raw_input.value().map_or_else(String::new, raw_input_text)
    }

    pub fn display_value(&self) -> Option<&str> {
        self.meta_str("display_value")
    }

    pub fn content(&self) -> &[acp::ToolCallContent] {
        self.protocol.content.value().map_or(&[], Vec::as_slice)
    }

    pub fn diffs(&self) -> impl Iterator<Item = &acp::Diff> {
        self.content().iter().filter_map(|content| match content {
            acp::ToolCallContent::Diff(diff) => Some(diff),
            _ => None,
        })
    }

    pub fn bash_command(&self) -> Option<&str> {
        if self.kind() != ToolKind::Bash {
            return None;
        }
        self.protocol.raw_input.value()?.get("command")?.as_str()
    }

    pub(super) fn from_update(update: &acp::ToolCallUpdate) -> Self {
        let mut tool = Self {
            status: ToolStatus::Running,
            error: None,
            sub_agents: Vec::new(),
            protocol: Box::new(update.clone()),
        };
        tool.refresh_status();
        tool
    }

    pub(super) fn apply_update(&mut self, update: &acp::ToolCallUpdate) {
        self.protocol.apply_update(update.clone());
        self.refresh_status();
    }

    pub(super) fn append_content(&mut self, content: acp::ToolCallContent) {
        match &mut self.protocol.content {
            MaybeUndefined::Value(items) => items.push(content),
            value => *value = MaybeUndefined::Value(vec![content]),
        }
    }

    pub(super) fn apply_sub_agent_progress(&mut self, notification: &SubAgentProgressParams) {
        apply_sub_agent_progress(&mut self.sub_agents, notification);
    }

    pub(super) fn finalize(&mut self, status: ToolStatus, error: Option<&str>) {
        if self.status == ToolStatus::Running {
            self.status = status;
            self.error = error.map(str::to_owned);
        }
        for agent in &mut self.sub_agents {
            agent.done = true;
            for call in &mut agent.tool_calls {
                call.finalize(status, None);
            }
        }
    }

    pub(super) fn is_running(&self) -> bool {
        self.status == ToolStatus::Running
            || self.sub_agents.iter().any(|agent| !agent.done || agent.tool_calls.iter().any(ToolCall::is_running))
    }

    pub(super) fn rendering_final(&self) -> bool {
        !self.is_running() && (self.kind() != ToolKind::SpawnSubagent || !self.sub_agents.is_empty())
    }

    fn kind(&self) -> ToolKind {
        tool_kind(self.protocol.name.value().map_or_else(|| self.title(), String::as_str))
    }

    fn refresh_status(&mut self) {
        self.status = match self.protocol.status.value() {
            Some(acp::ToolCallStatus::Completed) => ToolStatus::Success,
            Some(acp::ToolCallStatus::Failed) => ToolStatus::Failed,
            Some(acp::ToolCallStatus::Cancelled) => ToolStatus::Cancelled,
            _ => ToolStatus::Running,
        };
        self.error = None;
    }

    fn meta_str(&self, key: &str) -> Option<&str> {
        self.protocol.meta.value().and_then(|meta| meta.get(key)).and_then(serde_json::Value::as_str)
    }
}

fn apply_sub_agent_progress(states: &mut Vec<SubAgentState>, notification: &SubAgentProgressParams) {
    let index = states.iter().position(|agent| agent.task_id == notification.task_id).unwrap_or_else(|| {
        states.push(SubAgentState {
            task_id: notification.task_id.clone(),
            agent_name: notification.agent_name.clone(),
            done: false,
            tool_calls: Vec::new(),
        });
        states.len() - 1
    });
    let agent = &mut states[index];

    match &notification.event {
        SubAgentEvent::Started => {}
        SubAgentEvent::ToolCallUpdate(update) => {
            match agent.tool_calls.iter_mut().find(|call| call.protocol.tool_call_id == update.tool_call_id) {
                Some(call) => call.apply_update(update),
                None => agent.tool_calls.push(ToolCall::from_update(update)),
            }
        }
        SubAgentEvent::Done => agent.done = true,
    }
}

fn raw_input_text(raw_input: &serde_json::Value) -> String {
    raw_input.as_str().map_or_else(|| raw_input.to_string(), str::to_string)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ToolKind {
    Bash,
    SpawnSubagent,
    Other,
}

fn tool_kind(tool_name: &str) -> ToolKind {
    let name = tool_name.rsplit("__").next().unwrap_or(tool_name);
    if name.eq_ignore_ascii_case("bash") {
        ToolKind::Bash
    } else if name.eq_ignore_ascii_case("spawn_subagent") {
        ToolKind::SpawnSubagent
    } else {
        ToolKind::Other
    }
}
