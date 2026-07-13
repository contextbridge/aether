use acp_utils::notifications::SubAgentProgressParams;
use agent_client_protocol::schema as acp;
use std::collections::HashMap;
use std::time::Instant;

use crate::components::sub_agent_tracker::SubAgentTracker;
use crate::components::tool_call_status_view::{ToolCallStatus, diff_preview_from_acp, render_tool_tree};
use crate::components::tracked_tool_call::{TrackedToolCall, raw_input_fragment, upsert_tracked_tool_call};
use tui::{Frame, ViewContext};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PromptTermination {
    EndTurn,
    Cancelled,
    Failed(String),
}

impl PromptTermination {
    pub(crate) fn terminal_status(&self) -> ToolCallStatus {
        match self {
            PromptTermination::EndTurn => ToolCallStatus::Success,
            PromptTermination::Cancelled => ToolCallStatus::Error("cancelled".to_string()),
            PromptTermination::Failed(msg) => ToolCallStatus::Error(format!("failed: {msg}")),
        }
    }
}

/// Tracks active tool calls and produces status lines for the frame.
#[derive(Clone)]
pub struct ToolCallStatuses {
    /// Ordered list of tool call IDs (insertion order)
    tool_order: Vec<String>,
    /// Tool call info by ID
    tool_calls: HashMap<String, TrackedToolCall>,
    /// Sub-agent states keyed by parent tool call ID
    sub_agents: SubAgentTracker,
    /// Animation tick for the spinner on running tool calls
    tick: u16,
}

impl ToolCallStatuses {
    pub fn new() -> Self {
        Self { tool_order: Vec::new(), tool_calls: HashMap::new(), sub_agents: SubAgentTracker::default(), tick: 0 }
    }

    pub fn running_any(&self) -> bool {
        self.tool_calls.values().any(|tc| matches!(tc.status, ToolCallStatus::Running)) || self.sub_agents.any_running()
    }

    /// Advance the animation state. Call this on tick events.
    pub fn on_tick(&mut self, _now: Instant) {
        if self.running_any() {
            self.tick = self.tick.wrapping_add(1);
        }
    }

    /// Handle a new tool call from ACP `SessionUpdate::ToolCall`.
    pub fn on_tool_call(&mut self, tool_call: &acp::ToolCall) {
        let id = tool_call.tool_call_id.0.to_string();
        let arguments = tool_call.raw_input.as_ref().map(raw_input_fragment).unwrap_or_default();

        let tracked = upsert_tracked_tool_call(
            &mut self.tool_order,
            &mut self.tool_calls,
            &id,
            &tool_call.title,
            arguments.clone(),
        );
        tracked.update_name(&tool_call.title);
        tracked.arguments = arguments;
        tracked.status = ToolCallStatus::Running;
    }

    /// Handle a tool call update from ACP `SessionUpdate::ToolCallUpdate`.
    pub fn on_tool_call_update(&mut self, update: &acp::ToolCallUpdate) {
        let id = update.tool_call_id.0.to_string();

        if let Some(tc) = self.tool_calls.get_mut(&id) {
            if let Some(title) = &update.fields.title {
                tc.update_name(title);
            }
            if let Some(raw_input) = &update.fields.raw_input {
                tc.append_arguments(&raw_input_fragment(raw_input));
            }
            if let Some(meta) = &update.meta
                && let Some(dv) = meta.get("display_value").and_then(|v| v.as_str())
            {
                tc.display_value = Some(dv.to_string());
            }
            if let Some(content) = &update.fields.content {
                for item in content {
                    if let acp::ToolCallContent::Diff(diff) = item {
                        tc.diff_preview = Some(diff_preview_from_acp(diff));
                    }
                }
            }
            if let Some(status) = update.fields.status {
                tc.apply_status(status);
            }
        }
    }

    pub fn finalize_running(&mut self, termination: &PromptTermination) {
        let terminal_status = termination.terminal_status();

        for tool_call in self.tool_calls.values_mut() {
            if matches!(tool_call.status, ToolCallStatus::Running) {
                tool_call.status = terminal_status.clone();
            }
        }

        self.sub_agents.finalize_running(termination);
    }

    pub fn has_tool(&self, id: &str) -> bool {
        self.tool_calls.contains_key(id)
    }

    #[cfg(test)]
    pub fn is_tool_running(&self, id: &str) -> bool {
        self.tool_calls.get(id).is_some_and(|tc| matches!(tc.status, ToolCallStatus::Running))
    }

    /// Handle a sub-agent progress notification.
    pub fn on_sub_agent_progress(&mut self, notification: &SubAgentProgressParams) {
        self.sub_agents.on_progress(notification);
    }

    #[cfg(test)]
    pub fn remove_tool(&mut self, id: &str) {
        self.tool_calls.remove(id);
        self.tool_order.retain(|tool_id| tool_id != id);
        self.sub_agents.remove(id);
    }

    pub fn render_tool(&self, id: &str, context: &ViewContext) -> Frame {
        render_tool_tree(id, &self.tool_calls, &self.sub_agents, self.tick, context)
    }

