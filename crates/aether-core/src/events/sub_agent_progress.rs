use super::AgentMessage;
use serde::{Deserialize, Serialize};

/// Payload for sub-agent progress updates emitted by MCP tools.
///
/// This is the internal payload embedded in MCP progress messages between
/// `mcp-subagents` and the ACP relay (in `aether-cli`). It uses `AgentMessage`
/// for the event (the full fat type). The relay converts this to
/// `SubAgentProgressParams` (which uses the lightweight `SubAgentEvent`)
/// before sending to clients.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SubAgentProgressPayload {
    pub task_id: String,
    pub agent_name: String,
    pub event: AgentMessage,
}

#[cfg(test)]
mod tests {
    use super::SubAgentProgressPayload;
    use crate::events::AgentMessage;

    #[test]
    fn test_sub_agent_progress_payload_roundtrip() {
        let payload = SubAgentProgressPayload {
            task_id: "task_123".to_string(),
            agent_name: "explorer".to_string(),
            event: AgentMessage::Done,
        };

        let json = serde_json::to_string(&payload).expect("serializable");
        let parsed: SubAgentProgressPayload = serde_json::from_str(&json).expect("deserializable");

        assert_eq!(payload, parsed);
    }
}
