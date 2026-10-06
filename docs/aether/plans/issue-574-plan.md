# Issue #574 — Axum HTTP MCP Gateway — Implementation Plan

## Overview

### Problem statement

Aether already has an MCP gateway / tool-aggregation layer used client-side for
dynamic tool discovery (`McpManager` + `ToolCatalog` + `McpSnapshot` in
`crates/mcp-utils`, orchestrated by `McpBuilder`/`McpHandle` in
`crates/aether-core/src/mcp`, exposed to `aether mcp` over a Unix socket via
`GatewayService`). There is no reusable **HTTP** gateway: no binary that
aggregates N upstream MCP servers and re-serves them as one MCP server over
HTTP.

We want a new, separate binary — an axum-based, **stateless** Streamable-HTTP
MCP gateway — that aggregates N upstream **HTTP** MCP servers (tools, prompts,
resources) and authenticates to upstreams with static `Authorization` headers /
API tokens. No human OAuth flow, no progressive/deferred tool discovery.

### Success / acceptance criteria

1. New binary (e.g. `aether-mcp-gateway`) runs an HTTP server exposing a single
   aggregated MCP endpoint (Streamable HTTP, e.g. `POST/GET/DELETE /mcp`).
2. Upstream servers are configured with the existing `McpConfig` JSON shape
   (`{ "servers": { "<name>": { "type": "http", "url": ..., "headers": {...} } } }`),
   N servers per file, multiple `--config` files merge (last wins, same as
   `McpConfig::from_json_files`).
3. All upstream tools are re-served namespaced as `<server>__<tool>`
   (existing `SERVERNAME_DELIMITER = "__"` convention), with descriptions,
   JSON input schemas, and annotations preserved.
4. All upstream prompts are re-served namespaced as `<server>__<prompt>`;
   `prompts/get` routes to the owning upstream (reuse `McpHandle::list_prompts`
   / `get_prompt`, which already do exactly this).
5. Upstream resources are aggregated too (new plumbing — see below; nothing
   aggregates resources today).
6. Upstream auth is static headers only (`Authorization`, `X-API-Key`, custom
   schemes preserved verbatim, `${VAR}` expansion via `Vars`); an explicit
   `Authorization` header already disables OAuth in `McpHttpConfig`, and the
   gateway passes **no** OAuth handler factory / credential store so `NeedsOAuth`
   can never block startup.
7. Stateless serving: no `Mcp-Session-Id` required; each request is served from
   the latest `McpSnapshot` (live tool-list refresh keeps working without
   restarts).
8. `just lint`, `just fmt`, `just test` (targeted + workspace check) pass; new
   unit + integration tests cover namespacing, routing, header forwarding, and
   stateless serving.

Out of scope (per issue): progressive/deferred tool discovery
(`_aether_list_servers`, Unix-socket `GatewayService`, `deferTools` semantics —
the gateway serves every tool directly).

---

## Technical Approach

### High-level architecture

```text
 MCP clients (agents, Inspector, aether CLI)
        │  Streamable HTTP (stateless), axum on /mcp
        ▼
 aether-mcp-gateway (new binary, new crate `crates/mcp-gateway`)
   ├─ clap CLI: --config <file>... | --config-json <json>..., --listen <addr>, --path /mcp
   ├─ McpBuilder::spawn() (reuse, aether-agent-core) → McpSession/McpHandle
   │     └─ McpManager connects to each upstream via
   │        StreamableHttpClientTransport (custom_headers incl. Authorization)
   ├─ HttpGatewayService (new ServerHandler, mirrors gateway_service.rs)
   │     ├─ list_tools  ← snapshot catalog (ALL tools, namespaced)
   │     ├─ call_tool   ← McpHandle::call(ModelVisible{namespaced}, …), drain stream to final result
   │     ├─ list_prompts/get_prompt ← McpHandle (already namespaced/routed)
   │     └─ list_resources/read_resource ← NEW manager/snapshot plumbing (mirror prompts)
   └─ axum Router mounting rmcp StreamableHttpService<S, NeverSessionManager>
```