    /// Clear all tracked tool calls (e.g., after pushing to scrollback).
    pub fn clear(&mut self) {
        self.tool_order.clear();
        self.tool_calls.clear();
        self.sub_agents.clear();
    }
}

impl Default for ToolCallStatuses {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use acp_utils::notifications::{SubAgentEvent, SubAgentProgressParams};
    use tui::{DiffLine, DiffPreview, DiffTag, SplitDiffEntry, SplitDiffRow};

    fn ctx() -> ViewContext {
        ViewContext::new((80, 24))
    }

    fn make_tool_call(id: &str, title: &str, raw_input: Option<&str>) -> acp::ToolCall {
        let mut tc = acp::ToolCall::new(id.to_string(), title);
        if let Some(input) = raw_input {
            tc = tc.raw_input(serde_json::from_str::<serde_json::Value>(input).unwrap());
        }
        tc
    }

    fn make_tool_call_update(id: &str, status: acp::ToolCallStatus) -> acp::ToolCallUpdate {
        acp::ToolCallUpdate::new(id.to_string(), acp::ToolCallUpdateFields::new().status(status))
    }

    fn make_sub_agent_notification(parent_tool_id: &str, agent_name: &str, event_json: &str) -> SubAgentProgressParams {
        make_sub_agent_notification_with_task_id(parent_tool_id, agent_name, agent_name, event_json)
    }

    fn make_sub_agent_notification_with_task_id(
        parent_tool_id: &str,
        task_id: &str,
        agent_name: &str,
        event_json: &str,
    ) -> SubAgentProgressParams {
        let json = format!(
            r#"{{"parent_tool_id":"{parent_tool_id}","task_id":"{task_id}","agent_name":"{agent_name}","event":{event_json}}}"#,
        );
        serde_json::from_str(&json).unwrap()
    }

    #[test]
    fn progress_reports_sub_agent_running_tools() {
        let mut statuses = ToolCallStatuses::new();
        statuses.on_tool_call(&make_tool_call("parent-1", "spawn_subagent", None));
        statuses.on_tool_call_update(&make_tool_call_update("parent-1", acp::ToolCallStatus::Completed));
        statuses.on_sub_agent_progress(&make_sub_agent_notification(
            "parent-1",
            "explorer",
            r#"{"ToolCall":{"request":{"id":"c1","name":"grep","arguments":"{}"},"model_name":"m"}}"#,
        ));

        assert!(statuses.running_any());
    }

    #[test]
    fn remove_tool_cleans_up_sub_agent_state() {
        let mut statuses = ToolCallStatuses::new();
        statuses.on_tool_call(&make_tool_call("parent-1", "spawn_subagent", None));
        statuses.on_sub_agent_progress(&make_sub_agent_notification(
            "parent-1",
            "explorer",
            r#"{"ToolCall":{"request":{"id":"c1","name":"grep","arguments":"{}"},"model_name":"m"}}"#,
        ));

        statuses.remove_tool("parent-1");
        assert!(!statuses.running_any());
        assert!(statuses.render_tool("parent-1", &ctx()).lines().is_empty());
    }

    #[test]
    fn clear_removes_sub_agent_state() {
        let mut statuses = ToolCallStatuses::new();
        statuses.on_tool_call(&make_tool_call("parent-1", "spawn_subagent", None));
        statuses.on_sub_agent_progress(&make_sub_agent_notification(
            "parent-1",
            "explorer",
            r#"{"ToolCall":{"request":{"id":"c1","name":"grep","arguments":"{}"},"model_name":"m"}}"#,
        ));

        statuses.clear();
        assert!(!statuses.running_any());
    }

    #[test]
    fn deserialize_tool_call_event() {
        let n = make_sub_agent_notification(
            "p1",
            "explorer",
            r#"{"ToolCall":{"request":{"id":"c1","name":"grep","arguments":"{\"pattern\":\"test\"}"},"model_name":"m"}}"#,
        );
        assert!(matches!(n.event, SubAgentEvent::ToolCall { .. }));
    }

    #[test]
    fn deserialize_tool_call_update_event() {
        let n = make_sub_agent_notification(
            "p1",
            "explorer",
            r#"{"ToolCallUpdate":{"update":{"id":"c1","chunk":"{\"pattern\":\"updated\"}"},"model_name":"m"}}"#,
        );
        assert!(matches!(n.event, SubAgentEvent::ToolCallUpdate { .. }));
    }

    #[test]
    fn deserialize_tool_result_event() {
        let n = make_sub_agent_notification(
            "p1",
            "explorer",
            r#"{"ToolResult":{"result":{"id":"c1","name":"grep","arguments":"{}","result":"ok"},"model_name":"m"}}"#,
        );
        assert!(matches!(n.event, SubAgentEvent::ToolResult { .. }));
    }

