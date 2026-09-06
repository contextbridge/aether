use crate::git_review::FileDiff;
use acp_utils::AETHER_TOOL_NAME_META_KEY;
use acp_utils::notifications::{SubAgentEvent, SubAgentProgressParams};
use agent_client_protocol::schema::v1 as acp;

pub const SUB_AGENT_VISIBLE_TOOL_LIMIT: usize = 3;

/// A tracked tool call within a sub-agent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubAgentToolCall {
    pub id: String,
    pub name: String,
    pub arguments: String,
    pub raw_input: String,
    pub display_value: Option<String>,
    pub status: ToolStatus,
    kind: ToolKind,
}

impl SubAgentToolCall {
    pub fn bash_command(&self) -> Option<String> {
        bash_command(self.kind, &self.raw_input)
    }
}

/// Per-sub-agent state: tracks its tool calls in arrival order.
#[derive(Debug, Clone, PartialEq, Eq)]
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
                raw_input: arguments.clone(),
                arguments,
                display_value: None,
                status: ToolStatus::Running,
                kind: tool_kind(name),
            });
            self.tool_calls.len() - 1
        });
        &mut self.tool_calls[index]
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
        SubAgentEvent::ToolCall { request } => {
            let call = agent.upsert(&request.id, &request.name, request.arguments.clone());
            update_title(&mut call.name, &request.name);
            call.kind = tool_kind(&request.name);
            call.arguments.clone_from(&request.arguments);
            call.raw_input.clone_from(&request.arguments);
            call.status = ToolStatus::Running;
        }
        SubAgentEvent::ToolCallUpdate { update } => {
            let call = agent.upsert(&update.id, "tool", String::new());
            call.arguments.push_str(&update.chunk);
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolCall {
    pub id: String,
    pub title: String,
    pub status: ToolStatus,
    pub diff: Option<Box<FileDiff>>,
    pub raw_input: String,
    pub display_value: Option<String>,
    pub sub_agents: Vec<SubAgentState>,
    kind: ToolKind,
}

impl ToolCall {
    pub(crate) fn from_acp(tool_call: &acp::ToolCall) -> Self {
        let tool_name = tool_call
            .meta
            .as_ref()
            .and_then(|meta| meta.get(AETHER_TOOL_NAME_META_KEY))
            .and_then(serde_json::Value::as_str)
            .unwrap_or(&tool_call.title);
        let raw_input = tool_call.raw_input.as_ref().map_or_else(String::new, raw_input_fragment);
        Self {
            id: tool_call.tool_call_id.0.to_string(),
            title: tool_call.title.clone(),
            status: ToolStatus::Running,
            diff: None,
            raw_input,
            display_value: None,
            sub_agents: Vec::new(),
            kind: tool_kind(tool_name),
        }
    }

    pub(crate) fn apply_update(&mut self, update: &acp::ToolCallUpdate) {
        if let Some(title) = &update.fields.title {
            update_title(&mut self.title, title);
        }
        if let Some(raw_input) = &update.fields.raw_input {
            self.raw_input.push_str(&raw_input_fragment(raw_input));
        }
        if let Some(meta) = &update.meta
            && let Some(serde_json::Value::String(display_value)) = meta.get("display_value")
        {
            self.display_value = Some(display_value.clone());
        }
        if let Some(content) = &update.fields.content {
            for item in content {
                if let acp::ToolCallContent::Diff(diff) = item {
                    self.diff = Some(Box::new(FileDiff::from_texts(
                        diff.path.display().to_string(),
                        diff.old_text.as_deref().unwrap_or_default(),
                        &diff.new_text,
                    )));
                }
            }
        }
        if let Some(status) = update.fields.status {
            match status {
                acp::ToolCallStatus::Completed => self.status = ToolStatus::Success,
                acp::ToolCallStatus::Failed => self.status = ToolStatus::Error("failed".to_string()),
                acp::ToolCallStatus::InProgress | acp::ToolCallStatus::Pending => self.status = ToolStatus::Running,
                _ => {}
            }
        }
    }

    pub(crate) fn apply_sub_agent_progress(&mut self, notification: &SubAgentProgressParams) {
        apply_sub_agent_progress(&mut self.sub_agents, notification);
    }

    pub(crate) fn finalize(&mut self, terminal_status: &ToolStatus) {
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
        bash_command(self.kind, &self.raw_input)
    }

    pub(crate) fn is_running(&self) -> bool {
        self.status == ToolStatus::Running
            || self.sub_agents.iter().any(|agent| {
                !agent.done || agent.tool_calls.iter().any(|call| matches!(call.status, ToolStatus::Running))
            })
    }

    /// Whether this call's rendering can no longer change: it reached a
    /// terminal status and every spawned sub-agent has finished. A background
    /// spawn completes before its agents start reporting, so an empty tree on
    /// a completed spawner means "not yet", not "none".
    pub(crate) fn rendering_final(&self) -> bool {
        !self.is_running() && (self.kind != ToolKind::SpawnSubagent || !self.sub_agents.is_empty())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolStatus {
    Running,
    Success,
    Error(String),
}

fn update_title(current: &mut String, new_title: &str) {
    if !new_title.is_empty() {
        current.clear();
        current.push_str(new_title);
    }
}

pub(crate) fn raw_input_fragment(raw_input: &serde_json::Value) -> String {
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
