# Issue #280 — Upgrade to the 2026-07-28 MCP spec

## Overview

### Problem statement
Aether currently depends on `rmcp` **1.7.0**, which implements the `2025-11-25` MCP
specification. The MCP spec undergoes its largest revision yet on **2026-07-28** (stateless
core, MRTR, routable headers, OAuth hardening, full JSON-Schema 2020-12, etc.). The Rust SDK
implements it in the new **`rmcp` 3.0.0-beta.2** line (3.0.0 stable lands on 2026-07-28).
We must: (1) upgrade `rmcp`, (2) address every breaking change, and (3) refactor to benefit
from the new additions — without changing the behaviour users and external MCP clients/servers
rely on.

This is effectively **two stacked migrations** because aether is skipping the `2.x` line:

1. **v1 → v2**: realignment of model types with the `2025-11-25` schema (mechanical renames +
   `#[non_exhaustive]` builders). The JSON wire format is unchanged; only the Rust API moves.
2. **v2 → v3**: the `2026-07-28` spec — removal of the `initialize` handshake, `server/discover`,
   MRTR, cache hints, routable headers, OAuth hardening, JSON-Schema 2020-12.

### Success criteria / acceptance conditions
- `rmcp` resolves to `3.0.0` (stable) — pinned to `=3.0.0-beta.2` while the beta is the latest,
  bumped to stable the day 3.0.0 ships.
- `just check`, `just lint`, `just fmt-check`, and `just test` are green across the workspace.
- Aether's own stdio MCP servers (coding, skills, tasks, subagents, survey, plan) still start and
  serve their tools; existing stdio integration tests pass.
- Aether can still connect to **remote** MCP servers over Streamable HTTP, including legacy
  `2025-11-25` servers (rmcp's new client lifecycle probes `server/discover` and falls back to
  `initialize` automatically).
- Elicitation (form + URL, incl. the `URL_ELICITATION_REQUIRED` `-32042` path) still works
  end-to-end against both local and remote servers.
- OAuth flow (stored credentials + fresh authorization) still works against a real provider.
- MRTR `InputRequiredResult` returned by a **remote** server is surfaced to the user through the
  existing elicitation channel and retried with `inputResponses` + echoed `requestState`.

---

## Technical Approach

### Target version & pinning
- Set `rmcp = "=3.0.0-beta.2"` in the workspace `Cargo.toml` (exact pin while it is a beta —
  the SDK maintainers explicitly warn that beta APIs may still change). The day **3.0.0** stable
  is published, relax the pin to `"3"`.
- MSRV check: `rmcp 3.0.0-beta.2` requires Rust **1.88**; aether's `rust-toolchain.toml` pins
  **1.97**, so no toolchain change is needed.

### Feature-flag changes
v3 reshapes some features. The workspace declaration stays `default-features = false`; update
per-crate `features = [...]` lists:

| v1 feature in use          | v3 status                                         |
|----------------------------|---------------------------------------------------|
| `client`                   | unchanged                                         |
| `server`                   | unchanged (now implies `schemars`)                |
| `elicitation`              | unchanged                                         |
| `macros`                   | unchanged                                         |
| `auth`                     | unchanged                                         |
| `transport-io`             | unchanged                                         |
| `transport-child-process`  | unchanged                                         |
| `transport-streamable-http-client-reqwest` | unchanged                              |
| **(new) `request-state`**  | **enable on the client crates** that call remote   |
|                            | servers, to support MRTR `requestState` round-trips |

Add `request-state` to `aether-cli` (the client that drives remote tool calls) — *not* to the
stdio server crates.

### High-level design decisions
1. **Client lifecycle**: adopt rmcp's new "modern" client lifecycle mode (probes
   `server/discover`, falls back to `initialize` for old servers). This is configured at
   `serve_client` call sites in `mcp-utils/src/client/connection.rs`; the 5 call sites collapse
   onto a single helper to keep the fallback logic in one place.
2. **MRTR handling (the "take advantage" win)**: the client tool-call loop in
   `aether-core/src/mcp/run_mcp_task.rs` already dispatches elicitation requests through an
   `mpsc` channel. Extend it so that when a remote tool returns `InputRequiredResult`, the
   `inputRequests` map is dispatched through the **same** channel and the original `tools/call`
   is retried with `inputResponses` + the echoed `requestState`. This reuses existing
   infrastructure rather than inventing a new consent path.