### Key design decisions

1. **New crate `crates/mcp-gateway`, not a subcommand of `aether`.**
   The issue asks for "a separate binary". Workspace `members = ["crates/*"]`
   picks it up automatically. Package name `aether-mcp-gateway`
   (lib `mcp_gateway` + `[[bin]] aether-mcp-gateway`), following the
   `wisp` / `aether-lspd` pattern (`[package.metadata.dist] dist = false`
   initially — only `aether-agent-cli` ships via cargo-dist today; changing
   `dist-workspace.toml` is a deliberate follow-up, see Notes).
   Alternative considered (new `aether gateway` subcommand): rejected — the
   gateway has a different lifecycle (long-running server, no agent/TUI) and
   would drag axum + server-transport features into the CLI.

2. **Maximal reuse of the client stack; do not reimplement connecting.**
   Upstream connections, header handling (`custom_headers`, sensitive
   `Authorization`, `${VAR}` expansion), lifecycle negotiation
   (`protocol::client_lifecycle_mode`), tool-list refresh, and prompt
   aggregation already exist and are well tested. The gateway reuses them via
   `McpBuilder` (config parsing → `McpServer` → `spawn`) and `McpHandle`
   (snapshot/call/prompts). Only two genuinely new pieces:
   - an HTTP **server** `ServerHandler` (`HttpGatewayService`), and
   - **resource** aggregation plumbing (the one aggregation gap: tools and
     prompts are aggregated, resources are not — verified: no
     `list_resources`/`read_resource` anywhere in `ToolCatalog`,
     `McpSnapshot`, `McpHandle`, or `GatewayService`).

3. **Force everything model-visible; ignore `deferTools`.**
   After `into_servers()`, map every server with
   `McpServer::with_exposure(ToolExposure::ModelVisible)`. This simultaneously
   (a) implements "no progressive discovery" (the handler serves the
   `model_visible` partition, which is then everything) and (b) prevents
   `McpBuilder::spawn` from binding its Unix-socket deferred gateway
   (`spawn` only binds when some server `has_deferred_tools()`).

4. **HTTP-only upstreams in v1.**
   Filter `McpServer` to `McpTransport::Http`; fail startup with a clear error
   naming any `stdio`/`in-memory` entries. Rationale: the issue scopes to "N
   upstream http mcps"; stdio children have process-lifecycle implications for
   a server binary. (The code path stays open — lifting the filter later is a
   ~5-line change since `McpBuilder` already handles all transports.)

5. **Stateless serving via `NeverSessionManager`.**
   `rmcp` 3.5.0 (pinned by `Cargo.lock`) ships
   `transport::streamable_http_server::{StreamableHttpService,
   StreamableHttpServerConfig, session::never::NeverSessionManager}` behind
   features `transport-streamable-http-server` (+ `-session`) and
   `server-side-http` — none currently enabled in the workspace, so the new
   crate enables them locally. `NeverSessionManager` "disables sessions
   entirely (stateless mode)"; each request constructs a fresh
   `HttpGatewayService` from the factory closure capturing a cloned
   `McpHandle`, which reads the current snapshot — so upstream tool changes
   propagate without reconnects. Set
   `StreamableHttpServerConfig { legacy_session_mode: false, json_response:
   true, .. }` (protocol `2026-07-28` is stateless regardless per SEP-2567).
   **Gotcha:** `StreamableHttpServerConfig::allowed_hosts` defaults to
   loopback-only (DNS-rebinding protection) — the binary must forward its
   `--listen` host into `allowed_hosts` (plus always `localhost`,
   `127.0.0.1`).