    #[test]
    fn deserialize_done_event() {
        let n = make_sub_agent_notification("p1", "explorer", r#""Done""#);
        assert!(matches!(n.event, SubAgentEvent::Done));
    }

    #[test]
    fn deserialize_other_variant() {
        let n = make_sub_agent_notification("p1", "explorer", r#""Other""#);
        assert!(matches!(n.event, SubAgentEvent::Other));
    }

    #[test]
    fn test_diff_preview_rendered_on_success() {
        let mut statuses = ToolCallStatuses::new();
        statuses.on_tool_call(&make_tool_call("tool-1", "Edit", None));

        let tc = statuses.tool_calls.get_mut("tool-1").unwrap();
        tc.status = ToolCallStatus::Success;
        tc.diff_preview = Some(DiffPreview {
            lines: vec![
                DiffLine { tag: DiffTag::Removed, content: "old line".to_string() },
                DiffLine { tag: DiffTag::Added, content: "new line".to_string() },
            ],
            rows: vec![SplitDiffRow {
                left: Some(SplitDiffEntry::new(DiffTag::Removed, "old line", Some(1))),
                right: Some(SplitDiffEntry::new(DiffTag::Added, "new line", Some(1))),
            }],
            lang_hint: "rs".to_string(),
            start_line: Some(1),
        });

        let frame = statuses.render_tool("tool-1", &ctx());
        let lines = frame.lines();
        assert!(lines.len() > 1);
        let all_text: String = lines.iter().map(tui::Line::plain_text).collect();
        assert!(all_text.contains("old line"), "Expected removed line: {all_text}");
        assert!(all_text.contains("new line"), "Expected added line: {all_text}");
    }

    #[test]
    fn test_diff_preview_not_rendered_while_running() {
        let mut statuses = ToolCallStatuses::new();
        statuses.on_tool_call(&make_tool_call("tool-1", "Edit", None));

        let tc = statuses.tool_calls.get_mut("tool-1").unwrap();
        tc.diff_preview = Some(DiffPreview {
            lines: vec![DiffLine { tag: DiffTag::Added, content: "new line".to_string() }],
            rows: vec![SplitDiffRow {
                left: None,
                right: Some(SplitDiffEntry::new(DiffTag::Added, "new line", Some(1))),
            }],
            lang_hint: "rs".to_string(),
            start_line: Some(1),
        });

        let frame = statuses.render_tool("tool-1", &ctx());
        assert_eq!(frame.lines().len(), 1, "Should only have status line while running");
    }

    #[test]
    fn finalize_running_marks_top_level_tools_terminal() {
        let mut statuses = ToolCallStatuses::new();
        statuses.on_tool_call(&make_tool_call("tool-1", "Read", None));

        statuses.finalize_running(&PromptTermination::EndTurn);

        assert!(!statuses.is_tool_running("tool-1"));
        assert!(!statuses.running_any());
        let frame = statuses.render_tool("tool-1", &ctx());
        assert!(frame.lines()[0].plain_text().contains('✓'));
    }

    #[test]
    fn finalize_running_marks_sub_agent_tools_terminal() {
        let mut statuses = ToolCallStatuses::new();
        statuses.on_sub_agent_progress(&make_sub_agent_notification(
            "parent-1",
            "explorer",
            r#"{"ToolCall":{"request":{"id":"c1","name":"grep","arguments":"{}"},"model_name":"m"}}"#,
        ));

        assert!(statuses.running_any());

        statuses.finalize_running(&PromptTermination::Cancelled);

        assert!(!statuses.running_any());
    }

    #[test]
    fn finalize_running_failed_variant_preserves_cause() {
        let mut statuses = ToolCallStatuses::new();
        statuses.on_tool_call(&make_tool_call("tool-1", "Read", None));
        assert!(statuses.is_tool_running("tool-1"));

        statuses.finalize_running(&PromptTermination::Failed("error".to_string()));

        assert!(!statuses.is_tool_running("tool-1"));
        assert!(!statuses.running_any());
    }

    #[test]
    fn finalize_running_cancelled_vs_failed_both_error() {
        let mut cancelled = ToolCallStatuses::new();
        cancelled.on_tool_call(&make_tool_call("t1", "test", None));
        cancelled.finalize_running(&PromptTermination::Cancelled);

        let mut failed = ToolCallStatuses::new();
        failed.on_tool_call(&make_tool_call("t1", "test", None));
        failed.finalize_running(&PromptTermination::Failed("error".to_string()));

        let cancelled_frame = cancelled.render_tool("t1", &ctx());
        let failed_frame = failed.render_tool("t1", &ctx());
        assert!(!cancelled_frame.lines()[0].plain_text().contains('\u{2713}'));
        assert!(!failed_frame.lines()[0].plain_text().contains('\u{2713}'));
    }

    #[test]
    fn running_any_false_when_sub_agent_done() {
        let mut statuses = ToolCallStatuses::new();
        statuses.on_sub_agent_progress(&make_sub_agent_notification(
            "parent-1",
            "explorer",
            r#"{"ToolCall":{"request":{"id":"c1","name":"grep","arguments":"{}"},"model_name":"m"}}"#,
        ));
        assert!(statuses.running_any());

        statuses.on_sub_agent_progress(&make_sub_agent_notification("parent-1", "explorer", r#""Done""#));
        assert!(statuses.running_any());

        statuses.finalize_running(&PromptTermination::EndTurn);
        assert!(!statuses.running_any());
    }
}
