//! Re-capture committed Z.ai fixtures by hitting the Z.ai coding endpoint
//! directly.
//!
//! Each scenario is an `#[ignore]`'d test so the regular `cargo nextest run`
//! never spends money. To refresh, set `ZAI_API_KEY` and run:
//!
//! ```sh
//! cargo nextest run --run-ignored only -E 'test(capture_z_ai)'
//! ```

use serde_json::{Value, json};

use crate::providers::common::{post_json_capture, require_env, write_fixture};

const MODEL: &str = "GLM-4.6";
// Subscription / coding endpoint, matching `providers::openai_compatible::generic::ZAI`.
const URL: &str = "https://api.z.ai/api/coding/paas/v4/chat/completions";

#[tokio::test]
#[ignore = "captures live fixture, requires ZAI_API_KEY"]
async fn capture_z_ai_01_minimal() {
    let bytes = send(&json!({
        "model": MODEL,
        "stream": true,
        "stream_options": {"include_usage": true},
        "messages": [{"role": "user", "content": "Reply with exactly: hi"}],
    }))
    .await;
    write_fixture("z_ai", "01_minimal", &bytes);
}

#[tokio::test]
#[ignore = "captures live fixture, requires ZAI_API_KEY"]
async fn capture_z_ai_02_tool_call() {
    let bytes = send(&json!({
        "model": MODEL,
        "stream": true,
        "stream_options": {"include_usage": true},
        "tools": [{
            "type": "function",
            "function": {
                "name": "get_weather",
                "description": "Get the current weather for a city.",
                "parameters": {
                    "type": "object",
                    "properties": {"city": {"type": "string"}},
                    "required": ["city"],
                },
            },
        }],
        "tool_choice": "auto",
        "messages": [{"role": "user", "content": "What's the weather in Tokyo? Use the tool."}],
    }))
    .await;
    write_fixture("z_ai", "02_tool_call", &bytes);
}

async fn send(body: &Value) -> Vec<u8> {
    let key = require_env("ZAI_API_KEY");
    let auth = format!("Bearer {key}");
    let client = reqwest::Client::new();
    post_json_capture(&client, URL, &[("authorization", auth.as_str())], body).await
}
