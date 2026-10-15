//! Re-capture committed `OpenRouter` fixtures by hitting `openrouter.ai`
//! directly.
//!
//! Each scenario is an `#[ignore]`'d test so the regular `cargo nextest run`
//! never spends money. To refresh, set `OPENROUTER_API_KEY` and run:
//!
//! ```sh
//! cargo nextest run --run-ignored only -E 'test(capture_openrouter)'
//! ```

use serde_json::{Value, json};

use crate::providers::common::{MINIMAL_PROMPT, chat_body, chat_tool_body, post_bearer_capture, write_fixture};

const MODEL: &str = "anthropic/claude-haiku-4.5";
const URL: &str = "https://openrouter.ai/api/v1/chat/completions";

#[tokio::test]
#[ignore = "captures live fixture, requires OPENROUTER_API_KEY"]
async fn capture_openrouter_01_minimal() {
    write_fixture("openrouter", "01_minimal", &send(chat_body(MODEL, MINIMAL_PROMPT)).await);
}

#[tokio::test]
#[ignore = "captures live fixture, requires OPENROUTER_API_KEY"]
async fn capture_openrouter_02_tool_call() {
    write_fixture("openrouter", "02_tool_call", &send(chat_tool_body(MODEL)).await);
}

async fn send(mut body: Value) -> Vec<u8> {
    body["usage"] = json!({"include": true});
    post_bearer_capture(URL, "OPENROUTER_API_KEY", &body).await
}
