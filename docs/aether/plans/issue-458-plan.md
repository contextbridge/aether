# Plan: Remote sandbox MCP binary (`aether mcp-serve`) — Issue #458

## Overview

### Problem statement

Today the harness (agent loop) and all first-party MCP servers (coding, skills,
tasks, subagents, survey, plan) run in one process via in-memory transports.
Following [Anthropic's Managed Agents](https://www.anthropic.com/engineering/managed-agents)
("decouple the brain from the hands"), we want to support running the
harness/agent loop **outside** a sandbox while a git repo checkout and tool
execution live **inside** the sandbox. Since every aether tool is already an MCP
server, the natural shape is:

- a new binary that aggregates aether's first-party MCP servers behind
  authenticated **streamable-HTTP** endpoints and runs inside the sandbox;
- the harness (unchanged agent loop) connects to it as an ordinary remote
  `type: "http"` MCP server.

The hard part is **not** the HTTP serving — it is the bash tool. Today bash
composes *any* MCP tool (including credentialed remotes like Linear/Sentry) via
the `aether mcp <server> <tool>` CLI → `AETHER_MCP_IPC_SOCKET` unix-socket
gateway → `GatewayService` → `McpHandle`. If the coding server moves into a
credential-less sandbox, that composition path must still work without leaking
tokens into the sandbox.

### Success / acceptance criteria

1. `aether mcp-serve --port <p> --root-dir <checkout> [--bearer-token ...]`
   (name TBD, see step 1) starts in a sandbox and exposes each first-party
   server on its own streamable-HTTP route
   (e.g. `POST /mcp/coding`, `/mcp/skills`, `/mcp/tasks`, `/mcp/plan`,
   `/mcp/survey`).
2. An unmodified harness connects to those routes using the existing
   `type: "http"` + `headers.Authorization` remote-server config and can
   list/call tools (round-trip integration test harness→sandbox).
3. Requests without (or with a wrong) bearer token are rejected with 401
   before reaching any tool.
4. Bash running **inside the sandbox** can still compose:
   - sibling first-party tools served by the same binary, and
   - credentialed remote tools (Linear/Sentry) whose tokens live
     **harness-side only**, via the chosen secure design (see Technical
     Approach — recommended: authenticated tool-gateway callback).
5. No OAuth client secrets, API tokens, or keyring/encrypted-file credential
   material is readable from inside the sandbox (verified by test: sandbox
   process env + filesystem contain only a short-lived session bearer token).
6. `dist` ships the new binary alongside `aether` (dist-workspace or docs
   updated accordingly).

---

## Technical Approach

### High-level architecture

```text
 OUTSIDE SANDBOX (trusted, holds all credentials)     INSIDE SANDBOX (untrusted code runs here)
 ─────────────────────────────────────────────────     ─────────────────────────────────────────
 harness (agent loop, unchanged)                      aether mcp-serve  (NEW binary)
   │  MCP client (exists:                              ┌─ /mcp/coding  → CodingMcp (root-dir = checkout)
   │  StreamableHttpClientTransport)                   ├─ /mcp/skills  → SkillsMcp
   │  + headers: Authorization: Bearer <sandbox-token> ├─ /mcp/tasks   → TasksMcp
   ├──────────────────────────────────────────────────┤─ /mcp/plan    → PlanMcp
   │                                                   └─ /mcp/survey  → SurveyMcp
   │  remote 3rd-party MCPs (Linear, Sentry)             bash tool inside CodingMcp composes via
   │  with OAuth/vault creds (exists)                    `aether mcp ...` → HTTP callback to the
   │                                                        harness-side tool gateway (see below)
   ▼
 harness-side tool gateway (NEW, small):
   axum route (e.g. /mcp-gateway) serving GatewayService-equivalent
   over StreamableHttpService + bearer auth (per-session token).
   Proxies deferred-tool calls to harness McpHandle, which fans out to
   Linear/Sentry with vault credentials. Sandbox never sees those tokens.
```

### Key decisions

1. **One route per server, not one merged server.** Each first-party
   `ServerHandler` impl (`CodingMcp`, `SkillsMcp`, …) already exists and is
   constructed in `crates/mcp-servers/src/setup.rs` (factories) and
   `crates/mcp-servers/src/bin/stdio.rs` (standalone). rmcp's
   `StreamableHttpService::new(service_factory, session_manager, config)`
   takes a `Fn() -> Result<S: ServerHandler>` factory, so each route gets its
   own service + `LocalSessionManager`. No multiplexing router to write, no
   tool-name collisions, and the harness's existing per-server connection
   model (`RuntimeMcpTransport::Http`, `connection.rs::connect_http`) works
   unchanged. (Verified: rmcp 3.3.0 in `Cargo.lock` ships
   `transport/streamable_http_server/{tower,session}` with `axum`-compatible
   tower `Service` impl + `LocalSessionManager`; usage pattern confirmed in
   rmcp's own `tests/test_custom_headers.rs`.)
2. **New binary lives in `aether-cli` (shipped by dist), reusing
   `mcp-servers` constructors.** `dist-workspace.toml` ships only
   `aether-agent-cli`, and `mcp-servers` has `dist = false`, so a
   `[[bin]]` in `mcp-servers` would not be released. Add
   `[[bin]] name = "aether-mcp-serve"` (final name decided in step 1; must not
   collide with the existing `aether mcp` subcommand) to
   `crates/aether-cli/Cargo.toml`, constructing servers exactly like
   `src/bin/stdio.rs` does. The sandbox still needs the `aether` binary (or
   equivalent minimal client) on `PATH` for the `aether mcp ...` composition
   path — decide in step 1 whether to ship the full `aether` binary into the
   sandbox image or add a `--gateway-url` mode to the existing `mcp_command.rs`
   (preferred: extend `mcp_command.rs`, no new client binary).
3. **Auth (sandbox side): shared-secret bearer token, not OAuth server.**
   The sandbox server validates `Authorization: Bearer <token>` in axum
   middleware (tower-http `ValidateRequest` or a 20-line axum middleware) and
   returns 401 otherwise. Token is minted per sandbox session harness-side and
   passed via env/flag (`--bearer-token` / `AETHER_MCP_BEARER_TOKEN`); support
   `--allow-unauthenticated` (loopback-only dev mode) explicitly, defaulting to
   required. Rationale: the sandbox is not an identity provider; full MCP
   OAuth *server* support in rmcp is client-oriented and would be large scope
   for zero benefit. Client side already supports this: `RemoteServerConfig`
   `headers.Authorization` with `Bearer` prefix stripped
   (`mcp-utils/src/client/config.rs::into_transport`).
4. **Credential-less composition (the core issue question): adopt option 1 —
   singular authenticated gateway.** Concretely:
   - Harness side exposes one authenticated HTTP endpoint that serves the
     **existing `GatewayService`** (`aether-core/src/mcp/gateway_service.rs`:
     deferred tools namespaced `server__tool` + `_aether_list_servers`) over
     `StreamableHttpService`. This is ~50 lines: it is already a
     `ServerHandler`.
   - Sandbox side sets `AETHER_MCP_GATEWAY_URL` + `AETHER_MCP_GATEWAY_TOKEN`;
     extend `aether-cli/src/mcp_command.rs` so that when the unix-socket env
     var is absent but the gateway URL is present, it connects via
     `StreamableHttpClientTransport` with the bearer token instead of the
     unix socket. The model-facing UX (`aether mcp <server> <tool> --json …`)
     is byte-identical, so `progressive_discovery_instructions.md` needs only a
     one-line tweak.
   - Only the **session-scoped gateway token** enters the sandbox — revocable,
     short-lived, and useless outside that session if bound to an audience/
     expiry the harness checks in middleware. Linear/Sentry OAuth tokens stay
     in the harness vault (`aether-auth`).
   - **Reject option 2 (harness "expands/substitutes" bash calls).** It
     requires parsing arbitrary shell to find embedded tool calls —
     fundamentally unreliable (quoting, pipes, scripts, jq) — and changes the
     tool contract. Option 1 reuses the exact IPC mechanism that already works
     over the unix socket, just with a different transport + auth.
5. **Subagents server needs special-casing.** `SubAgentsMcp::embedded_*`
   spawns nested agents needing LLM creds (`AgentDeps`); in a credential-less
   sandbox that must not run. Serve `subagents` in `standalone` mode only, or
   exclude it from the sandbox binary and keep it harness-side (recommendation:
   exclude in v1, keep harness-side; document why). Same consideration for any
   future server that needs `AgentDeps`/vault access.
6. **Assume ACP 2.0 (per issue).** No ACP changes: harness-side remote servers
   already flow through `map_acp_mcp_servers` (`aether-cli/src/acp/protocol/mcp.rs`)
   → `RuntimeBuilder::extra_servers`. The sandbox endpoints are just more
   `type: "http"` entries.

### Trade-offs / non-goals

- No SSE server support (`type: "sse"` is currently parsed but served as
  streamable HTTP client-side; out of scope).
- No multi-tenancy inside one `mcp-serve` process (one checkout / one token per
  process; scale by running more processes).
- TLS termination is the deployer's job (reverse proxy / Tailscale / VPC);
  bearer-over-plaintext on loopback is acceptable for dev, documented as such.
- `legacy_session_mode` / stateful sessions via `LocalSessionManager` (default);
  investigate `with_legacy_session_mode(false)` stateless only if session
  affinity becomes an operational problem — do not gold-plate v1.

---

## Implementation Steps

1. **Decide binary + route naming; spike the rmcp server transport.**
   - Confirm with maintainer: binary name (`aether-mcp-serve` vs
     `aether mcp-serve` subcommand — prefer separate `[[bin]]` so the sandbox
     image need not carry the TUI/LLM stack long-term) and routes
     (`/mcp/<server>`).
   - Spike: enable `rmcp/transport-streamable-http-server` in a scratch branch,
     serve `SurveyMcp::new()` via `StreamableHttpService::new(|| Ok(...),
     Arc::new(LocalSessionManager::default()), StreamableHttpServerConfig::default())`
     mounted in axum, and connect with the existing HTTP client path. Time-box:
     1 day. This de-risks feature unification (`default-features = false` in
     workspace `rmcp` dep) and axum version compat (workspace axum 0.8.9).
2. **Add server-side HTTP transport dependencies.**
   - `crates/mcp-utils/Cargo.toml`: add feature `server-http = ["rmcp/transport-streamable-http-server", "dep:axum", "dep:tower", ...]`
     (or put axum deps on `aether-cli` directly — prefer `mcp-utils` so the
     gateway + sandbox server share one helper module).
   - Workspace `Cargo.toml` already has `axum`, `tower`, `tower-http`, `http` —
     just wire them as (optional) deps.
   - New module `crates/mcp-utils/src/server/http.rs` (or `server_http.rs`):
     `pub async fn serve_routes(routes: Vec<(path, factory)>, bind, auth) -> Result<...>`
     — shared axum app builder: per-route `StreamableHttpService`, bearer-auth
     middleware (`HttpError::Unauthorized` → 401, allowlist `allowed_hosts`
     passthrough from `StreamableHttpServerConfig`), graceful shutdown via
     `CancellationToken`.
3. **Create the sandbox binary.**
   - `crates/aether-cli/src/bin/mcp_serve.rs` (name per step 1) with clap args:
     `--port/--bind`, `--root-dir` (default: cwd), `--bearer-token`
     (default: `$AETHER_MCP_Bearer_TOKEN`, auto-generate + print once if
     absent in dev?), `--server coding,skills,tasks,plan,survey`
     (multi-select, default all-minus-subagents), plus per-server passthrough
     args mirroring `mcp-servers-stdio` (`--rules-dir`, `--permission-mode`,
     `--disable-lsp`, `--plans-dir`, …). Reuse `CodingMcpArgs::from_args`,
     `SkillsMcp::from_args`, etc. exactly as `src/bin/stdio.rs` does.
   - `[[bin]]` entry in `crates/aether-cli/Cargo.toml`; check `dist` picks it
     up (dist `packages = ["aether-agent-cli"]` ships all bins by default —
     verify with `dist plan` / `cargo dist generate-ci` dry run).
   - Per-connection server construction: factory closures must build a **fresh**
     `CodingMcp` per session (no shared `&mut`); confirm `CodingMcp`/`TasksMcp`
     are `Clone` or cheaply re-constructible from parsed args (store parsed
     args in `Arc` and clone-construct inside the factory).
4. **Harness-side: document + test the remote connection (mostly exists).**
   - Add an example `mcp.json` snippet + docs showing the sandbox servers as
     `{"type": "http", "url": "http://<sandbox>:<port>/mcp/coding",
     "headers": {"Authorization": "Bearer ${SANDBOX_TOKEN}"}}`.
   - No changes expected in `mcp-utils/src/client/{config,connection}.rs` or
     `aether-core/src/mcp/mcp_builder.rs` — verify by test (step 6). If the
     sandbox serves `instructions` per server, they flow through
     `McpConnectionDetails` already.
5. **Build the harness-side authenticated tool gateway (option 1).**
   - New tiny axum route harness-side serving the **existing**
     `GatewayService::new(mcp_handle)` via `StreamableHttpService` + the same
     bearer middleware from step 2, with a per-session token + expiry check.
     Likely home: `crates/aether-cli/src/` (e.g. `gateway.rs`) started by the
     harness when `--sandbox-gateway-bind` is set; keep it out of `aether-core`
     (axum is a CLI/deploy concern).
   - Extend `crates/aether-cli/src/mcp_command.rs`: connection selection =
     `AETHER_MCP_IPC_SOCKET` (unix, unchanged priority) else
     `AETHER_MCP_GATEWAY_URL` + `AETHER_MCP_GATEWAY_TOKEN` (HTTP via
     `StreamableHttpClientTransport::from_config` / `with_client`) else usage
     error. Keep output contract (exactly one JSON value) identical.
   - Sandbox `mcp-serve` startup: it does **not** need the gateway; the sandbox
     image just needs the `aether` (or extended `mcp`) client binary on `PATH`
     plus the two env vars — harness injects them at sandbox provision time
     (env propagation, like `shell_environment` today). Document that
     `DefaultCodingTools::with_current_exe_dir_on_path` behavior must hold in
     the sandbox image.
6. **Wire `deferTools` thought through.** Decide which sandbox servers are
   deferred vs model-visible from the harness perspective (recommendation:
   model-visible by default; harness operators use existing `deferTools` in
   their `mcp.json` — no new config keys). Ensure the unix-socket gateway still
   binds harness-side only when *harness-local* config has deferred tools
   (`mcp_builder.rs` logic unchanged).
7. **Docs + ops.**
   - `crates/mcp-servers/README.md` (or new `docs/aether/remote-sandbox.md`):
     architecture diagram (brain/hands/session per Managed Agents post),
     sandbox-image build checklist (binary, `PATH`, env vars, no credentials),
     token lifecycle, TLS note, subagents exclusion rationale.
   - `CHANGELOG.md` entry; `mcp.json` reference docs if a helper snippet is
     added.

---

## Testing Plan

- **Unit tests** (all against public API, fakes not mocks):
  - Bearer middleware: missing/wrong token → 401; correct → passthrough
    (axum test client, no network).
  - `mcp_command.rs` transport selection: unix socket preferred; gateway URL
    fallback; neither → usage error (fake unix gateway + fake HTTP server).
  - Args parsing for the new binary (clap `try_parse_from`), including
    per-server allowlist and default-excludes-subagents.
- **Integration tests** (in-process where possible, real HTTP on loopback):
  - Sandbox binary (started as a subprocess or in-process axum server) →
    harness `McpBuilder` + `StreamableHttpClientTransport` round trip:
    list tools + call a coding read-only tool against a tempdir checkout.
    **Write this test first and confirm it fails before implementing.**
  - End-to-end composition: bash tool inside sandbox executing
    `aether mcp <gateway-server> <tool>` against a fake harness gateway
    (mirrors existing
    `crates/aether-cli/tests/integration/mcp_command.rs::coding_bash_composes_real_cli_with_jq_pipes_redirects_and_scripts`),
    asserting piped/jq composition still works over the HTTP path.
  - Credential isolation: harness gateway proxies to a fake Linear-like server
    requiring a secret; assert the secret never appears in sandbox env,
    process args, or tool outputs (only the session bearer token does).
  - Auth negative: harness refuses to bootstrap (surfaces `Failed`, not hang)
    when the sandbox token is wrong — assert on `McpClientEvent`/snapshot
    status.
- **Edge cases:** bind-port conflict → clean `ServeError` enum variant (no
  `anyhow`); large tool outputs over SSE stream; concurrent sessions against
  one route (rmcp session manager isolation); graceful shutdown (in-flight
  `bash` task cancelled via `CancellationToken`); `--disable-lsp` in sandbox
  (LSP binaries may be absent in minimal images).

---

## Files to Modify/Create

| Path | Change | Kind |
|---|---|---|
| `crates/aether-cli/Cargo.toml` | Add `[[bin]] aether-mcp-serve` (final name TBD step 1); add axum/tower deps + `rmcp/transport-streamable-http-server` feature | Modified |
| `crates/aether-cli/src/bin/mcp_serve.rs` | New sandbox binary: clap args, per-server factory closures, axum mount, bearer middleware wiring | Added |
| `crates/mcp-utils/Cargo.toml` | New `server-http` feature: `rmcp/transport-streamable-http-server`, `axum`, `tower`, `tower-http` | Modified |
| `crates/mcp-utils/src/server/http.rs` | Shared axum app builder + bearer-auth middleware + `HttpServeError` enum | Added |
| `crates/mcp-utils/src/server/mod.rs` | Export new module | Modified |
| `crates/aether-cli/src/mcp_command.rs` | HTTP gateway fallback (`AETHER_MCP_GATEWAY_URL/TOKEN`) alongside unix socket | Modified |
| `crates/aether-cli/src/gateway.rs` (or `sandbox_gateway.rs`) | Harness-side authenticated gateway serving existing `GatewayService` | Added |
| `crates/aether-cli/src/main.rs` / `runtime.rs` | Flag to bind harness gateway (`--sandbox-gateway-bind`); inject gateway env into sandbox provisioning path | Modified |
| `crates/mcp-servers/src/progressive_discovery_instructions.md` | One-line note that `aether mcp` may target a remote gateway (UX unchanged) | Modified |
| `crates/mcp-utils/src/client/{config,connection}.rs`, `crates/aether-core/src/mcp/mcp_builder.rs` | Expected **no** changes — verify via tests | Unmodified |
| `crates/mcp-servers/src/bin/stdio.rs` | Expected **no** changes (reference implementation for server construction) | Unmodified |
| `dist-workspace.toml` / release config | Verify new bin ships; adjust if `cargo dist` needs explicit `binaries` list | Modified if needed |
| `docs/aether/remote-sandbox.md` (new) + `crates/mcp-servers/README.md` | Architecture, sandbox image checklist, token lifecycle, `mcp.json` snippets | Added/Modified |
| `CHANGELOG.md` | Feature entry | Modified |
| `crates/aether-cli/tests/integration/mcp_serve.rs` (new) + `mcp_command.rs` | Round-trip, composition-over-HTTP, credential-isolation, auth-negative tests | Added/Modified |

---

## Additional Notes

- **Documentation updates needed:** new `docs/aether/remote-sandbox.md` is the
  primary deliverable alongside code; keep the Managed Agents mapping
  (session/harness/sandbox) explicit so future ACP-2.0 work can reference it.
- **Follow-ups (out of scope, file as issues):** (a) sandbox provisioning
  recipe (`provision({resources})` — container image + token minting); (b) TLS
  / identity for the sandbox and gateway endpoints (mutual TLS or cloud IAM);
  (c) session persistence/replay (`wake(sessionId)` equivalent — aether
  sessions already durable, needs cross-process story); (d) read-only
  audit log of gateway-proxied calls; (e) per-tool scoping of the gateway
  token (today: whole-gateway bearer); (f) reconnect/session-restore across
  `mcp-serve` restarts (rmcp `SessionStore` is pluggable — revisit when needed).
- **Clarifying questions for the maintainer (block step 1 if undecided):**
  1. Binary name and distribution: separate `aether-mcp-serve` bin vs
     `aether mcp-serve` subcommand?
  2. Should the sandbox image contain the full `aether` binary (simplest for
     `aether mcp` composition) or a slimmer client?
  3. Confirm `subagents` stays harness-side in v1.
