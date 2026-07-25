use crate::diff::DiffPreview;
use acp_utils::notifications::{SubAgentEvent, SubAgentProgressParams, ToolResultMeta};
use agent_client_protocol::schema as acp;
use std::collections::HashMap;
use std::path::Path;

/// Minimal tool-call tracker: id, display title, and lifecycle status.
#[derive(Debug, Default)]
pub struct ToolCallLog {
    entries: Vec<ToolCallEntry>,
    sub_agents: SubAgentTracker,
}

pub const SUB_AGENT_VISIBLE_TOOL_LIMIT: usize = 3;

/// A tracked tool call within a sub-agent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubAgentToolCall {
    pub name: String,
    pub arguments: String,
    pub raw_input: String,
    pub display_value: Option<String>,
    pub status: ToolStatus,
}

impl SubAgentToolCall {
    fn new_running(name: &str, arguments: String) -> Self {
        Self {
            name: name.to_string(),
            raw_input: arguments.clone(),
            arguments,
            display_value: None,
            status: ToolStatus::Running,
        }
    }

    fn update_name(&mut self, name: &str) {
        if !name.is_empty() {
            self.name.clear();
            self.name.push_str(name);
        }
    }

    fn append_arguments(&mut self, fragment: &str) {
        self.arguments.push_str(fragment);
        self.raw_input.push_str(fragment);
    }

    fn apply_result_meta(&mut self, meta: &ToolResultMeta) {
        self.name.clone_from(&meta.display.title);
        self.display_value = Some(meta.display.value.clone());
    }
}

/// Per-sub-agent state: tracks its tool calls in order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubAgentState {
    pub task_id: String,
    pub agent_name: String,
    pub done: bool,
    pub tool_order: Vec<String>,
    pub tool_calls: HashMap<String, SubAgentToolCall>,
}

impl SubAgentState {
    fn is_active_for_render(&self) -> bool {
        !self.done || self.tool_calls.values().any(|tc| matches!(tc.status, ToolStatus::Running))
    }
}

/// Manages sub-agent state for tool calls that spawn child agents.
///
/// Keyed by parent tool call ID; each parent can have multiple sub-agents
/// tracked in insertion order.
#[derive(Debug, Clone, Default)]
struct SubAgentTracker {
    agents: HashMap<String, Vec<SubAgentState>>,
}

impl SubAgentTracker {
    fn on_progress(&mut self, notification: &SubAgentProgressParams) {
        let agents = self.agents.entry(notification.parent_tool_id.clone()).or_default();

        let agent = if let Some(a) = agents.iter_mut().find(|a| a.task_id == notification.task_id) {
            a
        } else {
            agents.push(SubAgentState {
                task_id: notification.task_id.clone(),
                agent_name: notification.agent_name.clone(),
                done: false,
                tool_order: Vec::new(),
                tool_calls: HashMap::new(),
            });
            agents.last_mut().unwrap()
        };

        match &notification.event {
            SubAgentEvent::ToolCall { request } => {
                let tc = upsert_sub_agent_tool_call(
                    &mut agent.tool_order,
                    &mut agent.tool_calls,
                    &request.id,
                    &request.name,
                    request.arguments.clone(),
                );
                tc.update_name(&request.name);
                tc.arguments.clone_from(&request.arguments);
                tc.raw_input.clone_from(&request.arguments);
                tc.status = ToolStatus::Running;
            }
            SubAgentEvent::ToolCallUpdate { update } => {
                let tc = upsert_sub_agent_tool_call(
                    &mut agent.tool_order,
                    &mut agent.tool_calls,
                    &update.id,
                    "tool",
                    String::new(),
                );
                tc.append_arguments(&update.chunk);
                tc.status = ToolStatus::Running;
            }
            SubAgentEvent::ToolResult { result } => {
                if let Some(tc) = agent.tool_calls.get_mut(&result.id) {
                    tc.status = ToolStatus::Success;
                    if let Some(result_meta) = &result.result_meta {
                        tc.apply_result_meta(result_meta);
                    }
                }
            }
            SubAgentEvent::ToolError { error } => {
                if let Some(tc) = agent.tool_calls.get_mut(&error.id) {
                    tc.status = ToolStatus::Error("failed".to_string());
                }
            }
            SubAgentEvent::Done => {
                agent.done = true;
            }
            SubAgentEvent::Other => {}
        }
    }

    fn get(&self, tool_id: &str) -> Option<&[SubAgentState]> {
        self.agents.get(tool_id).map(std::vec::Vec::as_slice)
    }

    fn any_running(&self) -> bool {
        self.agents.values().any(|agents| agents.iter().any(SubAgentState::is_active_for_render))
    }

