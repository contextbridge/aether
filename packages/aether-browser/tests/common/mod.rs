const FAKE_AGENT_WS_URL: &str = env!("FAKE_AGENT_WS_URL");

/// The fake agent's WebSocket URL for `scenario`.
pub fn url(scenario: &str) -> String {
    format!("{FAKE_AGENT_WS_URL}{scenario}")
}
