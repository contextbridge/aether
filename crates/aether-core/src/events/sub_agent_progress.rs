use super::AgentEvent;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// A sub-agent's event, tagged with the task and agent it came from.
///
/// `mcp-subagents` serializes this into MCP progress notification messages.
/// The parent agent parses it once at the tool-execution boundary into
/// `ToolEvent::SubAgentProgress`, so every consumer downstream sees a typed
/// event instead of JSON in a string.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, JsonSchema)]
pub struct SubAgentProgressPayload {
    pub task_id: String,
    pub agent_name: String,
    pub event: AgentEvent,
}

#[cfg(test)]
mod tests {
    use super::SubAgentProgressPayload;
    use crate::events::{AgentEvent, TurnOutcome};

    #[test]
    fn test_sub_agent_progress_payload_roundtrip() {
        let payload = SubAgentProgressPayload {
            task_id: "task_123".to_string(),
            agent_name: "explorer".to_string(),
            event: AgentEvent::turn_ended(TurnOutcome::Completed),
        };

        let json = serde_json::to_string(&payload).expect("serializable");
        let parsed: SubAgentProgressPayload = serde_json::from_str(&json).expect("deserializable");

        assert_eq!(payload, parsed);
    }
}