    fn has_any_running_for_parent(&self, tool_id: &str) -> bool {
        self.agents.get(tool_id).is_some_and(|agents| agents.iter().any(SubAgentState::is_active_for_render))
    }

    fn finalize_running(&mut self, terminal_status: &ToolStatus) {
        for agents in self.agents.values_mut() {
            for agent in agents {
                agent.done = true;
                for tool_call in agent.tool_calls.values_mut() {
                    if matches!(tool_call.status, ToolStatus::Running) {
                        tool_call.status = terminal_status.clone();
                    }
                }
            }
        }
    }

    fn remove(&mut self, id: &str) {
        self.agents.remove(id);
    }

    fn clear(&mut self) {
        self.agents.clear();
    }
}

fn upsert_sub_agent_tool_call<'a>(
    tool_order: &mut Vec<String>,
    tool_calls: &'a mut HashMap<String, SubAgentToolCall>,
    id: &str,
    default_name: &str,
    default_arguments: String,
) -> &'a mut SubAgentToolCall {
    if !tool_calls.contains_key(id) {
        tool_order.push(id.to_string());
    }
    tool_calls.entry(id.to_string()).or_insert_with(|| SubAgentToolCall::new_running(default_name, default_arguments))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolCallEntry {
    pub id: String,
    pub title: String,
    pub status: ToolStatus,
    pub diff: Option<DiffPreview>,
    pub raw_input: String,
    pub display_value: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolStatus {
    Running,
    Success,
    Error(String),
}

impl ToolCallLog {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn on_tool_call(&mut self, tool_call: &acp::ToolCall) {
        let id = tool_call.tool_call_id.0.to_string();

        if let Some(entry) = self.entry_mut(&id) {
            update_title(&mut entry.title, &tool_call.title);
            entry.status = ToolStatus::Running;
            entry.raw_input = tool_call.raw_input.as_ref().map_or(String::new(), raw_input_fragment);
        } else {
            self.entries.push(ToolCallEntry {
                id,
                title: tool_call.title.clone(),
                status: ToolStatus::Running,
                diff: None,
                raw_input: tool_call.raw_input.as_ref().map_or(String::new(), raw_input_fragment),
                display_value: None,
            });
        }
    }

    pub fn on_tool_call_update(&mut self, update: &acp::ToolCallUpdate) {
        let Some(entry) = self.entry_mut(&update.tool_call_id.0) else {
            return;
        };

        if let Some(title) = &update.fields.title {
            update_title(&mut entry.title, title);
        }
        if let Some(raw_input) = &update.fields.raw_input {
            entry.raw_input.push_str(&raw_input_fragment(raw_input));
        }
        if let Some(meta) = &update.meta
            && let Some(serde_json::Value::String(display_value)) = meta.get("display_value")
        {
            entry.display_value = Some(display_value.clone());
        }
        if let Some(content) = &update.fields.content {
            for item in content {
                if let acp::ToolCallContent::Diff(diff) = item {
                    let language =
                        Path::new(&diff.path).extension().and_then(|extension| extension.to_str()).unwrap_or_default();
                    entry.diff = Some(DiffPreview::compute(
                        diff.old_text.as_deref().unwrap_or_default(),
                        &diff.new_text,
                        language,
                    ));
                }
            }
        }
        if let Some(status) = update.fields.status {
            match status {
                acp::ToolCallStatus::Completed => entry.status = ToolStatus::Success,
                acp::ToolCallStatus::Failed => entry.status = ToolStatus::Error("failed".to_string()),
                acp::ToolCallStatus::InProgress | acp::ToolCallStatus::Pending => entry.status = ToolStatus::Running,
                _ => {}
            }
        }
    }

    /// Mark every still-running tool call with the given terminal status.
    /// Called when the prompt turn ends (done, cancelled, or failed).
    pub fn finalize_running(&mut self, terminal_status: &ToolStatus) {
        for entry in &mut self.entries {
            if entry.status == ToolStatus::Running {
                entry.status = terminal_status.clone();
            }
        }
        self.sub_agents.finalize_running(terminal_status);
    }

    pub fn has_tool(&self, id: &str) -> bool {
        self.entries.iter().any(|entry| entry.id == id)
    }

    pub fn is_running(&self, id: &str) -> bool {
        self.entry(id).is_some_and(|entry| entry.status == ToolStatus::Running)
            || self.sub_agents.has_any_running_for_parent(id)
    }

    pub fn any_running(&self) -> bool {
        self.entries.iter().any(|entry| entry.status == ToolStatus::Running) || self.sub_agents.any_running()
    }

    pub fn entry(&self, id: &str) -> Option<&ToolCallEntry> {
        self.entries.iter().find(|entry| entry.id == id)
    }

    pub fn remove(&mut self, id: &str) -> Option<ToolCallEntry> {
        let index = self.entries.iter().position(|entry| entry.id == id)?;
        self.sub_agents.remove(id);
        Some(self.entries.remove(index))
    }

    pub fn clear(&mut self) {
        self.entries.clear();
        self.sub_agents.clear();
    }

    /// Handle a sub-agent progress notification.
    pub fn on_sub_agent_progress(&mut self, notification: &SubAgentProgressParams) {
        self.sub_agents.on_progress(notification);
    }

    /// Returns the sub-agent states for a parent tool call, if any.
    pub fn sub_agent_states(&self, tool_id: &str) -> Option<&[SubAgentState]> {
        self.sub_agents.get(tool_id)
    }

    fn entry_mut(&mut self, id: &str) -> Option<&mut ToolCallEntry> {
        self.entries.iter_mut().find(|entry| entry.id == id)
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

#[cfg(test)]
mod tests {
    use super::*;
    use acp_utils::notifications::{
        SubAgentEvent, SubAgentProgressParams, SubAgentToolCallUpdate, SubAgentToolError, SubAgentToolRequest,
        SubAgentToolResult, ToolDisplayMeta, ToolResultMeta,
    };

    fn sub_agent_event(
        parent_tool_id: &str,
        task_id: &str,
        agent_name: &str,
        event: SubAgentEvent,
    ) -> SubAgentProgressParams {
        SubAgentProgressParams {
            parent_tool_id: parent_tool_id.to_string(),
            task_id: task_id.to_string(),
            agent_name: agent_name.to_string(),
            event,
        }
    }

    fn status_update(id: &str, status: acp::ToolCallStatus) -> acp::ToolCallUpdate {
        acp::ToolCallUpdate::new(id.to_string(), acp::ToolCallUpdateFields::new().status(status))
    }

    #[test]
    fn tool_call_registers_as_running() {
        let mut log = ToolCallLog::new();
        log.on_tool_call(&acp::ToolCall::new("tool-1".to_string(), "Read file"));

        assert!(log.is_running("tool-1"));
        assert_eq!(log.entry("tool-1").unwrap().title, "Read file");
    }

    #[test]
    fn completed_update_marks_success() {
        let mut log = ToolCallLog::new();
        log.on_tool_call(&acp::ToolCall::new("tool-1".to_string(), "Read file"));
        log.on_tool_call_update(&status_update("tool-1", acp::ToolCallStatus::Completed));

        assert_eq!(log.entry("tool-1").unwrap().status, ToolStatus::Success);
        assert!(!log.any_running());
    }

    #[test]
    fn failed_update_marks_error() {
        let mut log = ToolCallLog::new();
        log.on_tool_call(&acp::ToolCall::new("tool-1".to_string(), "Read file"));
        log.on_tool_call_update(&status_update("tool-1", acp::ToolCallStatus::Failed));

        assert_eq!(log.entry("tool-1").unwrap().status, ToolStatus::Error("failed".to_string()));
    }

    #[test]
    fn update_for_unknown_tool_is_ignored() {
        let mut log = ToolCallLog::new();
        log.on_tool_call_update(&status_update("ghost", acp::ToolCallStatus::Completed));

        assert!(!log.has_tool("ghost"));
    }

    #[test]
    fn update_replaces_title_when_provided() {
        let mut log = ToolCallLog::new();
        log.on_tool_call(&acp::ToolCall::new("tool-1".to_string(), "Read"));
        log.on_tool_call_update(&acp::ToolCallUpdate::new(
            "tool-1".to_string(),
            acp::ToolCallUpdateFields::new().title("Read main.rs"),
        ));

        assert_eq!(log.entry("tool-1").unwrap().title, "Read main.rs");
    }

    #[test]
    fn finalize_running_only_touches_running_entries() {
        let mut log = ToolCallLog::new();
        log.on_tool_call(&acp::ToolCall::new("done".to_string(), "Done tool"));
        log.on_tool_call_update(&status_update("done", acp::ToolCallStatus::Completed));
        log.on_tool_call(&acp::ToolCall::new("stuck".to_string(), "Stuck tool"));

        log.finalize_running(&ToolStatus::Error("cancelled".to_string()));

        assert_eq!(log.entry("done").unwrap().status, ToolStatus::Success);
        assert_eq!(log.entry("stuck").unwrap().status, ToolStatus::Error("cancelled".to_string()));
    }

    #[test]
    fn remove_returns_entry_and_forgets_it() {
        let mut log = ToolCallLog::new();
        log.on_tool_call(&acp::ToolCall::new("tool-1".to_string(), "Read file"));

        let removed = log.remove("tool-1").unwrap();

        assert_eq!(removed.title, "Read file");
        assert!(!log.has_tool("tool-1"));
        assert!(log.remove("tool-1").is_none());
    }

    fn tool_call_with_raw(id: &str, title: &str, raw_input: serde_json::Value) -> acp::ToolCall {
        acp::ToolCall::new(id.to_string(), title).raw_input(raw_input)
    }

    fn tool_call_update_with_raw(id: &str, raw_input: serde_json::Value) -> acp::ToolCallUpdate {
        acp::ToolCallUpdate::new(id.to_string(), acp::ToolCallUpdateFields::new().raw_input(raw_input))
    }

    fn tool_call_update_with_meta(id: &str, key: &str, value: serde_json::Value) -> acp::ToolCallUpdate {
        let mut meta = serde_json::Map::new();
        meta.insert(key.to_string(), value);
        acp::ToolCallUpdate::new(id.to_string(), acp::ToolCallUpdateFields::new()).meta(meta)
    }

    fn tool_call_update_with_display_value(id: &str, display_value: &str) -> acp::ToolCallUpdate {
        tool_call_update_with_meta(id, "display_value", serde_json::Value::String(display_value.to_string()))
    }

    #[test]
    fn initial_raw_input_json_object_is_stored_as_serialized_json() {
        let mut log = ToolCallLog::new();
        let raw = serde_json::json!({"file_path": "/src/main.rs", "offset": 10});
        log.on_tool_call(&tool_call_with_raw("tool-1", "Read file", raw));

        let entry = log.entry("tool-1").unwrap();
        assert_eq!(entry.raw_input, "{\"file_path\":\"/src/main.rs\",\"offset\":10}");
    }

    #[test]
    fn initial_raw_input_json_string_is_stored_as_plain_string() {
        let mut log = ToolCallLog::new();
        let raw = serde_json::Value::String("a plain string argument".to_string());
        log.on_tool_call(&tool_call_with_raw("tool-1", "Read file", raw));

        let entry = log.entry("tool-1").unwrap();
        assert_eq!(entry.raw_input, "a plain string argument");
    }

    #[test]
    fn missing_raw_input_defaults_to_empty_string() {
        let mut log = ToolCallLog::new();
        log.on_tool_call(&acp::ToolCall::new("tool-1".to_string(), "Read file"));

        let entry = log.entry("tool-1").unwrap();
        assert_eq!(entry.raw_input, "");
    }

    #[test]
    fn streamed_raw_input_fragments_are_concatenated_in_arrival_order() {
        let mut log = ToolCallLog::new();
        log.on_tool_call(&acp::ToolCall::new("tool-1".to_string(), "Edit file"));

        log.on_tool_call_update(&tool_call_update_with_raw("tool-1", serde_json::Value::String("first ".to_string())));
        log.on_tool_call_update(&tool_call_update_with_raw("tool-1", serde_json::Value::String("second ".to_string())));
        log.on_tool_call_update(&tool_call_update_with_raw("tool-1", serde_json::Value::String("third".to_string())));

        let entry = log.entry("tool-1").unwrap();
        assert_eq!(entry.raw_input, "first second third");
    }

    #[test]
    fn duplicate_tool_call_request_updates_raw_input_of_existing_entry() {
        let mut log = ToolCallLog::new();
        log.on_tool_call(&tool_call_with_raw("tool-1", "First", serde_json::Value::String("old".to_string())));

        log.on_tool_call(&tool_call_with_raw("tool-1", "Second", serde_json::Value::String("new".to_string())));

        let entry = log.entry("tool-1").unwrap();
        assert_eq!(entry.title, "Second");
        assert_eq!(entry.raw_input, "new");
    }

    #[test]
    fn display_value_overrides_raw_input_for_display() {
        let mut log = ToolCallLog::new();
        log.on_tool_call(&tool_call_with_raw("tool-1", "Read file", serde_json::json!({"path": "/src/main.rs"})));

        log.on_tool_call_update(&tool_call_update_with_display_value("tool-1", "42 lines read"));

        let entry = log.entry("tool-1").unwrap();
        assert_eq!(entry.display_value, Some("42 lines read".to_string()));
        assert_eq!(entry.raw_input, "{\"path\":\"/src/main.rs\"}");
    }

    #[test]
    fn updated_title_overrides_original_title() {
        let mut log = ToolCallLog::new();
        log.on_tool_call(&acp::ToolCall::new("tool-1".to_string(), "Original"));

        log.on_tool_call_update(&tool_call_update_with_meta(
            "tool-1",
            "display_value",
            serde_json::Value::String("done".to_string()),
        ));

        let entry = log.entry("tool-1").unwrap();
        assert_eq!(entry.title, "Original");
        assert_eq!(entry.display_value, Some("done".to_string()));
    }

    #[test]
    fn unicode_raw_input_is_preserved_as_is() {
        let mut log = ToolCallLog::new();
        let raw = serde_json::Value::String("こんにちは世界".to_string());
        log.on_tool_call(&tool_call_with_raw("tool-1", "Greet", raw));

        let entry = log.entry("tool-1").unwrap();
        assert_eq!(entry.raw_input, "こんにちは世界");
    }

    #[test]
    fn raw_input_preserved_through_remove() {
        let mut log = ToolCallLog::new();
        log.on_tool_call(&tool_call_with_raw("tool-1", "Read file", serde_json::json!({"path": "/src/main.rs"})));
        log.on_tool_call_update(&tool_call_update_with_display_value("tool-1", "done"));
        log.on_tool_call_update(&status_update("tool-1", acp::ToolCallStatus::Completed));

        let removed = log.remove("tool-1").unwrap();

        assert_eq!(removed.raw_input, "{\"path\":\"/src/main.rs\"}");
        assert_eq!(removed.display_value, Some("done".to_string()));
        assert_eq!(removed.status, ToolStatus::Success);
    }

    #[test]
    fn display_value_from_meta_is_ignored_when_absent() {
        let mut log = ToolCallLog::new();
        log.on_tool_call(&acp::ToolCall::new("tool-1".to_string(), "Read file"));

        let entry = log.entry("tool-1").unwrap();
        assert_eq!(entry.display_value, None);
    }

    #[test]
    fn sub_agent_tool_call_creates_child_entry() {
        let mut log = ToolCallLog::new();
        log.on_tool_call(&acp::ToolCall::new("parent-1".to_string(), "spawn_subagent"));
        log.on_sub_agent_progress(&sub_agent_event(
            "parent-1",
            "task-abc",
            "explorer",
            SubAgentEvent::ToolCall {
                request: SubAgentToolRequest {
                    id: "c1".to_string(),
                    name: "grep".to_string(),
                    arguments: r#"{"pattern":"test"}"#.to_string(),
                },
            },
        ));

        assert!(log.is_running("parent-1"));
        let agents = log.sub_agent_states("parent-1").unwrap();
        assert_eq!(agents.len(), 1);
        assert_eq!(agents[0].task_id, "task-abc");
        assert_eq!(agents[0].agent_name, "explorer");
        assert!(!agents[0].done);
        assert_eq!(agents[0].tool_order, vec!["c1"]);
    }

    #[test]
    fn sub_agent_tool_call_update_appends_arguments() {
        let mut log = ToolCallLog::new();
        log.on_tool_call(&acp::ToolCall::new("parent-1".to_string(), "spawn_subagent"));
        log.on_sub_agent_progress(&sub_agent_event(
            "parent-1",
            "task-1",
            "explorer",
            SubAgentEvent::ToolCall {
                request: SubAgentToolRequest {
                    id: "c1".to_string(),
                    name: "grep".to_string(),
                    arguments: r#"{"pattern":"te"#.to_string(),
                },
            },
        ));
        log.on_sub_agent_progress(&sub_agent_event(
            "parent-1",
            "task-1",
            "explorer",
            SubAgentEvent::ToolCallUpdate {
                update: SubAgentToolCallUpdate { id: "c1".to_string(), chunk: "st\"}".to_string() },
            },
        ));

        let agents = log.sub_agent_states("parent-1").unwrap();
        let tc = agents[0].tool_calls.get("c1").unwrap();
        assert_eq!(tc.arguments, r#"{"pattern":"test"}"#);
    }

    #[test]
    fn sub_agent_tool_call_corrects_placeholder_when_update_arrived_first() {
        // If a streaming update arrives before the canonical ToolCall request, the entry is
        // created with a placeholder name. The later ToolCall must correct it.
        let mut log = ToolCallLog::new();
        log.on_tool_call(&acp::ToolCall::new("parent-1".to_string(), "spawn_subagent"));
        log.on_sub_agent_progress(&sub_agent_event(
            "parent-1",
            "task-1",
            "explorer",
            SubAgentEvent::ToolCallUpdate {
                update: SubAgentToolCallUpdate { id: "c1".to_string(), chunk: r#"{"pat"#.to_string() },
            },
        ));
        log.on_sub_agent_progress(&sub_agent_event(
            "parent-1",
            "task-1",
            "explorer",
            SubAgentEvent::ToolCall {
                request: SubAgentToolRequest {
                    id: "c1".to_string(),
                    name: "grep".to_string(),
                    arguments: r#"{"pattern":"test"}"#.to_string(),
                },
            },
        ));

        let agents = log.sub_agent_states("parent-1").unwrap();
        let tc = agents[0].tool_calls.get("c1").unwrap();
        assert_eq!(tc.name, "grep", "late ToolCall must overwrite the streaming placeholder name");
        assert_eq!(tc.arguments, r#"{"pattern":"test"}"#);
    }

    #[test]
    fn sub_agent_tool_result_applies_metadata() {
        let mut log = ToolCallLog::new();
        log.on_tool_call(&acp::ToolCall::new("parent-1".to_string(), "spawn_subagent"));
        log.on_sub_agent_progress(&sub_agent_event(
            "parent-1",
            "task-1",
            "explorer",
            SubAgentEvent::ToolCall {
                request: SubAgentToolRequest {
                    id: "c1".to_string(),
                    name: "grep".to_string(),
                    arguments: "{}".to_string(),
                },
            },
        ));
        log.on_sub_agent_progress(&sub_agent_event(
            "parent-1",
            "task-1",
            "explorer",
            SubAgentEvent::ToolResult {
                result: SubAgentToolResult {
                    id: "c1".to_string(),
                    name: "grep".to_string(),
                    result_meta: Some(ToolResultMeta::new(ToolDisplayMeta::new("Grep", "'test' in 3 files"))),
                },
            },
        ));

        let agents = log.sub_agent_states("parent-1").unwrap();
        let tc = agents[0].tool_calls.get("c1").unwrap();
        assert_eq!(tc.status, ToolStatus::Success);
        assert_eq!(tc.display_value.as_deref(), Some("'test' in 3 files"));
    }

    #[test]
    fn sub_agent_tool_error_marks_failed() {
        let mut log = ToolCallLog::new();
        log.on_tool_call(&acp::ToolCall::new("parent-1".to_string(), "spawn_subagent"));
        log.on_sub_agent_progress(&sub_agent_event(
            "parent-1",
            "task-1",
            "explorer",
            SubAgentEvent::ToolCall {
                request: SubAgentToolRequest {
                    id: "c1".to_string(),
                    name: "read_file".to_string(),
                    arguments: "{}".to_string(),
                },
            },
        ));
        log.on_sub_agent_progress(&sub_agent_event(
            "parent-1",
            "task-1",
            "explorer",
            SubAgentEvent::ToolError {
                error: SubAgentToolError { id: "c1".to_string(), name: "read_file".to_string() },
            },
        ));

        let agents = log.sub_agent_states("parent-1").unwrap();
        let tc = agents[0].tool_calls.get("c1").unwrap();
        assert_eq!(tc.status, ToolStatus::Error("failed".to_string()));
    }

    #[test]
    fn sub_agent_done_marks_agent_as_complete() {
        let mut log = ToolCallLog::new();
        log.on_tool_call(&acp::ToolCall::new("parent-1".to_string(), "spawn_subagent"));
        log.on_sub_agent_progress(&sub_agent_event("parent-1", "task-1", "explorer", SubAgentEvent::Done));

        let agents = log.sub_agent_states("parent-1").unwrap();
        assert!(agents[0].done);
    }

    #[test]
    fn sub_agent_other_is_harmless() {
        let mut log = ToolCallLog::new();
        log.on_tool_call(&acp::ToolCall::new("parent-1".to_string(), "spawn_subagent"));
        log.on_sub_agent_progress(&sub_agent_event("parent-1", "task-1", "explorer", SubAgentEvent::Other));

        let agents = log.sub_agent_states("parent-1").unwrap();
        assert!(!agents[0].done);
        assert!(agents[0].tool_order.is_empty());
    }

    #[test]
    fn sub_agent_multiple_task_ids_same_name() {
        let mut log = ToolCallLog::new();
        log.on_tool_call(&acp::ToolCall::new("parent-1".to_string(), "spawn_subagent"));
        log.on_sub_agent_progress(&sub_agent_event("parent-1", "task-a", "explorer", SubAgentEvent::Done));
        log.on_sub_agent_progress(&sub_agent_event("parent-1", "task-b", "explorer", SubAgentEvent::Done));

        let agents = log.sub_agent_states("parent-1").unwrap();
        assert_eq!(agents.len(), 2);
        assert_eq!(agents[0].task_id, "task-a");
        assert_eq!(agents[1].task_id, "task-b");
        assert_eq!(agents[0].agent_name, "explorer");
        assert_eq!(agents[1].agent_name, "explorer");
    }

    #[test]
    fn sub_agent_is_running_includes_children() {
        let mut log = ToolCallLog::new();
        log.on_tool_call(&acp::ToolCall::new("parent-1".to_string(), "spawn_subagent"));
        log.on_tool_call_update(&status_update("parent-1", acp::ToolCallStatus::Completed));

        // parent completed but no sub-agents → not running
        assert!(!log.is_running("parent-1"));

        log.on_sub_agent_progress(&sub_agent_event(
            "parent-1",
            "task-1",
            "explorer",
            SubAgentEvent::ToolCall {
                request: SubAgentToolRequest {
                    id: "c1".to_string(),
                    name: "grep".to_string(),
                    arguments: "{}".to_string(),
                },
            },
        ));

        // parent completed but has running sub-agent → running
        assert!(log.is_running("parent-1"));

        log.on_sub_agent_progress(&sub_agent_event(
            "parent-1",
            "task-1",
            "explorer",
            SubAgentEvent::ToolResult {
                result: SubAgentToolResult { id: "c1".to_string(), name: "grep".to_string(), result_meta: None },
            },
        ));

        // child completed but agent not done → still running (agent itself not done)
        assert!(log.is_running("parent-1"));

        log.on_sub_agent_progress(&sub_agent_event("parent-1", "task-1", "explorer", SubAgentEvent::Done));

        // agent done, all tools complete → not running
        assert!(!log.is_running("parent-1"));
    }

    #[test]
    fn sub_agent_any_running_includes_all_parents() {
        let mut log = ToolCallLog::new();
        log.on_tool_call(&acp::ToolCall::new("parent-1".to_string(), "spawn_subagent_a"));
        log.on_tool_call(&acp::ToolCall::new("parent-2".to_string(), "spawn_subagent_b"));

        log.on_tool_call_update(&status_update("parent-1", acp::ToolCallStatus::Completed));
        log.on_tool_call_update(&status_update("parent-2", acp::ToolCallStatus::Completed));

        assert!(!log.any_running());

        log.on_sub_agent_progress(&sub_agent_event(
            "parent-1",
            "task-1",
            "explorer",
            SubAgentEvent::ToolCall {
                request: SubAgentToolRequest {
                    id: "c1".to_string(),
                    name: "grep".to_string(),
                    arguments: "{}".to_string(),
                },
            },
        ));

        assert!(log.any_running());
    }

    #[test]
    fn sub_agent_finalize_running_marks_child_tools_terminal() {
        let mut log = ToolCallLog::new();
        log.on_tool_call(&acp::ToolCall::new("parent-1".to_string(), "spawn_subagent"));
        log.on_sub_agent_progress(&sub_agent_event(
            "parent-1",
            "task-1",
            "explorer",
            SubAgentEvent::ToolCall {
                request: SubAgentToolRequest {
                    id: "c1".to_string(),
                    name: "grep".to_string(),
                    arguments: "{}".to_string(),
                },
            },
        ));

        log.finalize_running(&ToolStatus::Error("cancelled".to_string()));

        let agents = log.sub_agent_states("parent-1").unwrap();
        assert!(agents[0].done);
        let tc = agents[0].tool_calls.get("c1").unwrap();
        assert_eq!(tc.status, ToolStatus::Error("cancelled".to_string()));
    }

    #[test]
    fn sub_agent_clear_removes_all_state() {
        let mut log = ToolCallLog::new();
        log.on_tool_call(&acp::ToolCall::new("parent-1".to_string(), "spawn_subagent"));
        log.on_sub_agent_progress(&sub_agent_event(
            "parent-1",
            "task-1",
            "explorer",
            SubAgentEvent::ToolCall {
                request: SubAgentToolRequest {
                    id: "c1".to_string(),
                    name: "grep".to_string(),
                    arguments: "{}".to_string(),
                },
            },
        ));

        assert!(log.any_running());

        log.clear();

        assert!(!log.any_running());
        assert!(log.sub_agent_states("parent-1").is_none());
    }

    #[test]
    fn sub_agent_remove_cleans_up_sub_agent_state() {
        let mut log = ToolCallLog::new();
        log.on_tool_call(&acp::ToolCall::new("parent-1".to_string(), "spawn_subagent"));
        log.on_tool_call_update(&status_update("parent-1", acp::ToolCallStatus::Completed));
        log.on_sub_agent_progress(&sub_agent_event("parent-1", "task-1", "explorer", SubAgentEvent::Done));

        assert!(!log.is_running("parent-1"));
        assert!(log.sub_agent_states("parent-1").is_some());

        let _ = log.remove("parent-1");

        assert!(!log.has_tool("parent-1"));
        assert!(log.sub_agent_states("parent-1").is_none());
    }

    #[test]
    fn sub_agent_late_event_after_remove_recreates_state() {
        let mut log = ToolCallLog::new();
        log.on_tool_call(&acp::ToolCall::new("parent-1".to_string(), "spawn_subagent"));
        log.on_sub_agent_progress(&sub_agent_event(
            "parent-1",
            "task-1",
            "explorer",
            SubAgentEvent::ToolCall {
                request: SubAgentToolRequest {
                    id: "c1".to_string(),
                    name: "grep".to_string(),
                    arguments: "{}".to_string(),
                },
            },
        ));

        // drain: parent completed, child tool finished, sub-agent done
        log.on_tool_call_update(&status_update("parent-1", acp::ToolCallStatus::Completed));
        log.on_sub_agent_progress(&sub_agent_event(
            "parent-1",
            "task-1",
            "explorer",
            SubAgentEvent::ToolResult {
                result: SubAgentToolResult { id: "c1".to_string(), name: "grep".to_string(), result_meta: None },
            },
        ));
        log.on_sub_agent_progress(&sub_agent_event("parent-1", "task-1", "explorer", SubAgentEvent::Done));
        assert!(!log.is_running("parent-1"));

        let removed_parent = log.remove("parent-1");
        assert!(removed_parent.is_some());
        assert!(log.sub_agent_states("parent-1").is_none());

        // late event after removal — re-creates sub-agent entry in tracker
        // but this is harmless since the parent tool is already drained
        log.on_sub_agent_progress(&sub_agent_event("parent-1", "task-1", "explorer", SubAgentEvent::Done));

        // late events re-create sub-agent state even after remove
        assert!(log.sub_agent_states("parent-1").is_some());
        // but any_running is still false since the agent is Done
        assert!(!log.any_running());
    }

    #[test]
    fn sub_agent_event_ordering_is_preserved() {
        let mut log = ToolCallLog::new();
        log.on_tool_call(&acp::ToolCall::new("parent-1".to_string(), "spawn_subagent"));
        log.on_sub_agent_progress(&sub_agent_event(
            "parent-1",
            "task-1",
            "explorer",
            SubAgentEvent::ToolCall {
                request: SubAgentToolRequest {
                    id: "c1".to_string(),
                    name: "first".to_string(),
                    arguments: "1".to_string(),
                },
            },
        ));
        log.on_sub_agent_progress(&sub_agent_event(
            "parent-1",
            "task-1",
            "explorer",
            SubAgentEvent::ToolCall {
                request: SubAgentToolRequest {
                    id: "c2".to_string(),
                    name: "second".to_string(),
                    arguments: "2".to_string(),
                },
            },
        ));
        log.on_sub_agent_progress(&sub_agent_event(
            "parent-1",
            "task-1",
            "explorer",
            SubAgentEvent::ToolCall {
                request: SubAgentToolRequest {
                    id: "c3".to_string(),
                    name: "third".to_string(),
                    arguments: "3".to_string(),
                },
            },
        ));

        let agents = log.sub_agent_states("parent-1").unwrap();
        assert_eq!(agents[0].tool_order, vec!["c1", "c2", "c3"]);
        assert_eq!(agents[0].tool_calls["c1"].name, "first");
        assert_eq!(agents[0].tool_calls["c2"].name, "second");
        assert_eq!(agents[0].tool_calls["c3"].name, "third");
    }

    #[test]
    fn sub_agent_child_tool_has_enriched_metadata_from_parent() {
        let mut log = ToolCallLog::new();
        log.on_tool_call(&acp::ToolCall::new("parent-1".to_string(), "spawn_subagent"));
        log.on_sub_agent_progress(&sub_agent_event(
            "parent-1",
            "task-1",
            "explorer",
            SubAgentEvent::ToolCall {
                request: SubAgentToolRequest {
                    id: "c1".to_string(),
                    name: "read_file".to_string(),
                    arguments: r#"{"filePath":"/src/main.rs"}"#.to_string(),
                },
            },
        ));
        log.on_sub_agent_progress(&sub_agent_event(
            "parent-1",
            "task-1",
            "explorer",
            SubAgentEvent::ToolResult {
                result: SubAgentToolResult {
                    id: "c1".to_string(),
                    name: "read_file".to_string(),
                    result_meta: Some(ToolResultMeta::new(ToolDisplayMeta::new("Read file", "src/main.rs, 42 lines"))),
                },
            },
        ));

        let agents = log.sub_agent_states("parent-1").unwrap();
        let tc = agents[0].tool_calls.get("c1").unwrap();
        assert_eq!(tc.name, "Read file");
        assert_eq!(tc.display_value.as_deref(), Some("src/main.rs, 42 lines"));
        assert_eq!(tc.raw_input, r#"{"filePath":"/src/main.rs"}"#);
    }
}