6. **Namespacing: tools AND prompts as `<server>__<tool>`.**
   Reuse `client::naming::{create_namespaced_tool_name, split_on_server_name}`
   and the `server__tool` convention from `ToolCatalog`/`GatewayService`.
   Collisions inside one upstream are impossible (upstream names are unique);
   cross-upstream collisions are impossible by construction (server prefix).
   Descriptions/schemas/annotations pass through verbatim (mirror the
   conversion in `gateway_service.rs::tools()`).

7. **Resources: verbatim URIs, exact-match routing, first-wins.**
   Resource identities are URIs, not names, so prefixing would break them.
   `list_resources` returns the union of upstream resources (each annotated
   with the owning server in `annotations`/`_meta` is optional; simplest is to
   pass through verbatim). `resources/read` routes by exact URI match to the
   first advertising server; ambiguous/missing URIs return `ErrorData`
   (`invalid_params` / `method not found`-style). Document the limitation.

8. **No interactive auth surface.**
   `McpBuilder::with_oauth_handler_factory` is simply never called and no
   credential store is installed, so `connect_http` either connects with
   static headers or yields `Failed` (the `NeedsOAuth` branch requires a
   factory). Upstream servers that 401 without static headers fail fast at
   startup with the error in server statuses — desired behavior for a
   headless gateway. Servers requiring elicitation/sampling mid-call: the
   gateway has no user to prompt; `call_tool`'s MRTR/elicitation path will
   time out per `CallToolOptions.timeout` and surface as a tool error.
   Document this; do not build an elicitation bridge in v1.

### Patterns to employ

- `ServerHandler` impl mirroring `aether-core/src/mcp/gateway_service.rs`
  (same `list_tools`/`call_tool` shape, same `server__tool` split, same
  cancellation-guard discipline), but over **all** (model-visible) tools.
- Builder pattern for the binary config (`GatewayConfig` from clap args +
  `Vars`), matching `McpBuilder`'s chained style.
- Test-builder pattern + existing fakes (`mcp_utils::testing::FakeMcpServer`,
  `ElicitationScript`) for handler tests; axum header-capture test server
  pattern from `crates/mcp-utils/tests/http_headers.rs` for auth tests.
- `thiserror` enum for gateway errors (`enum GatewayError { Config(ParseError),
  Upstream(McpError), … }`) — never `anyhow`.

### Trade-offs / risks

| Decision | Pro | Con / mitigation |
|---|---|---|
| Depend on `aether-agent-core` for `McpBuilder` | Zero duplication of connect/refresh/snapshot logic | Pulls agent deps into the binary; acceptable for v1. If binary size/build time hurts, a follow-up can drive `McpManager` + `run_mcp_task` directly from `mcp-utils` |
| `NeverSessionManager` stateless | Matches issue scope; no session store to operate | No resumable SSE streams; clients must handle plain request/response + SSE stream per request (standard for stateless MCP) |
| First-wins resource URIs | Simple, predictable | Duplicate URIs across upstreams ambiguous → log warning at refresh; document |
| Single-shot snapshot per request | Always fresh, no locking across await | A tool list changing mid-pagination can shift pages; use small pages / `list_all_*` (client uses `list_all_tools` already) |

---

## Implementation Steps

### Step 0 — Crate scaffolding

Create `crates/mcp-gateway/Cargo.toml`:

