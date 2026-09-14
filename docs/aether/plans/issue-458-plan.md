# Implementation Plan — Issue #458: Remote 1st-party MCP tool server over stateless HTTP

## Overview

### Problem statement

Today the harness (agent loop) and all MCP tool servers run in the same process /
machine: built-in servers (`coding`, `skills`, `tasks`, `subagents`, `survey`,
`plan`) run in-memory, deferred tools are exposed over a Unix-domain socket
(`AETHER_MCP_IPC_SOCKET`), and `aether mcp` composes tools from `bash` through
that socket. There is no way to run the harness on one machine (laptop / EC2)
and execute tools on another machine (e.g. a Lambda MicroVM holding a git
checkout), even though Aether's "all tools come from MCP servers" architecture
is a natural fit for it (cf. https://www.anthropic.com/engineering/managed-agents).

### Goal

Ship a new crate + binary (e.g. `crates/aether-tool-server`, binary
`aether-tool-server`) that:

1. Aggregates 1st-party MCP servers (starting with `coding`) into a single MCP
   server and serves it over **stateless Streamable HTTP** (rmcp
   `StreamableHttpService` + `NeverSessionManager`), suitable for running in a
   MicroVM / sandbox.
2. Stays **stateless** by receiving per-request context (agent/stable id + tool
   filter) in the MCP request `_meta` field, applying it to `list_tools` and
   `call_tool` on every request.
3. Preserves **tool deferral/composition via `bash` + `aether mcp`** on the
   remote machine, and preserves per-agent **read-before-write** enforcement in
   the coding server.
4. Keeps `subagents`, `tasks`, and `survey` MCPs on the harness machine (they
   depend on local agent loops, session logs, and user elicitation).

The harness side already speaks Streamable HTTP as a client
(`mcp-utils::client::connection::connect_http`, `type: "http"` in `mcp.json`),
so no new client transport is needed — only per-request `_meta` plumbing and a
per-server `tools` filter in config.

### Success / acceptance criteria

- [ ] `aether-tool-server serve --port 8080 --root-dir <checkout>` runs in a
      sandbox with no harness/session state and serves `coding__*` tools over
      stateless HTTP (no `Mcp-Session-Id` required, `NeverSessionManager`).
- [ ] A harness configured with `"vm_tools": {"type": "http", "url": ...}`
      lists and calls remote tools through the existing HTTP client path.
- [ ] Per-request `_meta` carries `{agent_id, tool_filter}`; the server filters
      `list_tools` results and enforces the filter on `call_tool`; absent
      `_meta` means "no filtering" (backwards compatible).
- [ ] Two agents sharing one remote server have independent read-before-write
      tracking (agent A reading a file does not authorize agent B to write it).
- [ ] `bash` on the remote server can still compose other tools via
      `aether mcp` (remote-local Unix-socket gateway + `AETHER_MCP_IPC_SOCKET`
      in the Bash environment), honoring the calling agent's filtered view.
- [ ] `just check`, `just lint`, `just fmt` clean; new unit + integration tests
      pass (`just test`).

## Technical Approach

### Architectural decisions

1. **rmcp `StreamableHttpService` in stateless mode** for the server transport.
   rmcp 3.3.0 (pinned in `Cargo.lock`) ships
   `transport::streamable_http_server::{StreamableHttpService,
   StreamableHttpServerConfig, session::never::NeverSessionManager}`. The
   service is a Tower service mounted on an axum `Router` (axum 0.8.9 is
   already a workspace dependency). `service_factory: impl Fn() -> Result<S,
   _>` builds a **fresh aggregator per request**, which is exactly the
   statelessness we want: no cross-request server state; all per-request
   context comes from `_meta`. Constructor shape (verified in registry source):
   `StreamableHttpService::new(service_factory, Arc<session_manager>, config)`.
   Set `legacy_session_mode: false`, `json_response: true`, and loopback-only
   `allowed_hosts` by default (public/Lambda deployments sit behind their own
   ingress + auth; see "Out of scope").
2. **No new shared library crate.** The issue sketches extracting gateway code
   into a new lib crate, but `mcp-utils` already *is* the shared crate (depended
   on by `mcp-servers`, `aether-core`, and `aether-cli`). Put the shared
   `_meta` context helpers in a new `mcp-utils::tool_gateway::request_context`
   module instead of creating `crates/mcp-gateway`.
3. **Aggregator preserves `server__tool` names.** The remote service exposes
   aggregated tools under their existing namespaced names (`coding__read_file`,
   …) so harness-side catalog namespacing (`vm_tools__coding__read_file`) and
   the `server__tool` split logic keep working unchanged, and so the
   remote-local `aether mcp` view matches what engineers already know.
   MVP aggregates **coding only** behind a `--server coding` allowlist flag
   with an obvious extension point to add `skills` later; `subagents`/`tasks`/
   `survey` are deliberately excluded (harness-side, per the issue).
4. **Per-request context in `_meta`, client-side filtering as backstop.**
   Filtering is applied in two places:
   - *Server-side (enforcement + correct remote view):* decode
     `{agent_id, tool_filter}` from `RequestContext::meta` on every
     `list_tools`/`call_tool`.
   - *Harness-side (static, per-agent):* the existing `ToolFilter` flow
     (`AgentSpec.tools` → `McpBuilder::with_tool_filter` → catalog `allowed`)
     plus a new per-server `tools` field in `mcp.json` keeps working without
     `_meta` support (e.g. connect-time `list_tools` where rmcp may not let us
     attach meta — verify in Step 3; if attachable, send it there too).
5. **Read-before-write keyed by agent id.** Change `CodingMcp::files_read`
   from `RwLock<HashSet<String>>` to `RwLock<HashMap<String, HashSet<String>>>`
   keyed by the agent id decoded from `context.meta` (empty string when
   absent → today's behavior). This is the minimal change that keeps the rule
   sound when one server instance serves many agents.
6. **Remote-local composition reuses the Unix-socket gateway.** The remote
   binary also binds a `UnixSocketMcpTransport` serving the *same aggregator*
   (plus `_aether_list_servers`) and injects `AETHER_MCP_IPC_SOCKET` into the
   coding `BashEnvironment`. Then the `aether` CLI binary on the MicroVM image
   (`aether mcp ...` is transport-agnostic) works unchanged from remote `bash`.
   This avoids dragging `aether-core`'s `McpHandle`/`GatewayService` into the
   remote binary.

### Design patterns

- Tower service composition (`StreamableHttpService` inside an axum `Router`,
  following rmcp's own doc examples and the existing axum usages in
  `crates/llm/.../test_capture_server.rs`).
- Factory-per-request (`service_factory` closure cloning an `Arc<ServerConfig>`
  and building `CodingMcp` — matches the existing in-memory factory pattern in
  `mcp-servers/src/setup.rs`).
- Newtype-ish `_meta` contract module with `to_meta`/`from_meta` helpers,
  mirroring `aether-core::events::trace_context::TraceContext`.

### Key considerations / trade-offs

- **`RequestMetaObject` arbitrary keys:** `model/meta.rs` documents that wire
  `params._meta` is stripped into envelope extensions and moved into
  `RequestContext::meta` before dispatch, and that typed `meta` fields are
  honored on serialization. Custom keys ride alongside `progressToken` /
  `traceparent`. The exact map-insert API (`DerefMut` to `JsonObject`) must be
  confirmed against rmcp 3.3.0 source during Step 1 — if extension-level meta
  wins over typed params on conflict, our keys must not collide with rmcp
  well-known keys (namespace everything under `"aether"`).
- **`list_tools` with `_meta` from the harness client:** `call_tool` already
  supports meta via `CallToolOptions.meta` →
  `PeerRequestOptions::with_meta`. Whether `list_all_tools`/`list_tools` on the
  rmcp client accepts request options must be verified in Step 3. Fallback
  (acceptable for MVP): connect-time list is unfiltered; harness-side catalog
  filtering hides disallowed tools from the model, and the server enforces the
  filter on every `call_tool` (which always carries meta). Document the
  fallback if taken.
- **Elicitation / MRTR over stateless HTTP:** coding tools use permission
  elicitation (`PermissionMode`). Stateless + elicitation server→client
  round-trips may not survive without sessions. MVP: remote server runs with
  `PermissionMode::AlwaysAllow` (sandbox is already the trust boundary; the
  VM image, not prompts, is the security control) and documents that
  `AlwaysAsk` is unsupported remotely. `survey` (pure elicitation) stays
  harness-side per the issue.
- **Auth/TLS:** Lambda MicroVM ingress uses AWS IAM auth. Terminating SigV4 or
  mTLS is explicitly a follow-up (see Additional Notes). MVP binds
  `--host 127.0.0.1` by default, supports `--host 0.0.0.0` behind user-provided
  ingress, and takes an optional `--bearer-token` (constant-time compare in an
  axum middleware layer) as a stopgap. No secrets in logs.
- **Tool-name length:** double namespacing (`vm_tools__coding__read_file`) is
  verbose but consistent with today's `chrome-devtools` deferred naming; not a
  blocker.

## Implementation Steps

### Step 0 — Scaffolding: new `aether-tool-server` crate

Create `crates/aether-tool-server/` (`package.name = "aether-tool-server"`,
`[lib] name = "tool_server"` or binary-only crate; binary-only is fine and
simplest):

```toml
[dependencies]
tokio = { workspace = true, features = ["rt-multi-thread", "macros", "signal", "net"] }
axum = { workspace = true }
clap = { workspace = true, features = ["env"] }
serde / serde_json / tracing (workspace)
rmcp = { workspace = true, features = ["server", "transport-streamable-http-server"] }
mcp_servers = { package = "aether-mcp-servers", path = "../mcp-servers", features = ["coding"] }
mcp_utils = { package = "aether-mcp-utils", path = "../mcp-utils", default-features = false }
tower / http (workspace, for bearer middleware)
```

Binary `src/main.rs` with clap:

```
aether-tool-server serve --port 8080 --host 127.0.0.1 [--root-dir <dir>]
  [--rules-dir ...] [--server coding]... [--bearer-token-env VAR] [--ipc-socket <path>]
```

`serve` builds an `Arc<RemoteServerConfig>`, binds axum, handles `SIGTERM`/Ctrl-C
via `tokio::signal` → cancels rmcp `cancellation_token`. Workspace membership is
automatic (`members = ["crates/*"]`); keep `[package.metadata.dist] dist = false`
for MVP (packaging follow-up).

### Step 1 — Shared `_meta` request-context contract in `mcp-utils`

New module `crates/mcp-utils/src/tool_gateway/request_context.rs`:

```rust
pub const AETHER_META_NAMESPACE: &str = "aether";
pub const AGENT_ID_META_KEY: &str = "aether/agent_id";       // string
pub const TOOL_FILTER_META_KEY: &str = "aether/tool_filter"; // serialized ToolFilter

pub struct AgentRequestContext {
    pub agent_id: String,              // "" when absent
    pub tool_filter: Option<ToolFilter>,
}

impl AgentRequestContext {
    pub fn from_meta(meta: &RequestMetaObject) -> Self;  // total fn, never errors
    pub fn apply_to_meta(&self, meta: &mut RequestMetaObject);
}
```

- `from_meta`: read the two keys, `serde_json::from_value` the filter,
  `None` on any shape mismatch (log at debug, never fail the request).
- Re-export from `tool_gateway/mod.rs`. `ToolFilter` already derives
  `Serialize`/`Deserialize`, so no changes needed for the wire shape; the JSON
  matches the `mcp.json` `tools` field (name globs like `"coding__*"`,
  `{"readOnly": true}`), which is exactly the DX the issue sketches.
- Unit tests: round-trip, missing keys, malformed filter → `None`, unknown
  keys ignored.

### Step 2 — Per-agent read-before-write in `CodingMcp`

In `crates/mcp-servers/src/coding/mod.rs`:

- Change field `files_read: RwLock<HashSet<String>>` →
  `files_read: RwLock<HashMap<String, HashSet<String>>>` (key = agent id).
- Add helper `fn agent_id(context: &RequestContext<RoleServer>) -> String`
  using `AgentRequestContext::from_meta(&context.meta).agent_id`.
- Thread it through `read_and_track`, `ensure_read_before_overwrite`,
  `ensure_read_before_edit` (all call sites: `read_file`, `write_file`,
  `edit_file`, plus the line ~845 variants).
- Absent id → `String::new()` key: single-agent local behavior unchanged.
- Tests (public API only, via served `CodingMcp` + in-memory transport):
  agent A reads → agent A may write; agent B (different `_meta`) writing the
  same path gets `NotReadBeforeOverwrite`; no-meta callers share the legacy
  bucket. Write the failing test first per repo testing guidance.

### Step 3 — Harness side: per-server `tools` config + `_meta` emission

1. `crates/mcp-utils/src/client/config.rs`: add
   `#[serde(default)] pub tools: ToolFilter` to `StdioServerConfig`,
   `RemoteServerConfig`, `InMemoryServerConfig` (deny_unknown_fields stays;
   update `src/docs/mcp_server_config.md` + JSON-schema-visible docs).
2. Plumb per-server filters into the catalog: `McpManager` currently applies
   one global `tool_filter` in `register_connection`/`replace_catalog_tools`
   (`ServerCatalogEntry::from_tools(..., &filter)`). Intersect the global
   agent filter with the per-server `tools` filter when building that server's
   entry. Add `ToolFilter::intersect(&self, other: &Self) -> Self`
   (concat allow-lists = union semantics need care: intersection of two
   allow-lists = tools matching *either*? No — must match *both* filters'
   allow rules and *neither* deny. Implement as a paired struct or, simpler,
   evaluate both filters at `is_tool_allowed` time. Prefer: store
   `Vec<ToolFilter>` per entry / evaluate conjunction — avoids inventing
   cross-product matcher algebra.)
3. Emit `_meta` on outgoing calls: in `aether-core::core::agent`
   `handle_tool_completion`, merge `AgentRequestContext { agent_id:
   <session/agent key>, tool_filter: <effective filter for the target server>
   }` into the existing `TraceContext::to_meta()` map before building
   `CallToolOptions`. Agent identity source: `AgentKey::Default/Named` in
   `aether-cli` session code — pass a stable string down to the agent (new
   field on agent config/spec; default `"default"`). `GatewayService::call_tool`
   already forwards `request.meta` → needs no change (verify by test).
4. Verify whether rmcp client's list-tools path accepts `PeerRequestOptions`
   with meta; if yes, attach the same context on (re)list; if no, take the
   documented fallback (connect-time list + harness-side filtering + server
   enforcement on call).

### Step 4 — Remote aggregator service

In the new crate, `src/aggregator.rs`:

```rust
pub struct RemoteToolAggregator {
    config: Arc<RemoteServerConfig>,   // root dir, rules dirs, allowed servers
    inner: CodingMcp<DefaultCodingTools>,  // built per request by factory; add SkillsMcp later
}

impl ServerHandler for RemoteToolAggregator {
    fn get_info(&self) -> ServerInfo;   // name "aether-remote-tool-server"
    async fn list_tools(&self, _p, ctx) -> ...;  // decode _meta, filter, return server__tool names + annotations
    async fn call_tool(&self, req, ctx) -> ...;  // decode _meta, enforce filter (deny → ErrorData::invalid_params), dispatch by "server__tool" prefix, forward ctx.meta to inner call
}
```

- Filtering uses `ToolFilter::is_tool_allowed` against `llm::ToolDefinition`s
  converted from rmcp `Tool`s (mirror `connection.rs::From<rmcp::model::Tool>`).
- `call_tool` enforcement failure returns a standard MCP error (never a tool
  `is_error` result, so policy denials can't be mistaken for tool output).
- Structure the dispatch as `match server { "coding" => ..., _ => unknown }`
  so adding `skills` is a new arm + constructor field.

### Step 5 — HTTP + Unix-socket serving in the remote binary

- `src/http.rs`: build `StreamableHttpServerConfig { legacy_session_mode:
  false, json_response: true, cancellation_token, allowed_hosts:
  loopback-default, .. }`, `StreamableHttpService::new(factory,
  Arc::new(NeverSessionManager::new()), config)`, mount with
  `.route_service("/mcp", service)` on an axum `Router` plus a `GET /healthz`
  route; optional bearer-token middleware (`Authorization: Bearer …`,
  constant-time compare, `401` on mismatch — never log the token).
- `src/unix.rs` (or inline): bind `UnixSocketMcpTransport::bind(...)` serving
  a tiny `LocalGateway` (same aggregator + `_aether_list_servers` listing,
  reusing `LIST_SERVERS_TOOL` const), print/export the socket path, and set it
  as `AETHER_MCP_IPC_SOCKET` in the `BashEnvironment` handed to `CodingMcp`
  (same pattern as `mcp-servers/src/setup.rs::coding_bash_environment`).
  `aether mcp` from remote `bash` then works with zero CLI changes.
- Wire graceful shutdown: Unix-socket `Drop` cleanup + axum
  `with_graceful_shutdown`.

### Step 6 — Docs + `mcp.json` example

- New doc page (e.g. `crates/aether-tool-server/README.md`, following the
  `mcp-servers` README style): architecture diagram (harness ↔ HTTP ↔ sandbox),
  MicroVM run instructions, example harness `mcp.json` with the `vm_tools`
  `http` entry + `tools.allow` filter, `_meta` contract reference, and the
  "what stays local" list (subagents/tasks/survey).
- Update `mcp-utils/src/docs/mcp_server_config.md` for the new `tools` field.

### Step 7 — Test & harden

See Testing Plan. Gate: `just fmt && just check && just lint && just test`.

## Testing Plan

### Unit tests (new / extended)

- `mcp-utils::tool_gateway::request_context`: round-trip
  `AgentRequestContext ↔ RequestMetaObject`; missing keys → empty id/`None`
  filter; malformed filter JSON → `None` (request still served); coexistence
  with `progressToken`/`traceparent` keys (no clobbering).
- `ToolFilter` conjunction for per-server ∩ agent filters: allow/deny
  interactions incl. `{"readOnly": true}` + `"coding__*"` cases from the issue.
- `CodingMcp` per-agent read tracking (Step 2 tests above).
- Aggregator: `list_tools` filtered vs unfiltered; `call_tool` to denied tool
  → MCP error; unknown `server__tool` → `invalid_params`; `_meta` forwarded
  to inner coding tools (assert via read-tracking isolation test through the
  aggregator).

### Integration tests (new crate `tests/`)

- `http_stateless_e2e`: spawn `serve` on an ephemeral port (tokio test, no
  timeouts — use readiness polling on `/healthz`); connect with rmcp
  `StreamableHttpClientTransport` (same client the harness uses); assert
  `initialize` needs no session id, `list_tools` honors `_meta` filter,
  `call_tool` read→write works and cross-agent write is rejected.
- `bash_composition_e2e`: start binary with Unix socket enabled; set
  `AETHER_MCP_IPC_SOCKET`; run `aether mcp --help` / a deferred-style
  `server__tool` call through the socket; assert the filtered tool view
  matches the `_meta`-less vs filtered HTTP views as appropriate.
- Harness-side: extend `aether-core/tests/mcp/` with a fake-HTTP-server test
  (model on existing `oauth_tests.rs` StreamableHttp usage) asserting the
  agent attaches `aether/agent_id` + `aether/tool_filter` in call `_meta`
  (capture via `FakeMcpServer`-style `context_meta` capture).

### Edge cases to verify

- No `_meta` at all → full tool list, legacy shared read-bucket (back-compat
  with existing HTTP clients).
- `tools.allow: []` + empty deny (server-side) → meaning "allow all", matching
  `ToolFilter::apply` semantics (empty allow = all). An *explicit* deny-all
  must use `deny: ["*"]`-style glob — document it.
- Annotation matchers (`readOnly`) evaluate against converted rmcp annotations
  (title/read_only/destructive/idempotent/open_world) — same conversion as
  `connection.rs`.
- Concurrent requests with different agent ids (factory-per-request ⇒ no
  shared `CodingMcp` borrow issues; `files_read` map contention is a short
  `RwLock` critical section).
- Elicitation-dependent tools under `AlwaysAsk` remotely → document as
  unsupported; test that `AlwaysAllow` serves cleanly.
- Shutdown: in-flight calls drain on SIGTERM; socket file removed
  (`UnixSocketPath` Drop).

## Files to Modify/Create

| File | Change | Add / Modify / Remove |
|---|---|---|
| `docs/aether/plans/issue-458-plan.md` | This plan | Add |
| `crates/aether-tool-server/Cargo.toml` | New crate + `aether-tool-server` binary manifest (rmcp server + `transport-streamable-http-server`, axum, clap, mcp-servers/coding, mcp-utils) | Add |
| `crates/aether-tool-server/src/main.rs` | `serve` CLI (host/port/root-dir/rules-dir/servers/bearer-token-env/ipc-socket), signal handling | Add |
| `crates/aether-tool-server/src/aggregator.rs` | `RemoteToolAggregator` `ServerHandler`: `server__tool` dispatch, `_meta` filter + enforce, `get_info` | Add |
| `crates/aether-tool-server/src/http.rs` | axum router (`/mcp` + `/healthz`), `NeverSessionManager` stateless config, bearer middleware | Add |
| `crates/aether-tool-server/src/unix.rs` | Local Unix-socket gateway (aggregator + `_aether_list_servers`) for remote `aether mcp` composition | Add |
| `crates/aether-tool-server/tests/http_stateless_e2e.rs` | Ephemeral-port HTTP e2e (list/call/filter/agent isolation, no session) | Add |
| `crates/aether-tool-server/tests/bash_composition_e2e.rs` | Unix-socket `aether mcp` composition from remote bash env | Add |
| `crates/aether-tool-server/README.md` | Architecture, MicroVM runbook, `mcp.json` example, `_meta` contract | Add |
| `crates/mcp-utils/src/tool_gateway/request_context.rs` | `AgentRequestContext::{from_meta, apply_to_meta}` + key consts + unit tests | Add |
| `crates/mcp-utils/src/tool_gateway/mod.rs` | Re-export `request_context` | Modify |
| `crates/mcp-utils/src/client/config.rs` | Add `tools: ToolFilter` to stdio/remote/in-memory server configs | Modify |
| `crates/mcp-utils/src/client/manager.rs` (+ `tool_catalog.rs`) | Intersect per-server `tools` with session filter when building catalog entries | Modify |
| `crates/mcp-utils/src/client/call_tool.rs` (or snapshot resolve path) | Attach per-server filter context where `CallToolOptions.meta` is built, if not done at agent layer | Modify |
| `crates/mcp-utils/src/docs/mcp_server_config.md` | Document new `tools` field | Modify |
| `crates/mcp-servers/src/coding/mod.rs` | `files_read` keyed by agent id from `_meta`; thread through read/write/edit paths | Modify |
| `crates/mcp-servers/tests/` (coding tests) | Per-agent read-tracking tests (fail-first) | Modify |
| `crates/aether-core/src/core/agent.rs` | Merge `AgentRequestContext` (agent id + effective filter) into `CallToolOptions.meta` | Modify |
| `crates/aether-core/tests/mcp/` | Harness-emits-`_meta` test via fake HTTP server + `context_meta` capture | Add |
| `crates/aether-cli/...` (session `AgentKey` → agent spec) | Provide stable agent-id string into agent config (default `"default"`) | Modify |

## Additional Notes

- **Documentation updates:** README + `mcp_server_config.md` (above). Also add
  the `vm_tools` HTTP example to the repo-root `mcp.json` docs/comments if a
  suitable place exists; do not change default `mcp.json` behavior.
- **Follow-ups (out of scope for this plan):**
  - Real ingress auth (AWS SigV4 validation, ideally via `aether-auth`; mTLS)
    to replace the stopgap bearer token for Lambda MicroVM deployments.
  - TLS termination / cert management for the remote server.
  - `dist` packaging of `aether-tool-server` (`dist-workspace.toml` currently
    ships only `aether-agent-cli`) + MicroVM image definition containing the
    `aether` CLI (needed on `PATH` for `aether mcp` composition).
  - Aggregating more servers remotely (`skills`; explicitly *not* subagents /
    tasks / survey) and sub-agent routing (sub-agent MCPs stay harness-side
    and connect to the remote URL with the sub-agent's own id/filter).
  - Connection-level tool-list `_meta` if rmcp list APIs allow options (Step 3
    investigation outcome); session-scoped `initialize` if stateful features
    (elicitation/MRTR/tasks) are ever needed remotely.
  - Observability: trace-context already flows in `_meta`; ensure remote
    spans join the same trace.
- **Backwards compatibility:** all `_meta` additions are optional; absent keys
  reproduce today's behavior. The `tools` config field defaults to empty
  (allow-all). No changes to the `ToolMatcher` wire format.