3. **List caching (SEP-2549)**: enable rmcp's built-in client-side TTL-honoring response cache
   so repeated `tools/list`/`resources/list` calls within `ttlMs` are served from cache. This is
   low-risk and removes redundant round-trips when connecting to remote servers repeatedly.
4. **Deliberate non-goal — MRTR for aether's *own* servers**: aether's servers run over **stdio
   / in-memory** to a single local client. The live, connection-bound elicitation they use today
   is simpler than MRTR and gains nothing from statelessness. Converting them to MRTR is
   complexity for no benefit and is recorded as a follow-up. (See Additional Notes.)
5. **Tasks extension (SEP-2663) is a non-issue**: aether's `tasks` crate exposes task management
   as ordinary **tools** (`tasks/task_create`, etc.), not the experimental MCP `tasks/*`
   JSON-RPC methods. A repo-wide search for `tasks/get|tasks/list|TaskStatusNotification` returns
   zero hits, so the Tasks-extension reshaping does not touch aether.
6. **Error code `-32002 → -32602` (SEP-2164) is a non-issue**: a grep for `-32002` /
   `RESOURCE_NOT_FOUND` finds zero matches; the only MCP error code aether matches on is
   `URL_ELICITATION_REQUIRED` (`-32042`), which is unchanged.

### Migration strategy
- **Bottom-up by crate**, starting from leaf dependencies (`acp-utils`, `aether-auth`) up through
  `mcp-utils`, `mcp-servers`, `aether-core`, `aether-cli`, and finally the `wisp` TUI.
- Bump the dependency and run `cargo check -p <crate>` after each crate so the compiler localizes
  the breakage. Most v1→v2 changes are mechanical renames the compiler will point at.
- Keep behaviour identical in Phase 1; layer the new-feature adoption (MRTR, caching) on top in
  Phase 2 behind the same public API.

---

## Implementation Steps

### Phase 0 — Preparation
1. Create a branch `chore/rmcp-3-mcp-2026-07-28`.
2. In the workspace `Cargo.toml`, change `rmcp = { version = "^1.7.0", ... }` to
   `rmcp = { version = "=3.0.0-beta.2", default-features = false }` and run `cargo update -p rmcp`.

### Phase 1 — Compile against rmcp 3 (breakage fix)
Apply the v1→v2 **type renames** and the v2→v3 **API changes** crate by crate. The compiler will
flag every site; the table in *Files to Modify/Create* lists them all.

3. **`acp-utils`** (`src/lib.rs`, `src/notifications.rs`, `src/testing.rs`): rename
   `CreateElicitationRequestParams → ElicitRequestParams`, `CreateElicitationResult → ElicitResult`,
   re-export the new names. (`ElicitationAction`, `ElicitationSchema` keep their names.)
4. **`aether-auth`** (`src/mcp/integration.rs`, `src/mcp/credential_store.rs`): update
   `AuthorizationManager` / `OAuthClientConfig` / `AuthClient` / `CredentialStore` imports to the
   v3 module paths. v3 already validates the discovered-metadata `issuer` (SEP-2468-ish) and binds
   DCR credentials to the issuing AS (SEP-2352) internally — verify `perform_oauth_flow` still
   compiles and that `register_client` / `configure_client` signatures are unchanged; pass the
   OpenID Connect `application_type` (SEP-837) via the new builder method if rmcp exposes one
   (stops servers defaulting CLI/desktop clients to `"web"` and rejecting `localhost` redirects).
