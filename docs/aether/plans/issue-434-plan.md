# Issue #434 — WebSocket API for OpenAI and Codex LLM providers

## Overview

### Problem statement

OpenAI released a WebSocket mode for the Responses API
(see <https://developers.openai.com/api/docs/guides/websocket-mode>).
Today the `llm` crate (`crates/llm`) speaks only HTTP/SSE (`POST {base}/responses`
with `Accept: text/event-stream`, decoded by `eventsource-stream`) for both
`OpenAiProvider` (`crates/llm/src/providers/openai/responses_provider.rs`) and
`CodexProvider` (`crates/llm/src/providers/codex/provider.rs`, feature `codex`).
Every agent turn re-sends the full context window, so long tool-call chains pay
full serialization + server-side reprocessing latency on each of hundreds of
round trips.

WebSocket mode keeps one persistent connection open and continues each turn with
only new input items plus `previous_response_id`, served from a connection-local
in-memory cache. OpenAI reports up to ~40% faster end-to-end execution on
rollouts with 20+ tool calls. The task asks for WebSocket support in both the
OpenAI and Codex providers, reusing the existing transport-agnostic
`StreamingModelProvider` trait (`crates/llm/src/provider.rs:37`), which yields
`LlmResponse` events over a boxed stream and does not care about transport.

Reference client: <https://github.com/openai/codex>.

### Success criteria / acceptance conditions

1. `OpenAiProvider` and `CodexProvider` can optionally communicate over a
   persistent WebSocket instead of per-turn HTTP/SSE, selected by an opt-in
   `websocket` flag on the provider connection override. Default behavior
   (flag absent/false) is byte-for-byte today's HTTP/SSE path.
2. The WebSocket path reuses the existing event pipeline unchanged:
   `ResponsesStreamEvent` deserialization and `process_response_stream`
   (`crates/llm/src/providers/openai_responses/streaming.rs`) map server events
   to the identical `LlmResponse` sequence (`Start → Text/Reasoning/ToolRequest*
   → Usage → Done`), so downstream agent code cannot tell the transport apart.
3. Continuation sends **incremental input only** (new items since the last turn
   on the same lane) with `previous_response_id` set. First turn, compaction,
   model/param changes, `previous_response_not_found`, and reconnects fall back
   to a full resend (`previous_response_id: null` + full input window).
4. Per-agent multiplexing: lanes keyed by the existing
   `Context::session_affinity_key` (`crates/llm/src/context.rs:23,46`), sent as
   the protocol `stream_id`. Agents without a key use the implicit default lane
   (`stream_id` omitted).
5. Codex handshake carries the same credentials as HTTP today (Bearer token +
   `chatgpt-account-id` + `originator: codex_cli_rs` + `version` headers).
6. `just test` (at minimum `cargo test -p aether-llm --all-features`) passes,
   including new unit tests (URL derivation, lane keys, incremental diff,
   envelope decode, error mapping) and new integration tests against a fake
   WebSocket server (incremental frames observed, compaction fallback,
   `previous_response_not_found` recovery, reconnect, Codex headers).
7. `just lint` and `just fmt` pass. No `anyhow`/`color-eyre`; errors use the
   existing `LlmError`/`ProviderError` enums. Public items go at the top of
   files; private helpers at the bottom.

---

## Technical Approach

### High-level architectural decisions

1. **One shared session module, two thin wirings.** Add
   `crates/llm/src/providers/openai_responses/websocket.rs` next to the existing
   `transport.rs`/`streaming.rs`/`mappers.rs` (registered in
   `openai_responses/mod.rs`). It owns: endpoint derivation, handshake headers,
   connection pooling, `response.create` envelope construction, incremental
   input diffing, WS-frame → `ResponsesStreamEvent` demultiplexing, and error
   mapping. `OpenAiProvider::stream_response` and
   `CodexProvider::stream_response` each gain a branch: if
   `connection.websocket` is set, delegate to the shared session; else run
   today's `send()`/`process_connection()` path untouched.
2. **Keep `StreamingModelProvider` unchanged.** It is object-safe and
   transport-agnostic by design (`stream_response(&Context) -> LlmResponseStream`).
   The WS session's per-turn future resolves to a `ResponsesEventStream`
   (`Pin<Box<dyn Stream<Item = Result<ResponsesStreamEvent>> + Send>>`, same
   alias as `transport.rs:12`), which feeds the existing
   `process_response_stream`. No trait changes, no downstream churn.
3. **Opt-in flag, default off.** Add `websocket: bool` (serde camelCase
   `websocket`, default false) to `ProviderConnectionOverride` and
   `ProviderConnectionConfig` (`crates/llm/src/provider_connection.rs`), plumbed
   through `from_override`/`merge`/constructors, the CLI `--provider` parser,
   and the settings JSON schema. This preserves backwards compatibility
   (including `deny_unknown_fields` consumers) and lets operators trial WS per
   provider (`{"openai": {"websocket": true}}`).
4. **One new workspace dependency: `tokio-tungstenite`.** No WS crate exists in
   the tree today (zero `tungstenite` hits in `Cargo.lock`). Add it to
   `[workspace.dependencies]` and reference `workspace = true` from
   `crates/llm/Cargo.toml`. Enable a rustls TLS feature consistent with the
   workspace `reqwest` (rustls + webpki roots) so `wss://` works without
   introducing native-tls/openssl. `futures`/`tokio`/`tokio-stream` are already
   present for sink/stream plumbing.
5. **Spike the endpoint first, especially Codex.** The OpenAI WS endpoint is
   documented as a persistent connection to the Responses resource with
   `response.create` client events. The Codex backend
   (`https://chatgpt.com/backend-api/codex`) is a reverse-proxied,
   header-sensitive API that may not expose WS at all. Step 1 below verifies
   reachability/handshake for both before building the session module; if Codex
   has no WS endpoint, wire WS for OpenAI only and keep Codex on HTTP behind the
   same flag (documented), rather than guessing a URL.

### Protocol mechanics (from OpenAI docs; verified 2026-09-06)

- **Connect:** persistent WS to the `/responses` resource. Derive by converting
  the provider base URL scheme `http(s) → ws(s)` and appending `/responses`
  (e.g. `https://api.openai.com/v1` → `wss://api.openai.com/v1/responses`;
  Codex base + `/responses`). Confirm exact path during the spike.
- **Client → server:** text frames of `response.create`. Payload mirrors the
  normal Responses create body **minus** transport fields (`stream`,
  `background` — implicit/unsupported over WS) **plus** WS-only
  `stream_id` (lane name, 1–256 chars `[A-Za-z0-9_.-]`, omit for the default
  lane). Because `async-openai`'s `CreateResponse` has no
  `stream_id`/`previous_response_id` fields, build the envelope from the
  existing `build_wire_request` JSON: strip `stream`/`background`, inject
  `stream_id` and `previous_response_id`.
- **Server → client:** envelope frames — either
  `{ "type": "message", "stream_id"?: string, "message": <Responses event> }`
  where the inner `message` is exactly the SSE `ResponsesStreamEvent` shape
  (`response.created`, `response.output_text.delta`, …, `response.completed`),
  or `{ "type": "error", "status": N, "stream_id"?: string,
  "error": { "type", "code", "message", "param" } }` for request-scoped errors.
  Demultiplex `message` frames by `stream_id` into the awaiting turn's event
  stream; surface `error` frames as `ProviderError`s.
- **Continuation:** each turn after the first sends `previous_response_id`
  (the prior response ID on that lane) + `input` containing **only new items**.
  The service holds recent response state in a connection-local in-memory cache
  per lane, so this path is fast and `store=false`/ZDR-compatible.
- **Cache misses:** unknown `previous_response_id` with `store=false` returns
  `previous_response_not_found` (a 400 `error` frame with
  `code: "previous_response_not_found"`). On miss: retry once on the same
  socket with `previous_response_id: null` (omitted) + the **full** input
  window. A same-lane 4xx/5xx evicts the cached ID, so the retry must be full.
- **Compaction:** `Context::with_compacted_summary` starts a **new chain** —
  send the compacted window as full `input` with `previous_response_id: null`.
  Never chain from a pre-compaction response ID. (Server-side
  `context_management` compaction, if ever enabled, would instead continue with
  the latest ID + new items only; standalone `/responses/compact` output becomes
  the new base input.)
- **Limits:** ≤16 in-flight responses per connection (extras queue FIFO per
  lane; same-`stream_id` requests never overlap, different lanes run
  concurrently); ≤32 distinct named `stream_id`s per connection (default lane
  exempt); connections live ≤60 minutes → reconnect and recover each lane via
  full-resend (for `store=false`) or `previous_response_id` replay (for
  `store=true`, not our default — we send `store: false`).

### Incremental input diffing

- The session keeps per-lane state: `last_input_items: Vec<Value>` (the exact
  serialized `input` array sent last turn), `last_response_id: Option<String>`,
  and a hash of the non-input request params (model, instructions, tools,
  reasoning effort/verbosity, prompt cache key, etc.).
- On each `stream_response`, build the **full** wire request with the existing
  `build_wire_request(model, context, policy)`, serialize `input` to
  `Vec<Value>`, and compare:
  - If lane state exists, non-input params hash matches, model matches, and the
    new `input` starts with the entire previous `input` as a prefix → send only
    the suffix (`input[sent_len..]`) with `previous_response_id`.
  - Otherwise (first turn, prefix mismatch from compaction/editing, param/model
    change, post-error eviction, reconnect) → send full `input` with
    `previous_response_id: null` and reset lane state on success.
- Prefix comparison on serialized JSON values is simple, deterministic, and
  correct for the append-only agent loop (assistant turn + tool results appended
  each round). Do not attempt item-level merging.

### Connection pooling and concurrency

- Pool keyed by `(provider_id, base_url_ws, auth_fingerprint, model)`, stored in
  a process-wide `tokio::sync::Mutex<HashMap<PoolKey, Arc<WsSession>>>`
  (or `OnceLock`). Each `WsSession` owns the write half (`SplitSink`) behind a
  `Mutex` and spawns one background reader task that routes incoming
  `message` frames to per-turn channels keyed by `stream_id` (default lane =
  empty key) and completes them at terminal events.
- `stream_response` flow: acquire/create session → compute
  full-vs-incremental envelope → send text frame → await routed events,
  converting each inner `message` JSON to `ResponsesStreamEvent` (reuse the
  serde enum; unknown types already decode to `Ignored`) → feed
  `process_response_stream` → return its `LlmResponse` stream. A turn ends at
  `response.completed`/`incomplete` (capture response ID for the lane) or at an
  `error`/transport failure (map and yield as `Err`, evict lane state).
- Reconnect: on transport failure or 60-minute expiry, drop the session from
  the pool, open a fresh connection, and retry the in-flight turn once as a
  full resend. Cap retries (open + one retry) and never loop silently.

### Error mapping (reuse `ProviderErrorKind`)

Map WS `error`-frame codes onto the existing taxonomy so retry behavior is
inherited:

| code | kind | retryable? |
|---|---|---|
| `previous_response_not_found` | internal trigger for one full-resend retry, not surfaced if retry succeeds | n/a |
| `rate_limit_exceeded` | `RateLimit` | yes |
| `server_error` / 5xx `status` | `Server` | yes |
| `invalid_stream_id`, `websocket_stream_limit_reached`, other 4xx validation | `Api` | no (surface; caller fixes lane usage) |
| `websocket_connection_limit_reached` (60-min) | reconnect + full-resend retry once | yes (once) |
| transport close / tungstenite error | `StreamInterrupted` | yes |
| 401/403 at handshake (esp. Codex token expiry) | `Authentication` (Codex: clear token cache as today) | no |

Follow `transport.rs::process_connection` precedent: attach HTTP-equivalent
metadata (`status`, `request_id` when present) to surfaced `ProviderError`s.

### Key technical considerations and trade-offs

- **Why `tokio-tungstenite`:** async-native, works with the existing multi-thread
  tokio runtime, and is the de-facto standard the `openai/codex` Rust client
  builds on. Alternative `async-tungstenite` adds no value here; raw
  `tungstenite` would need manual runtime bridging.
- **Why pool rather than per-call connections:** per-call WS setup would erase
  the latency win (handshake + cold server cache every turn). Pooling keeps the
  connection-local `previous_response_id` cache hot, which is the entire point.
  Cost: shared mutable session state and a background reader task — contained
  in the new module, invisible to providers.
- **Why prefix-diff on serialized items:** the agent loop is append-only, and
  the Responses mappers already produce a canonical item order. Structural
  diffing of `ChatMessage`s would duplicate mapper logic and risk divergence
  from what the server cached. Serialized prefix comparison compares exactly
  what was sent.
- **Codex uncertainty:** Codex auth (OAuth Bearer + `chatgpt-account-id`) and
  missing SSE content-type already forced custom transport once
  (`codex/provider.rs:68-72`). WS may need the same headers at handshake time
  (`tokio-tungstenite::connect_async` with a `Request` builder carrying them)
  and may not exist at all — hence the mandatory spike (Step 1) with a go/no-go
  for the Codex wiring (Step 7).
- **Schema ripple:** `ProviderConnectionOverride` derives `JsonSchema` and feeds
  `aether-project` settings (`aether_settings.rs`, `agent_config.rs`) plus the
  CLI parser. A new field requires updating all three surfaces plus generated
  schemas/docs, or `deny_unknown_fields` and help text drift.

---

## Implementation Steps

### Step 1 — Endpoint + protocol spike (go/no-go, esp. Codex)

- Using a scratch binary or `wscat`-equivalent against the real backends (or
  the `openai/codex` Rust client source as reference), verify:
  1. The exact WS URL for OpenAI (expected `wss://<base>/responses` derived
     from the REST base) and the handshake headers required (Bearer auth).
  2. Whether the Codex backend exposes a WS endpoint at all, and if so its URL
     and required handshake headers (Bearer + `chatgpt-account-id` +
     `originator` + `version`).
  3. The exact server envelope shapes (`message` vs `error` frames,
     `stream_id` presence on default-lane events, terminal event coverage).
- Record findings (URLs, headers, envelope samples) in the PR description.
- **Go/no-go:** if Codex has no WS endpoint, implement Steps 2–6 + OpenAI
  wiring only (Step 7 becomes: keep Codex on HTTP, document why), and note it
  in `docs/websocket.md`.

### Step 2 — Add the `tokio-tungstenite` dependency

- `Cargo.toml` `[workspace.dependencies]`: add e.g.
  `tokio-tungstenite = { version = "0.28", features = ["rustls-tls-webpki-roots"] }`
  (pin to the latest compatible at implementation time; the rustls feature must
  match the workspace `reqwest` rustls stack — verify with `cargo tree` that
  only one rustls version is unified).
- `crates/llm/Cargo.toml` `[dependencies]`: add
  `tokio-tungstenite = { workspace = true }`. No feature gate (WS is a
  runtime-selected transport, not a provider feature like `codex`/`bedrock`).
- Confirm `cargo check -p aether-llm --all-features` passes with no other
  changes.

### Step 3 — Add the opt-in `websocket` connection flag

- `crates/llm/src/provider_connection.rs`:
  - Add `pub websocket: bool` to `ProviderConnectionConfig` (default false) and
    `pub websocket: Option<bool>`-style opt-in field to
    `ProviderConnectionOverride` with
    `#[serde(default, skip_serializing_if = "Option::is_none")]` (camelCase
    name `websocket`, so JSON is `{"openai": {"websocket": true}}`).
  - Wire `from_override` (map `Some(true)` → true), `merge` (override wins when
    `Some`), and add a `pub fn websocket(bool)` constructor alongside
    `url`/`auth`/`request_model`.
  - Unit tests: deserialize `{"websocket": true}`, default-off, merge
    precedence, `config_for` propagation.
- `crates/llm/src/docs/provider_connection_override.md`: document the flag with
  a JSON example and a one-line note that it selects WebSocket mode for
  providers that support it (OpenAI, Codex if available), default off.
- `crates/aether-cli/src/provider_connection_args.rs`: accept
  `PROVIDER.websocket=true|false` (parse `"true"/"1"` → true,
  `"false"/"0"` → false, reject others), update the `value_name` help string
  and the field-must-be error message, add tests mirroring the existing
  `parses_provider_*` cases.
- Regenerate or update any checked-in settings JSON schemas that embed
  `ProviderConnectionOverride` (check `aether-project` schema outputs; the
  website `src/data/*.schema.json` files are gitignored build artifacts —
  regenerate via the repo's schema-gen step rather than hand-editing).

### Step 4 — Create the shared WebSocket session module

Create `crates/llm/src/providers/openai_responses/websocket.rs` and register
`pub(crate) mod websocket;` in `openai_responses/mod.rs`. Contents (public
items at top, private helpers at bottom, per repo style):

```rust
// Top: public surface used by the two providers.
pub(crate) struct WsSessionPool { /* Mutex<HashMap<PoolKey, Arc<WsSession>>> */ }
pub(crate) struct WsRequestParams { pub ws_url: String, pub headers: HeaderMap, pub provider: Provider, pub policy: &'static ResponsesRequestPolicy /* or owned fields */ }
pub(crate) async fn stream_via_websocket(params: WsRequestParams, model: &str, context: &Context) -> LlmResponseStream;

// Envelope types (serde):
// struct WsClientCreate { #[serde(flatten)] body: Value /* minus stream/background, plus stream_id/previous_response_id */ }
//   — implement as explicit struct: type: "response.create", stream_id: Option<String>,
//     previous_response_id: Option<String>, #[serde(flatten)] rest: Map<String, Value>.
// enum WsServerFrame { Message { stream_id: Option<String>, message: Value }, Error { status: Option<u16>, stream_id: Option<String>, error: WsErrorBody } }
//   with #[serde(tag = "type", rename_all = "lowercase")] i.e. "message"/"error".
```

Private machinery:

- `fn derive_ws_url(base_http_url: &str) -> Result<String>`: parse URL,
  map `http→ws`/`https→wss`, ensure path ends with `/responses` (append if the
  base is `…/v1` or the Codex root; do not double-append). Unit-test with
  OpenAI default, custom gateway, Codex base, trailing slashes.
- `fn lane_key(context: &Context) -> Option<String>`: return the sanitized
  `session_affinity_key` (validate `[A-Za-z0-9_.-]{1,256}`; on invalid, fall
  back to default lane `None` + `tracing::warn`). `None` → omit `stream_id`.
- `fn build_create_envelope(model, context, policy, lane, lane_state) -> (Value, bool /*is_incremental*/)`:
  call `build_wire_request`, strip `stream`/`background`, serialize `input`
  array; prefix-diff against lane's `last_input_items` + params-hash check;
  inject `stream_id` and `previous_response_id` (`null`/omitted for full sends).
  Pure function — unit-test matrix: first turn (full), append-only (incremental
  suffix), compaction (full, `previous_response_id: null`), model/param change
  (full), empty suffix edge (still send with empty `input`? prefer full resend
  to avoid a no-op turn — decide in code, test it).
- `WsSession`: `connect(url, headers)` via
  `tokio_tungstenite::connect_async(request)`; split sink/source; `Mutex<SplitSink>`
  for sends; `tokio::spawn` reader loop routing `Message::Text` frames:
  deserialize `WsServerFrame`; `Message` → parse inner `message` Value into
  `ResponsesStreamEvent` (invalid JSON → `ProviderError::stream_interrupted`,
  same message style as `transport.rs:53-55`; unknown `type` → existing
  `Ignored` variant handles it); forward to the lane's `mpsc::UnboundedSender`.
  `Error` frames → map codes per the table above (including the
  `previous_response_not_found` → full-resend-retry signal).
- Turn orchestration: send envelope → collect lane receiver into a
  `ResponsesEventStream` → `process_response_stream` → on terminal
  `response.completed` capture the response ID (from the `Created` event's
  `response.id`… note: lane's `previous_response_id` for the *next* turn is the
  latest completed response ID — track from `response.completed`'s
  `response.id` if present, else the created ID) and update lane state
  (`last_input_items` = full input sent-or-implied, params hash); on
  `previous_response_not_found` retry once with full input; on transport error
  reconnect once and retry as full resend.
- `From<tungstenite::Error> for LlmError` (new impl in `error.rs`):
  timeouts → `Timeout`, connection/IO → `Network`, protocol →
  `StreamInterrupted`. Keep the error-type discipline (no `anyhow`).

### Step 5 — Wire the OpenAI provider

- `crates/llm/src/providers/openai/responses_provider.rs`:
  - Store the connection flag: add `websocket: bool` field to `OpenAiProvider`,
    set from `ProviderConnectionConfig` in `provider_from_connection`.
  - In `stream_response`, after building URL/headers/model/request as today,
    branch: if `self.websocket`, derive the WS URL from the same base
    (`config.url("/responses")` → `derive_ws_url`), reuse `config.headers()`
    for the handshake, and call
    `stream_via_websocket(params, &model, context)` with
    `ResponsesRequestPolicy::openai()`; else the existing
    `send()`/`process_connection()` path unchanged.
  - Tests (in-file + integration): WS mode sends `response.create` without
    `stream`/`background`, includes `stream_id` when `session_affinity_key` is
    set, omits it otherwise; second turn with appended tool output sends only
    the suffix + `previous_response_id`; HTTP mode unchanged (existing tests
    keep passing unmodified).

### Step 6 — Extend the fake test server with WebSocket support

- `crates/llm/src/providers/test_capture_server.rs` (test-only):
  - Add a WS route (axum `WebSocketUpgrade`, already available via the
    dev-dependency `axum` with default features — verify `ws` is enabled; if
    not, enable it as a dev-dependency feature) alongside `/responses` that
    records received `response.create` envelopes (full parsed JSON per turn)
    and replays scripted server frames (hand-crafted `message` frames wrapping
    the existing `tests/fixtures/openai_responses/*.sse` event payloads, plus
    scripted `error` frames for `previous_response_not_found`).
  - Expose captured envelopes (`captured_ws()` returning `Vec<Value>` in order)
    so tests can assert incremental `input` + `previous_response_id`.
- This is the integration backbone for Steps 5/7: prefer asserting against
  captured state (received envelopes, response IDs chained) over counting mock
  calls, per repo testing guidance. Implement `Fake`-style in-memory behavior,
  no timeouts in tests (drive completion via terminal test frames).

### Step 7 — Wire the Codex provider (pending Step 1 go/no-go)

- `crates/llm/src/providers/codex/provider.rs`:
  - Add `websocket: bool` via `with_connection` (reads
    `ProviderConnectionConfig::websocket`).
  - In `stream_response`, branch like OpenAI but with
    `ResponsesRequestPolicy::codex()` and Codex handshake headers from
    `build_headers()` (Bearer + `chatgpt-account-id` + `originator` +
    `version`) passed to `connect_async` as WS handshake headers. On 401-class
    handshake failure, clear the token cache exactly as `send_request` does
    today.
  - Codex-specific tests: handshake headers observed by the fake WS server;
    `reasoning.effort: medium` default preserved in the WS envelope;
    `store: false` present; no `stream` field.
- If the Step 1 spike shows no Codex WS endpoint: skip the Codex branch (flag
  accepted but documented as OpenAI-only for now), and record the finding in
  `docs/websocket.md` + the PR.

### Step 8 — Docs, schemas, and changelog

- New `crates/llm/src/docs/websocket.md` (embedded via `include_str!` from the
  module docs of `websocket.rs` or `openai_responses/mod.rs`): when to enable
  it, the `previous_response_id` + incremental-input model, lane semantics,
  fallback cases, limits (16/32/60-min), and the Codex status from Step 1.
- Update `crates/llm/src/docs/providers.md` provider table notes if it lists
  transports, and the `provider_connection_override.md` (Step 3).
- Check website settings docs under `packages/website/src/content/docs/` for a
  providers/connection-override page that enumerates override fields; add
  `websocket` there if such a page exists.
- Add a `CHANGELOG.md` entry in `crates/llm/` per repo convention (check
  `release-plz` behavior — do not hand-edit generated release sections).

---

## Testing Plan

### Unit tests (in `crates/llm/src/...`, no network)

- `derive_ws_url`: `https://api.openai.com/v1` → `wss://api.openai.com/v1/responses`;
  custom `http://127.0.0.1:PORT` → `ws://…/responses`; Codex base;
  trailing-slash and already-suffixed inputs; invalid URL → `ProviderRequest` error.
- `lane_key`: valid affinity key passes through; missing → `None` (default lane);
  invalid chars/empty/overlong → `None` + warn (never send an invalid `stream_id`).
- `build_create_envelope`: first turn full + `previous_response_id: null`;
  append-only second turn incremental (`input` == suffix only,
  `previous_response_id` == lane ID); compaction/param-change/model-change →
  full; no `stream`/`background` keys ever; `store: false` preserved;
  `instructions`/`tools`/`reasoning` carried from `build_wire_request`.
- `WsServerFrame` deserialization: `message` frame unwraps to each
  `ResponsesStreamEvent` variant (reuse fixture payloads); `error` frame maps
  per the error table, incl. `previous_response_not_found`,
  `invalid_stream_id`, `websocket_stream_limit_reached`,
  `websocket_connection_limit_reached`.
- `ProviderConnectionOverride`: `websocket` serde round-trip, default-off,
  merge precedence (`#[test]`s next to the existing override tests).
- `From<tungstenite::Error>` mapping kinds.

### Integration tests (fake WS server, `tests/` + `test_capture_server.rs`)

- **Incremental chain:** two sequential `stream_response` calls sharing
  `session_affinity_key`; assert the second received envelope has
  `previous_response_id` == first response ID and `input` == only the new
  items; assert the consumer-visible `LlmResponse` sequence matches the HTTP
  path for the same fixture.
- **Compaction fallback:** `with_compacted_summary` between turns → full
  `input`, `previous_response_id: null`.
- **`previous_response_not_found` recovery:** server first replies with the
  400 error frame, client retries once with full input, stream completes; assert
  two envelopes received (incremental attempt + full retry).
- **Reconnect:** server drops the connection mid-stream; client reconnects and
  completes via full resend; surfaced events remain a valid
  `Start…Usage…Done` sequence.
- **Codex handshake:** fake server asserts `authorization`, `chatgpt-account-id`,
  `originator`, `version` headers on the WS upgrade; Codex 401 clears the token
  cache (reuse the existing `FakeOAuthCredentialStore` pattern).
- **Default-off:** without the flag, providers never attempt a WS upgrade
  (existing HTTP tests cover this; add an assertion that no WS route was hit).

### Edge cases to verify

- Empty-suffix turn (no new input): must not send an empty continuation that
  the server rejects — resend full or surface a clear error (test it).
- Concurrent lanes: two affinity keys on one pooled connection interleave
  correctly (frames routed by `stream_id`); same-lane turns are FIFO.
- 32-`stream_id` / 16 in-flight limits: excess lanes surface the server's
  `error` frame as non-retryable `Api` rather than hanging.
- 60-minute expiry: treated as reconnect + full-resend, not a fatal error.
- HTTP/SSE behavior unchanged: all existing fixture tests
  (`tests/providers/**`, `tests/fixtures/openai_responses/*.sse`) pass
  unmodified.

---

## Files to Modify/Create

| File | Changes | Add / Modify / Remove |
|---|---|---|
| `Cargo.toml` (workspace) | Add `tokio-tungstenite` to `[workspace.dependencies]` with a rustls feature matching the `reqwest` stack | Modify |
| `crates/llm/Cargo.toml` | Reference `tokio-tungstenite = { workspace = true }`; add `From<tungstenite::Error>`-compatible features as needed | Modify |
| `crates/llm/src/provider_connection.rs` | Add `websocket` flag to `ProviderConnectionOverride` + `ProviderConnectionConfig`; `from_override`, `merge`, `websocket()` constructor; unit tests | Modify |
| `crates/llm/src/docs/provider_connection_override.md` | Document `{"websocket": true}` with example | Modify |
| `crates/llm/src/providers/openai_responses/websocket.rs` | **New** shared session module: pool, URL derivation, lane keys, envelope builder + prefix-diff, frame routing, error mapping, `stream_via_websocket`; unit tests | Add |
| `crates/llm/src/providers/openai_responses/mod.rs` | Register `pub(crate) mod websocket;`, update module docs | Modify |
| `crates/llm/src/error.rs` | Add `From<tungstenite::Error> for LlmError` mapping; tests | Modify |
| `crates/llm/src/providers/openai/responses_provider.rs` | Store `websocket` flag; branch `stream_response` to WS path; tests | Modify |
| `crates/llm/src/providers/codex/provider.rs` | Store `websocket` flag via `with_connection`; branch `stream_response` with Codex headers/policy; 401 cache-clear on handshake; tests (gated on Step 1 spike) | Modify |
| `crates/llm/src/providers/test_capture_server.rs` | Add WS upgrade route, envelope capture, scripted frame replay (`captured_ws()`) | Modify |
| `crates/llm/src/docs/websocket.md` | **New** rustdoc page: usage, lanes, fallbacks, limits, Codex status | Add |
| `crates/aether-cli/src/provider_connection_args.rs` | Parse `PROVIDER.websocket=true\|false`; help text; tests | Modify |
| `packages/website/src/content/docs/**` settings page (if it enumerates override fields) | Document `websocket` | Modify (if applicable) |
| `crates/llm/tests/providers/openai/*` and codex-adjacent integration tests | **New** WS integration tests (incremental, compaction, error-recovery, reconnect, headers) | Add |
| `crates/llm/CHANGELOG.md` | Entry for WS support (follow repo release conventions) | Modify |

---

## Additional Notes

- **Documentation updates needed:** `provider_connection_override.md`,
  new `docs/websocket.md`, CLI `--provider` help string, website settings page
  if it lists override fields, regenerated JSON schemas for
  `ProviderConnectionOverride` (do not hand-edit gitignored generated schema
  artifacts — run the repo's schema-gen).
- **No trait or public-API breaks:** `StreamingModelProvider`,
  `ProviderFactory`, `LlmResponse`, and `Context` are untouched (only read via
  existing getters). The flag defaults keep every existing config valid.
- **Follow-up tasks that may be spawned:**
  1. Step 1 spike findings review (Codex go/no-go) — blocks Step 7.
  2. Evaluate enabling WS-by-default for OpenAI after soak-testing latency and
     `previous_response_not_found` rates in production-like agent runs.
  3. Server-side compaction (`context_management`) support if the product ever
     opts into it (protocol already accommodates it; client just keeps chaining
     the latest ID).
  4. Connection-pool observability (pool size, reconnect counts, incremental
     vs full-send ratios via `tracing` spans already used in providers).
  5. Bedrock Mantle is the third consumer of `openai_responses` but out of
     scope — the shared module should not regress its HTTP path (covered by
     existing `03_mantle_cache_write.sse` fixture tests).
- **Galleries of prior art consulted:** OpenAI WebSocket-mode guide + WS events
  reference (fetched 2026-09-06), the `openai/codex` repo (reference Rust WS
  client per the issue), and the in-tree `transport.rs`/`streaming.rs` split
  that this plan mirrors for WS.