```toml
[package]
name = "aether-mcp-gateway"
version = "0.1.0"
edition = "2024"
description = "Stateless HTTP MCP gateway aggregating upstream HTTP MCP servers"

[package.metadata.dist]
dist = false

[[bin]]
name = "aether-mcp-gateway"
path = "src/main.rs"

[lib]
name = "mcp_gateway"
path = "src/lib.rs"

[lints]
workspace = true

[dependencies]
aether-agent-core = { path = "../aether-core", version = "…" }  # McpBuilder/McpHandle
aether-mcp-utils = { path = "../mcp-utils", version = "…" }     # McpConfig, naming, call types
rmcp = { workspace = true, features = ["server", "transport-streamable-http-server", "transport-streamable-http-server-session"] }
axum = { workspace = true }
tower-service = "…"   # check rmcp's tower-service version; may need workspace pin
tokio = { workspace = true, features = ["rt", "rt-multi-thread", "macros", "signal", "net", "time", "sync"] }
clap = { workspace = true }
serde_json = { workspace = true }
tracing = { workspace = true }
tracing-subscriber = { workspace = true }
thiserror = { workspace = true }
utils = { package = "aether-utils", path = "../utils", version = "…" }  # Vars

[dev-dependencies]
# rmcp client transport for end-to-end test through real HTTP:
rmcp = { workspace = true, features = ["client", "transport-streamable-http-client-reqwest"] }
```

Verify `tower-service` version compatibility with rmcp 3.5.0's `tower` feature
(`dep:tower-service`) and axum 0.8 before finalizing; add to workspace deps if
missing.

`src/lib.rs` re-exports modules: `config`, `handler`, `server`, `error`.

### Step 1 — Resource aggregation plumbing in `mcp-utils` (the one gap)

Mirror the existing prompt plumbing 1:1 (see `manager.rs::list_prompts` /
`get_prompt` and `mcp_snapshot.rs::clients_with_prompts`):

1. `mcp_snapshot.rs`: add
   ```rust
   pub fn clients_with_resources(&self) -> Vec<(String, Arc<RunningService<RoleClient, McpClient>>)>
   ```
   filtering on `peer_info().capabilities.resources.is_some()`.
2. `manager.rs`: add
   ```rust
   pub async fn list_resources(&self) -> Result<Vec<rmcp::model::Resource>>;
   pub async fn read_resource(&self, uri: &str) -> Result<rmcp::model::ReadResourceResult>;
   ```
   `list_resources` fans out with `join_all` like `list_prompts`, passing
   resources through verbatim. `read_resource` finds the first connected
   client whose advertised resource list contains `uri` (cache per call —
   call `list_all_resources` on candidates; N is small), else
   `McpError::ResourceNotFound`-style error (add variant to `McpError`).
3. `mcp_handle.rs` (aether-core): add `list_resources` / `read_resource`
   delegating through the snapshot exactly like `list_prompts` / `get_prompt`
   do, with `McpHandleError::ResourceList { server, message }` /
   `ResourceRead { server, uri, message }` variants.
4. Unit tests in `mcp-utils` with two `FakeMcpServer`s: union listing,
   exact-URI routing, unknown-URI error. (Check whether `FakeMcpServer`
   supports resources; extend the fake minimally if not — prefer extending
   the existing fake over a bespoke mock.)

### Step 2 — `HttpGatewayService` (`crates/mcp-gateway/src/handler.rs`)

```rust
#[derive(Clone)]
pub struct HttpGatewayService { handle: McpHandle }

impl HttpGatewayService {
    pub fn new(handle: McpHandle) -> Self { … }

    fn tools(&self) -> Vec<Tool> {
        // snapshot.catalog().tools().model_visible, namespaced names already
        // (ToolCatalog namespaces at build: server__tool) → convert
        // ToolDefinition { name, description, parameters, annotations }
        // into rmcp Tool exactly like gateway_service.rs::tools(),
        // but over model_visible instead of deferred.
    }
}

impl ServerHandler for HttpGatewayService {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(
            ServerCapabilities::builder()
                .enable_tools().enable_prompts().enable_resources()
                .build(),
        ).with_server_info(Implementation::new("aether-mcp-gateway", env!("CARGO_PKG_VERSION")))
    }
    fn list_tools(…)      // Ok(ListToolsResult::with_all_items(self.tools()))
    async fn call_tool(…) // split_once("__") → ToolRoute::ModelVisible{namespaced}
                          // → self.handle.call(route, args, CallToolOptions{ timeout: 10min, meta, cancel })
                          // → drain ToolCallStream to terminal Complete/TaskComplete
                          //   (same select!/cancellation-guard loop as gateway_service.rs),
                          //   ignoring Progress/TaskCreated/TaskStatus (no SSE-per-event in stateless v1)
    async fn list_prompts(…) // self.handle.list_prompts() → ListPromptsResult
    async fn get_prompt(…)   // self.handle.get_prompt(name, args) → GetPromptResult
    async fn list_resources(…) // self.handle.list_resources()
    async fn call_resource(…)  // exact-uri read_resource  (check rmcp 3.5 ServerHandler
                               // method name: list_resources / read_resource)
}
```

