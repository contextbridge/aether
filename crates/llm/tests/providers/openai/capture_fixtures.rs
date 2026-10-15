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

use crate::providers::common::{MINIMAL_PROMPT, chat_body, chat_tool_body, post_bearer_capture, write_fixture};

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
    write_fixture("openai", "01_minimal", &send(CHAT_URL, &chat_body(MODEL, MINIMAL_PROMPT)).await);
}

#[tokio::test]
#[ignore = "captures live fixture, requires OPENAI_API_KEY"]
async fn capture_openai_02_tool_call() {
    write_fixture("openai", "02_tool_call", &send(CHAT_URL, &chat_tool_body(MODEL)).await);
}

#[tokio::test]
#[ignore = "captures live fixture, requires OPENAI_API_KEY"]
async fn capture_openai_03_reasoning() {
    let mut body = chat_body(REASONING_MODEL, REASONING_PROMPT);
    body["reasoning_effort"] = json!("medium");
    write_fixture("openai", "03_reasoning", &send(CHAT_URL, &body).await);
}

#[tokio::test]
#[ignore = "captures live fixture, requires OPENAI_API_KEY"]
async fn capture_openai_responses_01_minimal() {
    let body = json!({ "model": REASONING_MODEL, "stream": true, "input": MINIMAL_PROMPT });
    write_fixture("openai_responses", "01_minimal", &send(RESPONSES_URL, &body).await);
}

#[tokio::test]
#[ignore = "captures live fixture, requires OPENAI_API_KEY"]
async fn capture_openai_responses_02_reasoning() {
    let body = json!({
        "model": REASONING_MODEL,
        "stream": true,
        "reasoning": {"effort": "medium"},
        "input": REASONING_PROMPT,
    });
    write_fixture("openai_responses", "02_reasoning", &send(RESPONSES_URL, &body).await);
}

async fn send(url: &str, body: &Value) -> Vec<u8> {
    post_bearer_capture(url, "OPENAI_API_KEY", body).await
}
