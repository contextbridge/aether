use crate::providers::common::{
    MINIMAL_PROMPT, TOOL_PROMPT, lorem_filler, post_json_capture, require_env, weather_tool, write_fixture,
};
use serde_json::{Value, json};

const MODEL: &str = "claude-haiku-4-5";
const REASONING_MODEL: &str = "claude-sonnet-4-5";
const URL: &str = "https://api.anthropic.com/v1/messages";

#[tokio::test]
#[ignore = "captures live fixture, requires ANTHROPIC_API_KEY"]
async fn capture_anthropic_01_minimal() {
    let bytes = send(&json!({
        "model": MODEL,
        "max_tokens": 64,
        "stream": true,
        "messages": [{"role": "user", "content": MINIMAL_PROMPT}],
    }))
    .await;
    write_fixture("anthropic", "01_minimal", &bytes);
}

#[tokio::test]
#[ignore = "captures live fixture, requires ANTHROPIC_API_KEY"]
async fn capture_anthropic_02_tool_call() {
    let bytes = send(&json!({
        "model": MODEL,
        "max_tokens": 256,
        "stream": true,
        "tools": [weather_tool("input_schema")],
        "messages": [{"role": "user", "content": TOOL_PROMPT}],
    }))
    .await;
    write_fixture("anthropic", "02_tool_call", &bytes);
}

#[tokio::test]
#[ignore = "captures live fixtures, requires ANTHROPIC_API_KEY"]
async fn capture_anthropic_03_04_cache_pair() {
    let long_system = lorem_filler(5_000);
    let body = json!({
        "model": MODEL,
        "max_tokens": 32,
        "stream": true,
        "system": [{
            "type": "text",
            "text": long_system,
            "cache_control": {"type": "ephemeral"},
        }],
        "messages": [{"role": "user", "content": "ok"}],
    });

    let write_bytes = send(&body).await;
    write_fixture("anthropic", "03_cache_write", &write_bytes);
    let read_bytes = send(&body).await;
    write_fixture("anthropic", "04_cache_read", &read_bytes);
}

#[tokio::test]
#[ignore = "captures live fixture, requires ANTHROPIC_API_KEY"]
async fn capture_anthropic_05_reasoning() {
    let bytes = send(&json!({
        "model": REASONING_MODEL,
        "max_tokens": 4096,
        "stream": true,
        "thinking": {"type": "enabled", "budget_tokens": 2048},
        "messages": [{"role": "user", "content": "Briefly: what is 2+2?"}],
    }))
    .await;
    write_fixture("anthropic", "05_reasoning", &bytes);
}

async fn send(body: &Value) -> Vec<u8> {
    let key = require_env("ANTHROPIC_API_KEY");
    post_json_capture(URL, &[("x-api-key", &key), ("anthropic-version", "2023-06-01")], body).await
}