Confirm exact `ServerHandler` method signatures for prompts/resources in rmcp
3.5.0 (`lsp_symbol` on the `rmcp` source or `cargo doc`) — they follow the
`list_prompts(request, context)` / `get_prompt(params, context)` shape used by
`GatewayService`.

Timeout: reuse 10-minute default from `GatewayService`; make it a
`--tool-timeout-secs` CLI flag (default 600).

### Step 3 — Server wiring (`src/server.rs` + `src/config.rs` + `src/main.rs`)

`config.rs` — `GatewayConfig`:

```rust
pub struct GatewayConfig {
    pub config_files: Vec<PathBuf>,   // --config (repeatable)
    pub config_json: Vec<String>,     // --config-json (repeatable)
    pub listen: SocketAddr,           // --listen, default 127.0.0.1:8080
    pub path: String,                 // --path, default "/mcp"
    pub tool_timeout: Duration,       // --tool-timeout-secs, default 600
}
```

Loading:

1. `McpConfig::from_json_files(&files)` + each `--config-json` via
   `McpConfig::from_json`, merge `servers` maps (same last-wins as builder).
2. `into_servers(&Vars::new()…)` — which `Vars`? `McpBuilder::new(root_dir)`
   seeds `WORKSPACE` + `AETHER_HOME`. For the gateway, seed `WORKSPACE` with
   cwd (document it) so `${WORKSPACE}`/`${AETHER_HOME}`/env expansion keeps
   working for header tokens like `"Authorization": "Bearer ${GATEWAY_TOKEN}"`.
   (Check `Vars` semantics — whether process env is included by default or
   must be added; follow `mcp_builder.rs::new`.)
3. Force `ToolExposure::ModelVisible` on every server; reject non-HTTP
   transports with `GatewayError::UnsupportedTransport { server, type }`.

`server.rs` — `serve(config)`:

1. `McpBuilder::new(cwd).with_servers(servers).spawn().await` → `McpSession`;
   `session.block_until_ready().await` (fail startup on `None`); spawn
   background task draining remaining `event_rx` into `tracing::` logs
   (server-status changes, auth failures). Keep `McpRuntime` alive for the
   process lifetime (hold in `main`, don't `split`).
2. Build rmcp service:
   ```rust
   let handle = session.handle().clone();
   let svc = StreamableHttpService::new(
       move || Ok(HttpGatewayService::new(handle.clone())),
       Arc::new(NeverSessionManager::default()),
       StreamableHttpServerConfig {
           legacy_session_mode: false,
           json_response: true,
           allowed_hosts: vec![listen_host, "localhost", "127.0.0.1", "::1"],
           ..Default::default()   // confirm Default exists; else construct fully
       },
   );
   ```
3. axum router (verify exact combinator against rmcp 3.5 docs — tower
   `Service<Request<Body>>` mounts via `route_service`/`nest_service`):
   ```rust
   let app = Router::new()
       .route_service(&path, svc)                       // POST/GET/DELETE /mcp
       .route("/healthz", get(|| async { "ok" }));
   ```
   Bind `tokio::net::TcpListener`, `axum::serve` with graceful shutdown on
   Ctrl-C / SIGTERM (follow `Ac
...[18379 chars truncated]