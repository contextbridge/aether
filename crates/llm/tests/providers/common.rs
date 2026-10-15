//! Shared helpers for fixture-driven provider tests.
//!
//! properties of the resulting `TokenUsage`. Fixtures are committed to git so
//! CI can run them without API keys; a missing fixture is a hard failure
//! (rather than a silent skip) so accidental deletions can't make the
//! parser-coverage net invisibly disappear.
//!
//! The companion `capture_fixtures.rs` files in each provider directory hold
//! `#[ignore]`'d tests that re-capture those fixtures by hitting the real
//! provider endpoints. They're skipped on `cargo nextest run`; refresh them
//! with `cargo nextest run --run-ignored only -E 'test(capture_)'` (and the
//! relevant API key in the environment).

use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::vec::IntoIter;

use llm::{LlmResponse, LlmResponseStream, TokenUsage};
use serde_json::{Value, json};
use tokio_stream::{Iter, StreamExt};

pub const MINIMAL_PROMPT: &str = "Reply with exactly: hi";
pub const TOOL_PROMPT: &str = "What's the weather in Tokyo? Use the tool.";

pub type FixtureData = Iter<IntoIter<llm::Result<String>>>;

pub async fn parse_fixture(
    provider: &str,
    scenario: &str,
    process: impl FnOnce(FixtureData) -> LlmResponseStream,
) -> Vec<LlmResponse> {
    let data: Vec<_> = parse_sse_data_lines(&read_fixture(provider, scenario)).into_iter().map(Ok).collect();
    process(tokio_stream::iter(data))
        .map(|event| event.unwrap_or_else(|e| panic!("{provider}/{scenario}: stream item errored: {e}")))
        .collect()
        .await
}

pub fn expect_usage(events: &[LlmResponse], label: &str) -> TokenUsage {
    let usage = events
        .iter()
        .find_map(|e| match e {
            LlmResponse::Usage { tokens } => Some(*tokens),
            _ => None,
        })
        .unwrap_or_else(|| panic!("{label}: usage event should be present"));
    assert!(!usage.input_tokens.is_zero(), "{label}: input_tokens should be > 0");
    assert!(!usage.output_tokens.is_zero(), "{label}: output_tokens should be > 0");
    usage
}

pub fn weather_tool(schema_key: &str) -> Value {
    let mut tool = json!({ "name": "get_weather", "description": "Get the current weather for a city." });
    tool[schema_key] = json!({
        "type": "object",
        "properties": {"city": {"type": "string"}},
        "required": ["city"],
    });
    tool
}

pub fn chat_body(model: &str, prompt: &str) -> Value {
    json!({
        "model": model,
        "stream": true,
        "stream_options": {"include_usage": true},
        "messages": [{"role": "user", "content": prompt}],
    })
}

pub fn chat_tool_body(model: &str) -> Value {
    let mut body = chat_body(model, TOOL_PROMPT);
    body["tools"] = json!([{ "type": "function", "function": weather_tool("parameters") }]);
    body["tool_choice"] = json!("auto");
    body
}

pub async fn post_bearer_capture(url: &str, key_env: &'static str, body: &Value) -> Vec<u8> {
    let auth = format!("Bearer {}", require_env(key_env));
    post_json_capture(url, &[("authorization", &auth)], body).await
}

/// `POST` a JSON body and return the raw response bytes. Panics on transport
/// errors and on any non-2xx status, with the response body included in the
/// panic message so capture failures are diagnosable from the test output.
pub async fn post_json_capture(url: &str, headers: &[(&str, &str)], body: &Value) -> Vec<u8> {
    let mut req = reqwest::Client::new().post(url).header("content-type", "application/json").json(body);
    for (k, v) in headers {
        req = req.header(*k, *v);
    }
    let resp = req.send().await.expect("http request to provider failed");
    let status = resp.status();
    let bytes = resp.bytes().await.expect("reading response body failed").to_vec();
    assert!(status.is_success(), "provider returned {}: {}", status.as_u16(), String::from_utf8_lossy(&bytes));
    bytes
}

pub fn require_env(name: &'static str) -> String {
    env::var(name).unwrap_or_else(|_| panic!("{name} must be set to capture fixtures"))
}

/// Write captured bytes to `tests/fixtures/{provider}/{scenario}.sse`,
/// creating the parent directory if needed.
pub fn write_fixture(provider: &str, scenario: &str, bytes: &[u8]) {
    let path = fixture_path(provider, scenario);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("create fixtures dir");
    }
    fs::write(&path, bytes).unwrap_or_else(|e| {
        panic!("write {}: {e}", path.display());
    });
    eprintln!("wrote {} ({} bytes)", relative_to_manifest(&path).display(), bytes.len());
}

/// Generate roughly `approx_tokens` tokens of lorem-ipsum filler. Used by the
/// Anthropic cache scenarios to comfortably exceed the smallest documented
/// per-model cache minimum (1024 tokens).
pub fn lorem_filler(approx_tokens: usize) -> String {
    let line = "Lorem ipsum dolor sit amet, consectetur adipiscing elit, sed do eiusmod tempor incididunt ut labore et dolore magna aliqua. ";
    let target_chars = approx_tokens * 4;
    let mut s = String::with_capacity(target_chars);
    while s.len() < target_chars {
        s.push_str(line);
    }
    s
}

fn fixture_path(provider: &str, scenario: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures").join(provider).join(format!("{scenario}.sse"))
}

fn read_fixture(provider: &str, scenario: &str) -> Vec<u8> {
    let path = fixture_path(provider, scenario);
    fs::read(&path).unwrap_or_else(|_| {
        panic!(
            "missing fixture {provider}/{scenario} at {} — \
             run `cargo nextest run --run-ignored only -E 'test(capture_{provider})'` to regenerate",
            path.display()
        )
    })
}

fn parse_sse_data_lines(bytes: &[u8]) -> Vec<String> {
    let text = std::str::from_utf8(bytes).expect("fixture is utf-8");
    text.lines()
        .filter_map(|line| line.strip_prefix("data: ").or_else(|| line.strip_prefix("data:")))
        .map(str::trim)
        .filter(|s| !s.is_empty() && *s != "[DONE]")
        .map(str::to_string)
        .collect()
}

fn relative_to_manifest(path: &Path) -> PathBuf {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    path.strip_prefix(&manifest).map_or_else(|_| path.to_path_buf(), PathBuf::from)
}