5. **`mcp-utils`** — the heart of the migration:
   - `src/client/mcp_client.rs`: rename elicitation types; update the `ClientHandler` impl.
     `get_info()` returns `ClientInfo` — verify the `ClientCapabilities` builder
     (`.enable_elicitation()` + `FormElicitationCapability` / `UrlElicitationCapability`) still
     compiles. Update `cancel_result()` to use the `ElicitResult` builder (`::new()` + `with_*`,
     since the struct is now `#[non_exhaustive]`).
   - `src/client/connection.rs`: this is where the **stateless** change lands.
     - Replace the implicit-initialize `serve_client(...)` calls (5 sites: `reconnect_with_auth`,
       `connect_stdio`, the two `connect_http` branches, `serve_in_memory`) with the new
       client-lifecycle-aware equivalent, preferring a single private helper (e.g.
       `serve_client_modern(mcp_client, transport)`) that selects rmcp's modern mode
       (`server/discover` with `initialize` fallback).
     - Update `StreamableHttpClientTransport::{from_config, with_client}` usage if the v3
       signatures changed; the transport now sends `Mcp-Method`/`Mcp-Name` headers automatically
       — remove any manual header logic that duplicates them (none today; just confirm).
     - `McpServerConnection::from_parts` uses `client.peer_info()` to read server instructions —
       confirm the accessor still exists in the stateless model (it may be `None` until
       `server/discover` completes; guard accordingly).
   - `src/client/manager.rs`: `list_tools` / capabilities-building; rename types in the in-process
     `TestServer`.
   - `src/client/oauth_handler.rs`: `ElicitingOAuthHandler` — rename elicitation param/result
     types; `AETHER_OAUTH_ELICITATION_ID` stays.
   - `src/testing.rs`: `connect()` runs `serve_server` + `serve_client` concurrently to perform the
     (legacy) handshake. Under v3 the handshake is optional; keep the helper but switch the error
     types (`ClientInitializeError`/`ServerInitializeError` → their v3 equivalents, or whatever
     `serve_*` returns now).
   - `src/transport.rs`: `InMemoryTransport` implements `rmcp::transport::Transport`. The trait's
     `send`/`receive` signatures were already adjusted for cancel-safety in 2.1; confirm v3 compiles
     against the current `impl Future` form, updating return types if the trait changed.
6. **`mcp-servers`** — all six `ServerHandler` impls:
   - Update `#[tool_router]` / `#[tool_handler(router = self.tool_router)]` sites (compiler will
     flag macro-level changes if any): `coding/mod.rs`, `plan/server.rs`, `skills/server.rs`,
     `subagents/server.rs`, `survey/server.rs`, `tasks/server.rs`.
   - Update server-side elicitation calls (`elicit_permission` in `coding/mod.rs`, `ask_user` in
     `survey/server.rs`, plan review) from `CreateElicitationRequestParams` → `ElicitRequestParams`
     and `ElicitationSchema`/`EnumSchema` builders to their v3 forms.
   - `src/bin/stdio.rs`: `server.serve(stdio())` — confirm `ServerHandler::serve` + `stdio()`
     import paths; the stdio transport is unaffected by the stateless-HTTP changes.
   - `src/lib.rs` / `src/setup.rs`: `into_dyn()` → `Box<dyn DynService<RoleServer>>`; confirm
     `DynService`/`ServiceExt` re-exports still resolve.
7. **`aether-core`**:
   - `src/mcp/run_mcp_task.rs`: rename elicitation types; the `URL_ELICITATION_REQUIRED`
     (`-32042`) handling stays as-is. `ServerResult`, `PeerRequestOptions`, `Request`,
     `ServiceError` — update to v3 paths.
   - `src/mcp/tool_bridge.rs`: `CallToolRequestParams`/`CallToolResult`/`Content` →
     `ContentBlock`; note SEP-2106 lets `structuredContent` be any JSON value and tool schemas
     allow `oneOf`/`anyOf`/`$ref` — no code change required unless we want to exploit it.
   - `src/mcp/mcp_builder.rs`: `McpManager` init + `ClientCapabilities` builder.
   - `src/testing/fake_mcp.rs`: `FakeMcpServer` `#[tool_router]`/`#[tool_handler]` rename.
8. **`aether-cli`**: `src/acp/session_actor.rs` (elicitation types), `src/acp/fake_prompt_mcp.rs`
   (`ServerHandler` + `ErrorData`), `src/slash_commands.rs`. Add the `request-state` rmcp feature
   here (client that drives remote calls).
9. **`wisp`** (TUI): `src/components/elicitation_form.rs` (16 call sites), `conversation_screen.rs`,
   `components/app/mod.rs`, `src/test_helpers.rs`, `tests/app_tests/mcp_oauth_elicitation_tests.rs`
   — mechanical elicitation-type renames.

