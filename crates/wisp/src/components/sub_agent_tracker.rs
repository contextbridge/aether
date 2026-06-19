use std::collections::HashMap;

use acp_utils::notifications::{SubAgentEvent, SubAgentProgressParams};

use crate::components::tool_call_status_view::ToolCallStatus;
use crate::components::tracked_tool_call::{TrackedToolCall, upsert_tracked_tool_call};

pub(crate) const SUB_AGENT_VISIBLE_TOOL_LIMIT: usize = 3;

/// Per-sub-agent state: tracks its tool calls in order.
#[derive(Clone)]
pub(crate) struct SubAgentState {
    pub(crate) task_id: String,
    pub(crate) agent_name: String,
    pub(crate) done: bool,
    pub(crate) tool_order: Vec<String>,
    pub(crate) tool_calls: HashMap<String, TrackedToolCall>,
}

impl SubAgentState {
    pub(crate) fn is_active_for_render(&self) -> bool {
        !self.done || self.tool_calls.values().any(|tc| matches!(tc.status, ToolCallStatus::Running))
    }
}

/// Manages sub-agent state for tool calls that spawn child agents.
///
/// Keyed by parent tool call ID; each parent can have multiple sub-agents
/// tracked in insertion order.
#[derive(Clone, Default)]
pub(crate) struct SubAgentTracker {
    agents: HashMap<String, Vec<SubAgentState>>,
}

impl SubAgentTracker {
    pub(crate) fn on_progress(&mut self, notification: &SubAgentProgressParams) {
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
                let tracked = upsert_tracked_tool_call(
                    &mut agent.tool_order,
                    &mut agent.tool_calls,
                    &request.id,
                    &request.name,
                    request.arguments.clone(),
                );
                tracked.update_name(&request.name);
                tracked.arguments.clone_from(&request.arguments);
                tracked.status = ToolCallStatus::Running;
            }
            SubAgentEvent::ToolCallUpdate { update } => {
                let tracked = upsert_tracked_tool_call(
                    &mut agent.tool_order,
                    &mut agent.tool_calls,
                    &update.id,
                    "tool",
                    String::new(),
                );
                tracked.append_arguments(&update.chunk);
                tracked.status = ToolCallStatus::Running;
            }
            SubAgentEvent::ToolResult { result } => {
                if let Some(tc) = agent.tool_calls.get_mut(&result.id) {
                    tc.status = ToolCallStatus::Success;
                    if let Some(result_meta) = &result.result_meta {
                        tc.apply_result_meta(result_meta.clone());
                    }
                }
            }
            SubAgentEvent::ToolError { error } => {
                if let Some(tc) = agent.tool_calls.get_mut(&error.id) {
                    tc.status = ToolCallStatus::Error("failed".to_string());
                }
            }
            SubAgentEvent::Done => {
                agent.done = true;
            }
            SubAgentEvent::Other => {}
        }
    }

    pub(crate) fn has_sub_agents(&self, tool_id: &str) -> bool {
        self.agents.get(tool_id).is_some_and(|a| !a.is_empty())
    }

    pub(crate) fn get(&self, tool_id: &str) -> Option<&[SubAgentState]> {
        self.agents.get(tool_id).map(std::vec::Vec::as_slice)
    }

    pub(crate) fn any_running(&self) -> bool {
        self.agents.values().any(|agents| agents.iter().any(SubAgentState::is_active_for_render))
    }

    pub(crate) fn finalize_running(&mut self, cancelled: bool) {
        let terminal_status =
            if cancelled { ToolCallStatus::Error("cancelled".to_string()) } else { ToolCallStatus::Success };

        for agents in self.agents.values_mut() {
            for agent in agents {
                agent.done = true;
                for tool_call in agent.tool_calls.values_mut() {
                    if matches!(tool_call.status, ToolCallStatus::Running) {
                        tool_call.status = terminal_status.clone();
                    }
                }
            }
        }
    }

    #[cfg(test)]
    pub(crate) fn remove(&mut self, id: &str) {
        self.agents.remove(id);
    }

    pub(crate) fn clear(&mut self) {
        self.agents.clear();
    }
}
