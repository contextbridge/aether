use crate::providers::common::{MINIMAL_PROMPT, chat_body, chat_tool_body, post_bearer_capture, write_fixture};
use serde_json::{Value, json};

const OPENAI: Endpoint = Endpoint {
    fixture_dir: "openai",
    url: "https://api.openai.com/v1/chat/completions",
    key_env: "OPENAI_API_KEY",
    model: "gpt-4o-mini",
};

const OPENAI_RESPONSES: Endpoint = Endpoint {
    fixture_dir: "openai_responses",
    url: "https://api.openai.com/v1/responses",
    key_env: "OPENAI_API_KEY",
    model: REASONING_MODEL,
};

const OPENROUTER: Endpoint = Endpoint {
    fixture_dir: "openrouter",
    url: "https://openrouter.ai/api/v1/chat/completions",
    key_env: "OPENROUTER_API_KEY",
    model: "anthropic/claude-haiku-4.5",
};

// Subscription / coding endpoint, matching `providers::generic::ZAI`.
const Z_AI: Endpoint = Endpoint {
    fixture_dir: "z_ai",
    url: "https://api.z.ai/api/coding/paas/v4/chat/completions",
    key_env: "ZAI_API_KEY",
    model: "GLM-4.6",
};

const REASONING_MODEL: &str = "gpt-5-mini";
const REASONING_PROMPT: &str = "A clock currently shows 3:15. Compute the exact angle in degrees \
     between the hour hand and the minute hand. Show your reasoning and \
     give the final answer.";

#[tokio::test]
#[ignore = "captures live fixtures, requires OPENAI_API_KEY"]
async fn capture_openai() {
    capture_chat_scenarios(&OPENAI, |_| {}).await;

    let mut body = chat_body(REASONING_MODEL, REASONING_PROMPT);
    body["reasoning_effort"] = json!("medium");
    OPENAI.capture("03_reasoning", &body).await;

    let minimal = json!({ "model": REASONING_MODEL, "stream": true, "input": MINIMAL_PROMPT });
    OPENAI_RESPONSES.capture("01_minimal", &minimal).await;

    let reasoning = json!({
        "model": REASONING_MODEL,
        "stream": true,
        "reasoning": {"effort": "medium"},
        "input": REASONING_PROMPT,
    });
    OPENAI_RESPONSES.capture("02_reasoning", &reasoning).await;
}

#[tokio::test]
#[ignore = "captures live fixtures, requires OPENROUTER_API_KEY"]
async fn capture_openrouter() {
    capture_chat_scenarios(&OPENROUTER, |body| body["usage"] = json!({"include": true})).await;
}

#[tokio::test]
#[ignore = "captures live fixtures, requires ZAI_API_KEY"]
async fn capture_z_ai() {
    capture_chat_scenarios(&Z_AI, |_| {}).await;
}

struct Endpoint {
    fixture_dir: &'static str,
    url: &'static str,
    key_env: &'static str,
    model: &'static str,
}

impl Endpoint {
    async fn capture(&self, scenario: &str, body: &Value) {
        write_fixture(self.fixture_dir, scenario, &post_bearer_capture(self.url, self.key_env, body).await);
    }
}

async fn capture_chat_scenarios(endpoint: &Endpoint, customize: impl Fn(&mut Value)) {
    for (scenario, mut body) in
        [("01_minimal", chat_body(endpoint.model, MINIMAL_PROMPT)), ("02_tool_call", chat_tool_body(endpoint.model))]
    {
        customize(&mut body);
        endpoint.capture(scenario, &body).await;
    }
}