### Phase 2 — Take advantage of new additions
10. **Handle MRTR `InputRequiredResult` from remote servers** (`aether-core/src/mcp/run_mcp_task.rs`):
    after `handle.await_response()`, branch on the `ServerResult::CallToolResult` variant: if it
    carries an `InputRequiredResult`, iterate `inputRequests`, dispatch each through
    `McpClient::dispatch_elicitation` (reuse the existing channel + UI), collect the answers into
    `inputResponses`, and re-issue the original `tools/call` with the echoed `requestState`.
    Cap retries (e.g. 5) to avoid loops. Add a focused integration test using an in-memory fake
    server that returns `InputRequiredResult` on the first call and a normal result on retry.
11. **Enable the client-side response cache (SEP-2549)**: where the client is constructed in
    `mcp-utils/src/client/connection.rs`, opt into rmcp's TTL-honoring cache for
    `tools/list` / `resources/list` / `prompts/list` so re-listing within a server-advertised
    `ttlMs` is served from cache. Confirm list-refresh semantics (cache invalidation on
    reconnection) still match aether's expectation that tool lists are fresh per session.
12. **Wire the modern client lifecycle explicitly**: ensure the helper from step 5 selects the
    probe-`server/discover`-then-fallback mode so we benefit from `server/discover` against
    `2026-07-28` servers while keeping `2025-11-25` servers working.

### Phase 3 — Polish
13. Update any doc comments / `crates/mcp-utils/src/docs/*` examples that reference old type names.
14. Run `just fmt`, `just lint`, `just test`; fix everything.
15. When `rmcp 3.0.0` stable ships, relax the pin to `"3"`, update `Cargo.lock`, re-run the suite.

---

## Testing Plan

### Unit tests
- `mcp-utils/src/client/mcp_client.rs` elicitation-dispatch tests: update type names; assert
  `dispatch_elicitation` still returns `Cancel` on dropped sender/receiver and `Accept` on a
  successful response.
- `aether-core/src/mcp/run_mcp_task.rs`: extend with a unit test for
  `parse_required_url_elicitations` and a new test asserting an `InputRequiredResult` with empty
  `inputRequests` yields a sensible error (no infinite retry).
- `aether-auth/src/mcp/integration.rs`: `dedupe_query_params` tests stay green; add a test that
  `perform_oauth_flow` passes `application_type` (non-`web`) when rmcp exposes it.

### Integration tests (must update, then must pass)
- `crates/mcp-servers/tests/stdio_transport.rs` — stdio server boot + `list_all_tools`.
- `crates/mcp-servers/tests/common/mod.rs` — `TestClient::connect` (uses
  `mcp_utils::testing::connect`).
- `crates/aether-core/tests/mcp/url_elicitation_tests.rs` — URL elicitation server +
  `ErrorData`/`ErrorCode::URL_ELICITATION_REQUIRED`.
- `crates/aether-core/tests/mcp/oauth_tests.rs` — `StreamableHttpClientTransportConfig` against
  `FailingHttpEndpoint`.
- `crates/wisp/tests/app_tests/mcp_oauth_elicitation_tests.rs`.
- All `crates/mcp-servers/tests/*_e2e.rs` and `coding_mcp_tools.rs` / `plan_mcp.rs` /
  `skills_self_improvement.rs` / `plugins_mcp_*` / `test_bash.rs` / `test_read_*` /
  `test_web_fetch.rs`.

### New integration test
- **MRTR round-trip**: an in-memory fake remote server whose tool returns
  `InputRequiredResult { inputRequests, requestState }` on the first call and a normal
  `CallToolResult` on the retry. Assert the client surfaces the input request, retries with the
  echoed `requestState` + `inputResponses`, and returns the final content.

### Edge cases to verify
- Connecting to a **legacy `2025-11-25`** remote server still works (fallback to `initialize`).
- Connecting to a **`2026-07-28`** remote server works via `server/discover` (no session header).
- Stdio servers start with no behaviour change; tools list/call unchanged from a client's view.
- Form elicitation, URL elicitation, and the `-32042` URL-required path all still resolve.
- OAuth: stored-credential fast path and fresh-authorization flow both succeed against a real
  provider; `iss`/`application_type` hardening does not regress.
