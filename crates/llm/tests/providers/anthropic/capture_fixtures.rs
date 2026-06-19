//! Re-capture committed Anthropic fixtures by hitting `api.anthropic.com`
//! directly.
//!
//! Each scenario is an `#[ignore]`'d test so the regular `cargo nextest run`
//! never spends money. To refresh, set `ANTHROPIC_API_KEY` and run:
//!
//! ```sh
//! cargo nextest run --run-ignored only -E 'test(capture_anthropic)'
//! ```
//!
//! Add a single scenario by name with e.g. `test(capture_anthropic_02_tool_call)`.

use serde_json::{Value, json};

use crate::providers::common::{lorem_filler, post_json_capture, require_env, write_fixture};

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
        "messages": [{"role": "user", "content": "Reply with exactly: hi"}],
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
        "tools": [{
            "name": "get_weather",
            "description": "Get the current weather for a city.",
            "input_schema": {
                "type": "object",
                "properties": {"city": {"type": "string"}},
                "required": ["city"],
            },
        }],
        "messages": [{"role": "user", "content": "What's the weather in Tokyo? Use the tool."}],
    }))
    .await;
    write_fixture("anthropic", "02_tool_call", &bytes);
}

/// `03_cache_write` populates the prompt cache; `04_cache_read` re-sends the
/// same prefix and should report non-zero `cache_read_input_tokens`. They must
/// be captured back-to-back, so they live in a single test function.
#[tokio::test]
#[ignore = "captures live fixtures, requires ANTHROPIC_API_KEY"]
async fn capture_anthropic_03_04_cache_pair() {
    // Comfortably above any per-model cache minimum (Anthropic's smallest
    // documented threshold is 1024 tokens; 2500 sat right on the edge for
    // claude-haiku-4-5 and produced cache_creation_input_tokens=0).
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
    let client = reqwest::Client::new();
    post_json_capture(&client, URL, &[("x-api-key", key.as_str()), ("anthropic-version", "2023-06-01")], body).await
}
