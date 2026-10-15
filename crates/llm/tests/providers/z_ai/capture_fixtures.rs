//! Re-capture committed Z.ai fixtures by hitting the Z.ai coding endpoint
//! directly.
//!
//! Each scenario is an `#[ignore]`'d test so the regular `cargo nextest run`
//! never spends money. To refresh, set `ZAI_API_KEY` and run:
//!
//! ```sh
//! cargo nextest run --run-ignored only -E 'test(capture_z_ai)'
//! ```

use serde_json::Value;

use crate::providers::common::{MINIMAL_PROMPT, chat_body, chat_tool_body, post_bearer_capture, write_fixture};

const MODEL: &str = "GLM-4.6";
// Subscription / coding endpoint, matching `providers::generic::ZAI`.
const URL: &str = "https://api.z.ai/api/coding/paas/v4/chat/completions";

#[tokio::test]
#[ignore = "captures live fixture, requires ZAI_API_KEY"]
async fn capture_z_ai_01_minimal() {
    write_fixture("z_ai", "01_minimal", &send(&chat_body(MODEL, MINIMAL_PROMPT)).await);
}

#[tokio::test]
#[ignore = "captures live fixture, requires ZAI_API_KEY"]
async fn capture_z_ai_02_tool_call() {
    write_fixture("z_ai", "02_tool_call", &send(&chat_tool_body(MODEL)).await);
}

async fn send(body: &Value) -> Vec<u8> {
    post_bearer_capture(URL, "ZAI_API_KEY", body).await
}