- Tool `structuredContent` that is a non-object JSON value (SEP-2106) round-trips.
- MRTR retry cap is honoured when a server keeps returning `InputRequiredResult`.

---

## Files to Modify/Create

| File | Change | Add/Modify/Remove |
|------|--------|-------------------|
| `Cargo.toml` (workspace) | `rmcp` `^1.7.0` → `=3.0.0-beta.2` (then `"3"`); keep `default-features = false` | Modify |
| `crates/acp-utils/Cargo.toml` | Verify feature list against v3 (`client`, `elicitation`) | Modify |
| `crates/aether-auth/Cargo.toml` | Verify `auth`, `client` features | Modify |
| `crates/aether-cli/Cargo.toml` | Add `request-state` feature; verify `client`, `elicitation`, `server`, `transport-streamable-http-client-reqwest` | Modify |
| `crates/aether-core/Cargo.toml` | Verify `client`, `elicitation`; `testing` → `rmcp/macros`, `rmcp/server` | Modify |
| `crates/mcp-servers/Cargo.toml` | Verify `elicitation`, `macros`, `server`; `stdio` → `rmcp/transport-io` | Modify |
| `crates/mcp-utils/Cargo.toml` | Verify `client`, `server`, `macros`, `transport-*`; add `request-state` | Modify |
| `crates/acp-utils/src/lib.rs` | Rename/re-export elicitation types (`ElicitRequestParams`, `ElicitResult`) | Modify |
| `crates/acp-utils/src/notifications.rs` | Rename elicitation types | Modify |
| `crates/acp-utils/src/testing.rs` | Rename elicitation types | Modify |
| `crates/aether-auth/src/mcp/integration.rs` | v3 auth import paths; pass `application_type` (SEP-837) if exposed | Modify |
| `crates/aether-auth/src/mcp/credential_store.rs` | `CredentialStore` trait v3 signatures | Modify |
| `crates/mcp-utils/src/client/mcp_client.rs` | `ClientHandler` impl; elicitation renames; `cancel_result()` builder | Modify |
| `crates/mcp-utils/src/client/connection.rs` | Modern client lifecycle helper; 5 `serve_client` sites; transport; cache enablement (Phase 2) | Modify |
| `crates/mcp-utils/src/client/manager.rs` | `ClientCapabilities` builder; `list_tools`; `TestServer` renames | Modify |
| `crates/mcp-utils/src/client/oauth_handler.rs` | Elicitation type renames | Modify |
| `crates/mcp-utils/src/testing.rs` | `connect()` + `ConnectError` for v3 `serve_*` return types | Modify |
| `crates/mcp-utils/src/transport.rs` | `Transport` impl against v3 trait signature | Modify |
| `crates/mcp-servers/src/bin/stdio.rs` | `serve` / `stdio()` import paths | Modify |
| `crates/mcp-servers/src/lib.rs`, `setup.rs` | `DynService`/`ServiceExt` re-exports | Modify |
| `crates/mcp-servers/src/coding/mod.rs` | `#[tool_router]`/`#[tool_handler]`; `elicit_permission` elicitation renames | Modify |
| `crates/mcp-servers/src/plan/server.rs` | `ServerHandler`; prompt types; elicitation renames | Modify |
| `crates/mcp-servers/src/skills/server.rs` | `ServerHandler`; `#[tool_router]`; renames | Modify |
| `crates/mcp-servers/src/subagents/server.rs` | `#[tool_router]`/`#[tool_handler]` | Modify |
| `crates/mcp-servers/src/survey/server.rs` | `ask_user` elicitation renames; macros | Modify |
| `crates/mcp-servers/src/tasks/server.rs` | `#[tool_router]`/`#[tool_handler]` (tools only — **not** the Tasks extension) | Modify |
| `crates/aether-core/src/mcp/run_mcp_task.rs` | Elicitation renames; **MRTR `InputRequiredResult` handling (Phase 2)** | Modify |
| `crates/aether-core/src/mcp/tool_bridge.rs` | `Content`→`ContentBlock`; `CallToolResult` v3 | Modify |
| `crates/aether-core/src/mcp/mcp_builder.rs` | `McpManager` init; capabilities builder | Modify |
| `crates/aether-core/src/testing/fake_mcp.rs` | `FakeMcpServer` macros/renames | Modify |
| `crates/aether-cli/src/acp/session_actor.rs` | Elicitation type renames | Modify |
| `crates/aether-cli/src/acp/fake_prompt_mcp.rs` | `ServerHandler`; `ErrorData` | Modify |
| `crates/aether-cli/src/slash_commands.rs` | Elicitation type renames | Modify |
| `crates/wisp/src/components/elicitation_form.rs` | Elicitation type renames (16 sites) | Modify |
| `crates/wisp/src/components/conversation_screen.rs` | Elicitation type renames | Modify |
| `crates/wisp/src/components/app/mod.rs` | Elicitation type renames | Modify |
| `crates/wisp/src/test_helpers.rs` | Elicitation type renames | Modify |
| `crates/wisp/tests/app_tests/mcp_oauth_elicitation_tests.rs` | Elicitation type renames | Modify |
| `crates/aether-core/tests/mcp/url_elicitation_tests.rs` | `ErrorData`/`ErrorCode`; `ServerHandler` | Modify |
| `crates/aether-core/tests/mcp/oauth_tests.rs` | `StreamableHttpClientTransportConfig` v3 | Modify |
| `crates/mcp-servers/tests/common/mod.rs` | `TestClient::connect` via `mcp_utils::testing::connect` | Modify |
| `crates/mcp-servers/tests/*.rs` (all e2e/tool tests) | Macro/type renames | Modify |
| `crates/aether-core/tests/mcp/mrtr_tests.rs` | **New** — MRTR `InputRequiredResult` round-trip test | Add |

