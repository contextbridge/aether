use acp_utils::AETHER_TOOL_NAME_META_KEY;
use acp_utils::notifications::{SubAgentEvent, SubAgentProgressParams};
use agent_client_protocol::schema::{MaybeUndefined, v2 as acp};

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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolDiff {
    pub changes: Vec<String>,
    pub patch: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ToolCall {
    pub id: String,
    pub title: String,
    pub status: ToolStatus,
    pub diffs: Vec<ToolDiff>,
    protocol: Box<acp::ToolCallUpdate>,
    pub raw_input: String,
    pub display_value: Option<String>,
    pub sub_agents: Vec<SubAgentState>,
    kind: ToolKind,
}

impl ToolCall {
    pub fn from_update(update: &acp::ToolCallUpdate) -> Self {
        let mut tool = Self {
            id: update.tool_call_id.to_string(),
            title: String::new(),
            status: ToolStatus::Running,
            diffs: Vec::new(),
            protocol: Box::new(acp::ToolCallUpdate::new(update.tool_call_id.clone())),
            raw_input: String::new(),
            display_value: None,
            sub_agents: Vec::new(),
            kind: ToolKind::Other,
        };
        tool.apply_update(update);
        tool
    }

    pub fn content(&self) -> &[acp::ToolCallContent] {
        self.protocol.content.value().map_or(&[], Vec::as_slice)
    }

    pub fn apply_update(&mut self, update: &acp::ToolCallUpdate) {
        self.protocol.apply_update(update.clone());
        self.title = self.protocol.title.value().cloned().unwrap_or_default();
        self.raw_input = self.protocol.raw_input.value().map_or_else(String::new, raw_input_fragment);
        let meta = self.protocol.meta.value();
        self.display_value =
            meta.and_then(|meta| meta.get("display_value")).and_then(serde_json::Value::as_str).map(str::to_string);
        let name = meta
            .and_then(|meta| meta.get(AETHER_TOOL_NAME_META_KEY))
            .and_then(serde_json::Value::as_str)
            .unwrap_or(&self.title);
        self.kind = tool_kind(name);
        if !update.status.is_undefined() {
            self.status = match self.protocol.status.value() {
                Some(acp::ToolCallStatus::Completed) => ToolStatus::Success,
                Some(acp::ToolCallStatus::Failed) => ToolStatus::Error("failed".to_string()),
                _ => ToolStatus::Running,
            };
        }
        if !update.content.is_undefined() {
            self.refresh_diffs();
        }
    }

    pub fn append_content(&mut self, content: acp::ToolCallContent) {
        match &mut self.protocol.content {
            MaybeUndefined::Value(items) => items.push(content),
            value => *value = MaybeUndefined::Value(vec![content]),
        }
        self.refresh_diffs();
    }

    fn refresh_diffs(&mut self) {
        self.diffs = self
            .content()
            .iter()
            .filter_map(|content| {
                let acp::ToolCallContent::Diff(diff) = content else {
                    return None;
                };
                Some(ToolDiff {
                    changes: diff.changes.iter().map(diff_change_label).collect(),
                    patch: diff.patch.as_ref().map(|patch| patch.text.clone()),
                })
            })
            .collect();
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

fn diff_change_label(change: &acp::DiffChange) -> String {
    match &change.operation {
        acp::DiffChangeOperation::Add(change) => format!("A {}", change.path.0.display()),
        acp::DiffChangeOperation::Delete(change) => format!("D {}", change.path.0.display()),
        acp::DiffChangeOperation::Modify(change) => format!("M {}", change.path.0.display()),
        acp::DiffChangeOperation::Move(change) => {
            format!("R {} → {}", change.old_path.0.display(), change.path.0.display())
        }
        acp::DiffChangeOperation::Copy(change) => {
            format!("C {} → {}", change.old_path.0.display(), change.path.0.display())
        }
        _ => "Unknown file change".to_string(),
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
