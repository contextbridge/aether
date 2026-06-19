<!-- START doctoc generated TOC please keep comment here to allow auto update -->
<!-- DON'T EDIT THIS SECTION, INSTEAD RE-RUN doctoc TO UPDATE -->

- [Provider Fixtures](#provider-fixtures)
  - [Why these exist](#why-these-exist)
  - [Refreshing fixtures](#refreshing-fixtures)
  - [Required env vars](#required-env-vars)
  - [Scenario matrix](#scenario-matrix)
  - [Adding a new scenario](#adding-a-new-scenario)
  - [Notes on Bedrock](#notes-on-bedrock)

<!-- END doctoc generated TOC please keep comment here to allow auto update -->

# Provider Fixtures

Raw byte-for-byte SSE responses captured from each provider's streaming
endpoint. Used by `tests/providers/*/fixture_tests.rs` to verify that the real
`process_*_stream` parsers extract `TokenUsage` correctly from on-the-wire
responses.

## Why these exist

Hand-written tests check our *mapping* (e.g. `cached_tokens` → `cache_read_tokens`)
but not our *deserialization*. If a provider quietly renames a JSON field, a
hand-built struct test still passes and the field silently becomes `None` in
production. Fixture tests parse real provider bytes through the real parser, so
field renames and shape drift fail loudly.

Fixture tests assert *structural* properties (presence, `> 0`) rather than
exact token counts, so re-capturing fixtures does not require re-baselining
test assertions.

## Refreshing fixtures

The capture step lives in `#[ignore]`'d tests under
`tests/providers/{provider}/capture_fixtures.rs`. They're skipped on a normal
`cargo nextest run` so CI never spends money; refresh by opting in:

```sh
# Single provider, all scenarios:
ANTHROPIC_API_KEY=sk-ant-... \
  cargo nextest run -p aether-llm --run-ignored only -E 'test(capture_anthropic)'

# Single scenario:
OPENAI_API_KEY=sk-... \
  cargo nextest run -p aether-llm --run-ignored only -E 'test(capture_openai_02_tool_call)'

# Everything (requires every key):
ANTHROPIC_API_KEY=... OPENAI_API_KEY=... OPENROUTER_API_KEY=... ZAI_API_KEY=... \
  cargo nextest run -p aether-llm --run-ignored only -E 'test(capture_)'
```

The capture tests write raw SSE bodies to `tests/fixtures/{provider}/{scenario}.sse`.
Fixtures are committed to git so CI can run the parser tests without API keys.
CI must not run the capture tests themselves — they cost money and need credentials.

## Required env vars

| Provider           | Variable             |
| ------------------ | -------------------- |
| `anthropic`        | `ANTHROPIC_API_KEY`  |
| `openai`           | `OPENAI_API_KEY`     |
| `openai_responses` | `OPENAI_API_KEY`     |
| `openrouter`       | `OPENROUTER_API_KEY` |
| `z_ai`             | `ZAI_API_KEY`        |

## Scenario matrix

Each scenario exercises a different shape of the parsed `TokenUsage`. Not every
provider exposes every dimension; missing scenarios under a provider mean that
provider does not surface that field.

| Scenario          | What it tests                        | Anthropic | OpenAI | OpenAI Responses | OpenRouter | Z.ai |
| ----------------- | ------------------------------------ | :-------: | :----: | :--------------: | :--------: | :--: |
| `01_minimal`      | basic input/output token counts      |     ✓     |   ✓    |        ✓         |     ✓      |  ✓   |
| `02_tool_call`    | tool-call parsing + finish reason    |     ✓     |   ✓    |                  |     ✓      |  ✓   |
| `03_cache_write`  | `cache_creation_tokens` populated    |     ✓     |        |                  |            |      |
| `04_cache_read`   | `cache_read_tokens` populated        |     ✓     |        |                  |            |      |
| `03_reasoning`    | `reasoning_tokens` populated         |           |   ✓    |                  |            |      |
| `02_reasoning`    | `reasoning_tokens` populated         |           |        |        ✓         |            |      |
| `05_reasoning`    | reasoning content blocks (Anthropic) |     ✓     |        |                  |            |      |

The cache scenarios for Anthropic must be captured back-to-back: `03_cache_write`
populates the cache, `04_cache_read` re-sends the same prefix and should report
non-zero `cache_read_input_tokens`.

## Adding a new scenario

1. Add a new `#[tokio::test] #[ignore = "..."]` function to
   `tests/providers/{provider}/capture_fixtures.rs` that posts the request body
   and writes the result via `write_fixture(...)`.
2. Run the new test with `cargo nextest run -p aether-llm --run-ignored only -E 'test(<name>)'`.
3. Add a `#[tokio::test]` to the matching `tests/providers/{provider}/fixture_tests.rs`
   that asserts the structural property the scenario is meant to prove.

## Notes on Bedrock

Bedrock is not represented here. AWS SDK uses event-stream binary framing, not
SSE, and the existing builder-based unit tests in `bedrock/streaming.rs` already
exercise the parser through stable typed-event constructors. Adding fixtures
would require re-implementing the AWS event-stream decoder in tests for marginal
benefit.