---

## Additional Notes

### Documentation updates
- Update any examples in `crates/mcp-utils/src/docs/*` and README snippets that reference the old
  1.x type names.
- Add a short note to `CHANGELOG.md` (the rmcp 3 / 2026-07-28 upgrade) under the next release
  section; `release-plz` will fold it into the per-crate changelogs.

### Follow-up tasks that may be spawned
- **MRTR for aether's own servers**: only worth doing if/when aether serves its tools over HTTP
  (e.g. a remote/headless mode). Local stdio/in-memory servers gain nothing from statelessness.
- **MCP Apps extension (SEP-1865)** and **Tasks extension (SEP-2663)**: both are now first-class
  extensions. Aether's task management is implemented as tools today; a future ticket could
  evaluate whether exposing it via the Tasks extension adds value.
- **Trace-context propagation (SEP-414)**: aether already uses `tracing`; wiring
  `traceparent`/`tracestate`/`baggage` `_meta` through to MCP calls would give end-to-end traces.
- **Pin relaxation**: the day `rmcp 3.0.0` stable ships, move `=3.0.0-beta.2` → `"3"`.

### Open questions for the reviewer
1. **Beta vs. stable timing**: Today is 2026-07-27; the 3.0.0 stable is expected 2026-07-28. Do we
   land this on `=3.0.0-beta.2` immediately and bump to `"3"` the next day, or wait ~1 day for
   stable? **Recommendation:** start on the beta now so the bulk is ready, then flip the pin. The
   plan assumes this.
2. **MRTR retry cap & UX**: When a remote server returns `InputRequiredResult`, aether surfaces
   each input request through the existing elicitation UI and retries. Proposed cap is 5 round
   trips. Acceptable, or should it be configurable?
3. **Client response cache scope (SEP-2549)**: enable for `tools/list` only, or also
   `resources/list` / `prompts/list`? Recommendation: all three, since rmcp honours the server's
   `ttlMs`/`cacheScope` and invalidates on reconnect.
4. **`application_type` for OAuth (SEP-837)**: if rmcp's DCR builder exposes
   `application_type`, do we want to send `"native"` unconditionally for CLI/desktop, or make it
   configurable? Recommendation: send `"native"` (matches our CLI/desktop reality and avoids the
   `localhost`-redirect rejection the SEP fixes).
5. **Scope of "take advantage"**: This plan adopts MRTR handling on the **client** (so remote
   `2026-07-28` servers that use it work) and the response cache, but deliberately does **not**
   convert aether's own local servers to MRTR (no benefit over stdio). Is that the right line, or
   should we also migrate the local servers for forward-compatibility?
