//! Re-capture committed `OpenAI` fixtures (Chat Completions and Responses)
//! by hitting `api.openai.com` directly.
//!
//! Each scenario is an `#[ignore]`'d test so the regular `cargo nextest run`
//! never spends money. To refresh, set `OPENAI_API_KEY` and run:
//!
//! ```sh
//! cargo nextest run --run-ignored only -E 'test(capture_openai)'
//! ```

use serde_json::{Value, json};

use crate::providers::common::{post_json_capture, require_env, write_fixture};

const MODEL: &str = "gpt-4o-mini";
const REASONING_MODEL: &str = "gpt-5-mini";
const CHAT_URL: &str = "https://api.openai.com/v1/chat/completions";
const RESPONSES_URL: &str = "https://api.openai.com/v1/responses";

const REASONING_PROMPT: &str = "A clock currently shows 3:15. Compute the exact angle in degrees \
     between the hour hand and the minute hand. Show your reasoning and \
     give the final answer.";

#[tokio::test]
#[ignore = "captures live fixture, requires OPENAI_API_KEY"]
async fn capture_openai_01_minimal() {
    let bytes = send(
        CHAT_URL,
        &json!({
            "model": MODEL,
            "stream": true,
            "stream_options": {"include_usage": true},
            "messages": [{"role": "user", "content": "Reply with exactly: hi"}],
        }),
    )
    .await;
    write_fixture("openai", "01_minimal", &bytes);
}

#[tokio::test]
#[ignore = "captures live fixture, requires OPENAI_API_KEY"]
async fn capture_openai_02_tool_call() {
    let bytes = send(
        CHAT_URL,
        &json!({
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
        }),
    )
    .await;
    write_fixture("openai", "02_tool_call", &bytes);
}

#[tokio::test]
#[ignore = "captures live fixture, requires OPENAI_API_KEY"]
async fn capture_openai_03_reasoning() {
    let bytes = send(
        CHAT_URL,
        &json!({
            "model": REASONING_MODEL,
            "stream": true,
            "stream_options": {"include_usage": true},
            "reasoning_effort": "medium",
            "messages": [{"role": "user", "content": REASONING_PROMPT}],
        }),
    )
    .await;
    write_fixture("openai", "03_reasoning", &bytes);
}

#[tokio::test]
#[ignore = "captures live fixture, requires OPENAI_API_KEY"]
async fn capture_openai_responses_01_minimal() {
    let bytes = send(
        RESPONSES_URL,
        &json!({
            "model": REASONING_MODEL,
            "stream": true,
            "input": "Reply with exactly: hi",
        }),
    )
    .await;
    write_fixture("openai_responses", "01_minimal", &bytes);
}

#[tokio::test]
#[ignore = "captures live fixture, requires OPENAI_API_KEY"]
async fn capture_openai_responses_02_reasoning() {
    let bytes = send(
        RESPONSES_URL,
        &json!({
            "model": REASONING_MODEL,
            "stream": true,
            "reasoning": {"effort": "medium"},
            "input": REASONING_PROMPT,
        }),
    )
    .await;
    write_fixture("openai_responses", "02_reasoning", &bytes);
}

async fn send(url: &str, body: &Value) -> Vec<u8> {
    let key = require_env("OPENAI_API_KEY");
    let auth = format!("Bearer {key}");
    let client = reqwest::Client::new();
    post_json_capture(&client, url, &[("authorization", auth.as_str())], body).await
}
