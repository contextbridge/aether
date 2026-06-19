//! Shared helpers for fixture-driven provider tests.
//!
//! Each provider's `fixture_tests.rs` loads a captured SSE body from
//! `tests/fixtures/{provider}/{scenario}.sse`, splits the SSE frames, feeds
//! them through the real `process_*_stream` parser, and asserts structural
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

#![allow(dead_code)]

use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use llm::{LlmResponse, TokenUsage};
use serde_json::Value;

/// Resolve `tests/fixtures/{provider}/{scenario}.sse` relative to the crate
/// manifest directory.
pub fn fixture_path(provider: &str, scenario: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures").join(provider).join(format!("{scenario}.sse"))
}

/// Read a committed fixture from disk. Panics if the fixture is missing — the
/// suite refuses to silently pass when its parser-regression net is gone. To
/// regenerate fixtures, run
/// `cargo nextest run --run-ignored only -E 'test(capture_{provider})'`.
pub fn read_fixture(provider: &str, scenario: &str) -> Vec<u8> {
    let path = fixture_path(provider, scenario);
    fs::read(&path).unwrap_or_else(|_| {
        panic!(
            "missing fixture {provider}/{scenario} at {} — \
             run `cargo nextest run --run-ignored only -E 'test(capture_{provider})'` to regenerate",
            path.display()
        )
    })
}

/// Parse an SSE body into the JSON payloads of `data:` lines, dropping
/// `[DONE]` sentinels, comments, and `event:` lines. Returns one entry per
/// `data:` frame in stream order.
pub fn parse_sse_data_lines(bytes: &[u8]) -> Vec<String> {
    let text = std::str::from_utf8(bytes).expect("fixture is utf-8");
    text.lines()
        .filter_map(|line| line.strip_prefix("data: ").or_else(|| line.strip_prefix("data:")))
        .map(str::trim)
        .filter(|s| !s.is_empty() && *s != "[DONE]")
        .map(str::to_string)
        .collect()
}

/// Find the first `LlmResponse::Usage` in a list of parsed events.
pub fn find_usage(events: &[LlmResponse]) -> Option<TokenUsage> {
    events.iter().find_map(|e| match e {
        LlmResponse::Usage { tokens } => Some(*tokens),
        _ => None,
    })
}

/// Assert the basic invariants every captured response should satisfy:
/// non-zero input tokens and non-zero output tokens.
pub fn assert_minimal_usage(usage: &TokenUsage, scenario: &str) {
    assert!(usage.input_tokens > 0, "{scenario}: input_tokens should be > 0");
    assert!(usage.output_tokens > 0, "{scenario}: output_tokens should be > 0");
}

/// Read an environment variable, panicking with a clear instruction if missing.
/// Used by `capture_fixtures.rs` tests to fail loudly when an API key is needed.
pub fn require_env(name: &'static str) -> String {
    env::var(name).unwrap_or_else(|_| panic!("{name} must be set to capture fixtures"))
}

/// `POST` a JSON body and return the raw response bytes. Panics on transport
/// errors and on any non-2xx status, with the response body included in the
/// panic message so capture failures are diagnosable from the test output.
pub async fn post_json_capture(client: &reqwest::Client, url: &str, headers: &[(&str, &str)], body: &Value) -> Vec<u8> {
    let mut req = client.post(url).header("content-type", "application/json").json(body);
    for (k, v) in headers {
        req = req.header(*k, *v);
    }
    let resp = req.send().await.expect("http request to provider failed");
    let status = resp.status();
    let bytes = resp.bytes().await.expect("reading response body failed").to_vec();
    assert!(status.is_success(), "provider returned {}: {}", status.as_u16(), String::from_utf8_lossy(&bytes));
    bytes
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

fn relative_to_manifest(path: &Path) -> PathBuf {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    path.strip_prefix(&manifest).map_or_else(|_| path.to_path_buf(), PathBuf::from)
}
