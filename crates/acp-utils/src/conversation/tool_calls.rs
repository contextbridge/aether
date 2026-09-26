use crate::notifications::{SubAgentEvent, SubAgentProgressParams};
use agent_client_protocol::schema::{MaybeUndefined, v2 as acp};
use serde::Serialize;

/// A tracked tool call within a sub-agent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SubAgentToolCall {
    pub id: String,
    pub name: String,
    pub raw_input: String,
    pub display_value: Option<String>,
    pub status: ToolStatus,
    #[serde(skip)]
    kind: ToolKind,
}

impl SubAgentToolCall {
    pub fn bash_command(&self) -> Option<String> {
        bash_command(self.kind, &self.raw_input)
    }
}

/// Per-sub-agent state: tracks its tool calls in arrival order.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SubAgentState {
    pub task_id: String,
    pub agent_name: String,
    pub done: bool,
    pub tool_calls: Vec<SubAgentToolCall>,
}

impl SubAgentState {
    fn tool_call_mut(&mut self, id: &str) -> Option<&mut SubAgentToolCall> {
        self.tool_calls.iter_mut().find(|call| call.id == id)
    }

    /// The call with `id`, appending a running placeholder when it is the first
    /// event seen for it.
    fn upsert(&mut self, id: &str, name: &str, arguments: String) -> &mut SubAgentToolCall {
        let index = self.tool_calls.iter().position(|call| call.id == id).unwrap_or_else(|| {
            self.tool_calls.push(SubAgentToolCall {
                id: id.to_string(),
                name: name.to_string(),
                raw_input: arguments,
                display_value: None,
                status: ToolStatus::Running,
                kind: tool_kind(name),
            });
            self.tool_calls.len() - 1
        });
        &mut self.tool_calls[index]
    }
}

/// A tool call as the merge of every update the agent sent for it.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolCall {
    pub status: ToolStatus,
    pub sub_agents: Vec<SubAgentState>,
    #[serde(rename = "toolCall")]
    protocol: Box<acp::ToolCallUpdate>,
}

impl ToolCall {
    pub(super) fn from_update(update: &acp::ToolCallUpdate) -> Self {
        let mut tool = Self { status: ToolStatus::Running, sub_agents: Vec::new(), protocol: Box::new(update.clone()) };
        tool.refresh_status();
        tool
    }

    pub fn title(&self) -> &str {
        self.protocol.title.value().map_or("", String::as_str)
    }

    pub fn raw_input(&self) -> String {
        self.protocol.raw_input.value().map_or_else(String::new, raw_input_fragment)
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

    pub(super) fn finalize(&mut self, terminal_status: &ToolStatus) {
        if self.status == ToolStatus::Running {
            self.status = terminal_status.clone();
        }
        for agent in &mut self.sub_agents {
            agent.done = true;
            for call in &mut agent.tool_calls {
                if matches!(call.status, ToolStatus::Running) {
                    call.status = terminal_status.clone();
                }
            }
        }
    }

    pub fn bash_command(&self) -> Option<String> {
        bash_command(self.kind(), &self.raw_input())
    }

    pub(super) fn is_running(&self) -> bool {
        self.status == ToolStatus::Running
            || self.sub_agents.iter().any(|agent| {
                !agent.done || agent.tool_calls.iter().any(|call| matches!(call.status, ToolStatus::Running))
            })
    }

    /// Whether this call can enter native history: it reached a terminal
    /// status and every spawned sub-agent has finished. A background
    /// spawn completes before its agents start reporting, so an empty tree on
    /// a completed spawner means "not yet", not "none".
    pub(super) fn rendering_final(&self) -> bool {
        !self.is_running() && (self.kind() != ToolKind::SpawnSubagent || !self.sub_agents.is_empty())
    }

    fn kind(&self) -> ToolKind {
        tool_kind(self.protocol.name.value().map_or_else(|| self.title(), String::as_str))
    }

    /// Re-derives the coarse status from the merged protocol update; `Undefined`
    /// fields keep their previous value, so re-running this is idempotent.
    fn refresh_status(&mut self) {
        self.status = match self.protocol.status.value() {
            Some(acp::ToolCallStatus::Completed) => ToolStatus::Success,
            Some(acp::ToolCallStatus::Failed) => ToolStatus::Error("failed".to_string()),
            Some(acp::ToolCallStatus::Cancelled) => ToolStatus::Error("cancelled".to_string()),
            _ => ToolStatus::Running,
        };
    }

    fn meta_str(&self, key: &str) -> Option<&str> {
        self.protocol.meta.value().and_then(|meta| meta.get(key)).and_then(serde_json::Value::as_str)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolStatus {
    Running,
    Success,
    Error(String),
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
        SubAgentEvent::ToolCall { request } => {
            let call = agent.upsert(&request.id, &request.name, request.arguments.clone());
            update_title(&mut call.name, &request.name);
            call.kind = tool_kind(&request.name);
            call.raw_input.clone_from(&request.arguments);
            call.status = ToolStatus::Running;
        }
        SubAgentEvent::ToolCallUpdate { update } => {
            let call = agent.upsert(&update.id, "tool", String::new());
            call.raw_input.push_str(&update.chunk);
            call.status = ToolStatus::Running;
        }
        SubAgentEvent::ToolResult { result } => {
            if let Some(call) = agent.tool_call_mut(&result.id) {
                call.status = ToolStatus::Success;
                if let Some(result_meta) = &result.result_meta {
                    call.name.clone_from(&result_meta.display.title);
                    call.display_value = Some(result_meta.display.value.clone());
                }
            }
        }
        SubAgentEvent::ToolError { error } => {
            if let Some(call) = agent.tool_call_mut(&error.id) {
                call.status = ToolStatus::Error("failed".to_string());
            }
        }
        SubAgentEvent::Done => agent.done = true,
        SubAgentEvent::Other => {}
    }
}

fn update_title(current: &mut String, new_title: &str) {
    if !new_title.is_empty() {
        current.clear();
        current.push_str(new_title);
    }
}

fn raw_input_fragment(raw_input: &serde_json::Value) -> String {
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

fn bash_command(kind: ToolKind, raw_input: &str) -> Option<String> {
    if kind != ToolKind::Bash {
        return None;
    }
    serde_json::from_str::<serde_json::Value>(raw_input).ok()?.get("command")?.as_str().map(str::to_string)
}
