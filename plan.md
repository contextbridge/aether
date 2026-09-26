# Issue #527 — Support a wasm / browser-based ACP client

## Overview

### Problem statement

`acp-utils` provides the ACP client used by the TUI (`wisp`) and the native remote
client (`aether client`). A second consumer is needed: a **browser-based ACP client
compiled to wasm** (`wasm32-unknown-unknown`) that connects to `aether server` (ACP v2
over WebSocket, default `ws://127.0.0.1:8765`).

Blockers today:

1. **ACP version.** The workspace pins `agent-client-protocol = "=2.1.0"`
   (`Cargo.toml:49`). The `wasm_js` feature (`uuid/js`, which gives `uuid` a Web Crypto
   randomness source on `wasm32-unknown-unknown`) first ships in **2.2.0**.
2. **Transitive native-only deps.** `acp-utils` pulls in `aether-llm` (reqwest, tokio
   `net`/`rt-multi-thread`), `aether-mcp-utils` (tokio `net`, rmcp server) and
   `aether-utils` (tokio `process`) unconditionally. Tokio refuses to compile on wasm
   with those features (`compile_error!("Only features sync,macros,io-util,rt,time are
   supported on wasm.")`). The client path only needs **plain serde types** from them:
   - `llm`: `ContentBlock` conversions in `content.rs`, `SessionUsageEvent` in
     `notifications::SessionUsageParams`.
   - `rmcp`: conversions in `elicitation.rs`.
   - `mcp_utils`: re-exported `McpServerStatus*` and `ToolDisplayMeta`/`ToolResultMeta`.
     `McpServerStatusEntry` is part of `AcpEvent::McpNotification`.
   - `utils`: `ReasoningEffort` and `is_false` in `config_meta.rs`. Tokio `process` is
     only used by `utils::shell_expander`.

   `acp-utils`' own tokio features (`rt`, `macros`, `sync`, `io-util`, `time`) and
   `tokio-util` are all in tokio's wasm-supported set.
3. **Runtime-bound connection driver.** `connect_acp_client` starts its connection
   future with `tokio::spawn` and stops it with `JoinHandle::abort`
   (`crates/acp-utils/src/client/session.rs:45`). Both need a tokio runtime. Everything
   else the client uses (`tokio::sync::{mpsc, oneshot}`, `CancellationToken`,
   `block_task()`) is runtime-independent and compiles on wasm.
4. **No browser transport.** `acp_utils::websocket::WebSocketTransport` wraps
   `tokio-tungstenite` over `AsyncRead + AsyncWrite`. Browsers expose a message-based
   `web_sys::WebSocket`. Its JS handles are `!Send` (`JsValue` carries
   `PhantomData<*mut u8>`). ACP's `ConnectTo` and `Lines` require `Send + 'static`
   sinks, streams and futures.
5. **No JS surface.** There is no `wasm-bindgen` API and no npm package. The existing
   `@aether-agent/sdk` is Node-only (it spawns `aether acp` over stdio).

### Success criteria / acceptance conditions

- [ ] The workspace builds on `agent-client-protocol =2.2.0`, and all existing native tests pass.
- [ ] `just wasm-check` (`cargo clippy -p aether-acp-wasm --target wasm32-unknown-unknown
      -- -D warnings`) passes. The wasm dependency graph contains no `aether-llm`,
      `rmcp`, `aether-mcp-utils`, `reqwest` or `tokio-tungstenite`.
- [ ] `acp-utils` keeps its feature set and defaults. `wisp`, `aether-cli`, `aether-core`
      and `aether-sessions` need no manifest changes and keep identical behavior. The
      existing `acp-utils` client suites pass without modification.
- [ ] A browser WebSocket transport in `aether-acp-wasm` speaks the wire protocol
      `aether server` expects:
      - text frames carry JSON-RPC lines
      - binary frames are rejected
      - clean close ends the stream
      - server pings are tolerated
- [ ] A `wasm-bindgen` facade (`aether-acp-wasm`) exposes the following, returning JS
      `Promise`s, with a generated `.d.ts` typed against ACP v2 types:
      - `connect` and `initializeResponse`
      - session new/resume
      - `prompt`, `cancel`, `disconnect`
      - event delivery, with a `respond` function on elicitation events
- [ ] `packages/aether-browser` (`@aether-agent/browser`) builds the facade with
      `wasm-pack` and is consumable from `packages/`.
- [ ] `wisp` and the facade reduce sessions with one shared model,
      `acp_utils::conversation`, and the facade delivers it as `conversation_changed`
      snapshots.
- [ ] Wasm tests (Node) cover:
      - transport open, failed open, text round-trip, binary rejection and close
      - connect/initialize
      - new session and the prompt/update stream
      - elicitation round-trip
      - disconnect and `connection_closed`
- [ ] `just ci` stays green on host and includes `just wasm-check`. Docs are updated.

## Technical Approach

### High-level architectural decisions

1. **Bump ACP; don't fork it.** Move the workspace pin to `=2.2.0`. The wasm-relevant
   delta is the opt-in `wasm_js` feature. The `ConnectTo: Send + 'static` bound is
   unchanged in 2.2.0, and this design satisfies it rather than working around it.
2. **One crate, gated by target where the target is the constraint.** `acp-utils`
   serves both native and wasm consumers:
   - The wire types (`meta`, `config_meta`, `config_option_id`, `notifications`) and
     the `client` module compile on both targets.
   - The conversions that exist to integrate native-only crates (`content.rs` for
     `llm`, `elicitation.rs` for `rmcp`, `SessionUsageParams` for `llm`) are marked
     `cfg(not(target_family = "wasm"))`, and `llm`/`rmcp` are native-target
     dependencies. Those crates can't compile on wasm, so the target is the real
     condition. A feature flag would only restate it and would have to be threaded
     through every consumer.
   - Features (`client`, `testing`, `websocket`) and defaults stay as they are.
     `websocket` is the native `tokio-tungstenite` transport, and wasm builds never
     enable it.
3. **Give the shared wire types wasm-safe homes.**
   - Move `mcp_utils::{status, display_meta}` into `aether-utils`. They are plain serde
     types that both `mcp-utils` and `acp-utils` need, and both already depend on `utils`.
   - Mark `utils::shell_expander` native-only, and enable tokio `process` only on
     native targets.
4. **Use ACP's own concurrency; target-select only the root spawn.**
   - ACP's `ConnectionTo::spawn` / `Builder::with_spawned` don't need a runtime. Tasks
     go into a `futures::channel::mpsc` queue that the connection's task actor polls
     concurrently (`jsonrpc/task_actor.rs`). Any background work inside a connection
     uses `cx.spawn`.
   - Those tasks only run while the connection future itself is being polled.
     `connect_acp_client` hands `AcpClientHandle` out to code running outside the
     connection (wisp's event loop, the JS facade), so the root driver needs an
     executor.
   - That single spot uses a private, target-`cfg`'d `spawn()`: `tokio::spawn` natively,
     `wasm_bindgen_futures::spawn_local` on wasm.
   - `futures::future::abortable` replaces `JoinHandle::abort`.
5. **The browser transport lives in the facade crate, as `Send` channels in front of
   `!Send` JS handles.**
   - `acp_wasm::websocket::connect_websocket(url)` returns `Lines::new(outgoing,
     incoming)`, where both halves are `futures::channel::mpsc` endpoints
     (`Send + 'static`). ACP's `Lines` already implements `ConnectTo`, so no transport
     type is needed.
   - A `spawn_local`'d pump task owns the `web_sys::WebSocket` and its JS closures and
     shuttles frames between the socket and the channels. No `Send` wrappers or unsafe
     impls are needed.
   - The facade is the transport's only consumer, so `acp-utils` carries no `web-sys`
     or `wasm-bindgen` dependency.
6. **Thin, typed `wasm-bindgen` facade.**
   - A separate `crates/acp-wasm` (`cdylib` + `rlib`) holds the browser transport and
     the `#[wasm_bindgen]` glue over `AcpClientHandle`. Protocol logic stays in
     `acp-utils` and is unit-tested natively.
   - The generated `.d.ts` is typed directly against the ACP v2 TS types through
     `unchecked_param_type` / `unchecked_return_type` and a `typescript_custom_section`.
     The npm package is the `wasm-pack` output, with no hand-written wrapper repeating
     the surface.

### Design patterns to employ

- **Facade:** `acp-wasm` over `acp-utils::client`.
- **Adapter:** the browser socket to ACP `Lines`.
- **Target-gated modules:** native-only integrations compile out on wasm, instead of
  living in parallel crates or behind feature flags.
- **Test builder + Fake:** extend the existing `FakeAgent` / `TestPeer` builders in
  `acp_utils::testing`, and assert on observable state, not call counts.
- **Tight `cfg` scope:** `cfg(target_family = "wasm")` appears only in the root `spawn`
  helper, the availability gates on native-only modules, and the facade crate. Business
  logic has no target forks.

### Key technical considerations and trade-offs

- **Host builds never compile the wasm configuration.** An ungated `use llm::…` in a
  shared module compiles on host but breaks wasm. The `just wasm-check` recipe (run by
  `just ci` and CI) is the guard. Keep that check in CI permanently.
- **Why target `cfg` rather than features for native-only modules.** `llm`, `rmcp` and
  tokio `process` can't build on wasm, so any feature toggling them would be on for
  every native consumer and off for wasm. Gating on the target expresses that directly
  and leaves every consumer manifest untouched. Native consumers that don't use the
  conversions (`aether-core`, `aether-sessions`) still compile `llm` and `rmcp`
  through `acp-utils`.
- **Why target `cfg` rather than an injectable spawner.** The target alone decides the
  executor (tokio natively, the browser event loop on wasm), and there is exactly one
  spawn site. A spawner trait or a caller-spawned driver would add API surface and
  change every call site (wisp, aether-cli, tests) for no extra flexibility.
- **Disconnect contract.** `AcpClientHandle::disconnect()` aborts the driver and waits
  for `closed`. Dropping the driver drops `ConnectionEvents`, which emits
  `AcpEvent::ConnectionClosed` and cancels `closed`. It sends neither `session/cancel`
  nor `session/close`. `abortable` preserves this exactly, since an aborted future is
  dropped the same way a tokio-aborted task is.
- **`block_task()` discipline.** Never `await` a request inside a `HandleDispatchFrom`
  handler (it deadlocks the dispatch loop). The client only blocks in the
  connection-setup closure and in `request()` futures. The facade must keep it that
  way, so handlers only forward into channels.
- **Elicitation responders.** `Responder<T>` is `Send`, so `AcpEvent` is shared as-is.
  The facade moves each responder into a one-shot JS `respond` function
  (`Closure::once_into_js`) attached to the `elicitation_request` event, so the facade
  keeps no responder state. An elicitation that JS never answers keeps its closure
  alive until the page unloads. Nothing hangs, because the server cancels pending
  elicitations on disconnect.
- **Server contract** (`aether-cli/src/acp/server.rs`, `state.rs`, `remote.mdx`):
  - Only one client can be attached. A second handshake gets HTTP 409
    `client already attached`.
  - `RemoteServerInfo { cwd, session_id }` is advertised in the initialize meta.
  - Resume-with-replay delivers history.
  - Detaching does not stop a running turn.
  - Pending permission/elicitation requests are cancelled on disconnect.
  - The server pings every 20 s.
  - There is no auth or TLS on the raw listener.
- **Browser-specific constraints.**
  - Browsers don't expose the HTTP status of a rejected WebSocket handshake, and they
    report any failed open as close code 1006 with an empty reason. A 409 therefore
    surfaces as a generic `connect_failed` error, and the docs state the single-client
    rule.
  - Pages served over `https://` can only open `wss://`, which requires a TLS/auth
    gateway in front of `aether server`. Document it; don't build it.

## Implementation Steps

### Phase 0 — ACP 2.2.0

1. **Update the workspace pin** (`Cargo.toml:49`):
   ```toml
   agent-client-protocol = { version = "=2.2.0", features = ["unstable_protocol_v2", "unstable_session_compaction"] }
   ```
   2.2.0 has no `unstable_tool_call_name` feature (tool-call names are stable in schema
   1.9), so it must not be listed.
2. **Refresh the lockfile:** `cargo update -p agent-client-protocol` (schema `=1.9.1`).
3. **Fix compile errors** in `acp-utils`, `wisp` and `aether-cli`. Then run
   `just test -p aether-acp-utils -p wisp -p aether-agent-cli`. The `client_session`,
   `client_turns`, `client_disconnect`, `protocol_transport`, `websocket` and
   `acp_remote` suites are the canaries.
4. **Verify the native round-trip** from `aether server` to `aether client` before
   continuing.

### Phase 1 — wasm-safe shared types

5. **`aether-utils`: make `shell_expander` native-only.**
   - Reduce the base tokio dep to `["rt", "macros"]` and add `process` on native
     targets:
     ```toml
     [target.'cfg(not(target_family = "wasm"))'.dependencies]
     tokio = { workspace = true, features = ["process"] }
     ```
   - Mark `pub mod shell_expander;` with `#[cfg(not(target_family = "wasm"))]`. Its
     users (`aether-core`, `mcp-servers`) are native-only and need no changes.
   - `markdown_file`'s `tokio::spawn` only needs `rt`, which compiles on wasm.
6. **Move the MCP wire types into `aether-utils`.**
   - `crates/mcp-utils/src/status.rs` → `crates/utils/src/mcp_status.rs`.
   - `crates/mcp-utils/src/display_meta.rs` → `crates/utils/src/display_meta.rs`.
   - Both depend only on serde, schemars and `std::path`.
   - Update imports in `mcp-utils`, `mcp-servers`, `aether-core`, `aether-cli` and
     `acp-utils` (about 35 files). Don't leave re-export shims in `mcp-utils`.
   - `acp_utils::notifications` keeps re-exporting `ToolDisplayMeta`, `ToolResultMeta`
     and `McpServerStatus*` from `utils`, because they are part of its wire types.

### Phase 2 — wasm build of `acp-utils` and a runtime-independent client

7. **Update `crates/acp-utils/Cargo.toml`:**
   ```toml
   [features]
   default = ["client"]
   client = []
   testing = ["client"]
   websocket = ["dep:tokio-tungstenite"]

   [dependencies]
   agent-client-protocol = { workspace = true }
   clankerdiff-protocol = { workspace = true }
   futures = { workspace = true }
   schemars = { workspace = true }
   serde = { workspace = true }
   serde_json = { workspace = true }
   thiserror = { workspace = true }
   tokio = { workspace = true, features = ["rt", "macros", "sync", "io-util", "time"] }
   tokio-tungstenite = { workspace = true, optional = true }
   tokio-util = { workspace = true }
   tracing = { workspace = true }
   utils = { package = "aether-utils", path = "../utils", version = "…" }

   [target.'cfg(not(target_family = "wasm"))'.dependencies]
   llm = { package = "aether-llm", path = "../llm", version = "…" }
   rmcp = { workspace = true, features = ["client", "elicitation"] }

   [target.'cfg(target_family = "wasm")'.dependencies]
   agent-client-protocol = { workspace = true, features = ["wasm_js"] }
   wasm-bindgen-futures = { workspace = true }
   ```
   - Features and defaults stay as they are today.
   - Remove `aether-mcp-utils` (replaced by `utils`) and the unused `clankerdiff-core`.
   - Add `wasm-bindgen`, `wasm-bindgen-futures`, `web-sys`, `js-sys`,
     `serde-wasm-bindgen` and `wasm-bindgen-test` to `[workspace.dependencies]`.
8. **Gate the native-only modules** with `#[cfg(not(target_family = "wasm"))]`:
   - `lib.rs`: `pub mod content;` and `pub mod elicitation;`.
   - `notifications.rs`: `SessionUsageParams` and its tests.
   - Everything else stays ungated.
9. **Make the connection driver runtime-independent** (`client/session.rs`):
   ```rust
   let (driver, abort) = abortable(run_client_connection(agent, init_request, init_tx, events));
   spawn(async move {
       let _ = driver.await;
   });
   let connection = Arc::new(ClientConnection { abort, closed });
   ```
   ```rust
   #[cfg(not(target_family = "wasm"))]
   fn spawn(future: impl Future<Output = ()> + Send + 'static) {
       tokio::spawn(future);
   }

   #[cfg(target_family = "wasm")]
   fn spawn(future: impl Future<Output = ()> + 'static) {
       wasm_bindgen_futures::spawn_local(future);
   }
   ```
   - `ClientConnection` holds `abort: AbortHandle` instead of `driver: JoinHandle<()>`.
   - Both `disconnect()` and `Drop` call `abort.abort()`, and `disconnect()` then awaits
     `closed.cancelled()`.
   - `tokio::sync` channels, `CancellationToken` and the `ClientHandlers` dispatch chain
     (including auto-approve) are unchanged.
10. **Consumers.** `wisp`, `aether-cli`, `aether-core` and `aether-sessions` keep their
    manifests. Run the Phase 0 canary suites again, plus `wisp`'s `runtime_acp` tests.
11. **Scaffold `crates/acp-wasm` and add the wasm gate.**
    - `aether-acp-wasm`: `crate-type = ["cdylib", "rlib"]`, `publish = false`,
      `[package.metadata.dist] dist = false`, added as a workspace member.
    - `lib.rs` starts with `#![cfg(target_family = "wasm")]`, so host workspace builds
      compile an empty crate.
    - Under `[target.'cfg(target_family = "wasm")'.dependencies]`:
      - `aether-acp-utils` (default features, i.e. `client`), `agent-client-protocol`
      - `wasm-bindgen`, `wasm-bindgen-futures`, `js-sys`
      - `web-sys` (`WebSocket`, `MessageEvent`, `CloseEvent`, `Event`, `BinaryType`)
      - `futures`, `serde`, `serde-wasm-bindgen`, `thiserror`
    - Add `targets = ["wasm32-unknown-unknown"]` to `rust-toolchain.toml`.
    - Add a `just wasm-check` recipe running `cargo clippy -p aether-acp-wasm --target
      wasm32-unknown-unknown -- -D warnings`. It compiles `acp-utils` with exactly the
      features the facade uses, and stays the same as the facade grows.
    - Add `wasm-check` to `just ci`.

### Phase 3 — Browser transport and `wasm-bindgen` facade

12. **Implement the browser transport** (`crates/acp-wasm/src/websocket.rs`):
    ```rust
    #[derive(Debug, Error)]
    pub enum WebSocketError {
        #[error("invalid WebSocket URL: {0}")]
        InvalidUrl(String),
        #[error("WebSocket connection failed")]
        ConnectFailed,
    }

    /// Opens the socket and resolves once it is open (or failed to open).
    pub async fn connect_websocket(url: &str) -> Result<impl ConnectTo<Client>, WebSocketError> {
        // outgoing: mpsc::Sender<String>, incoming: mpsc::UnboundedReceiver<io::Result<String>>
        Ok(Lines::new(outgoing.sink_map_err(io::Error::other), incoming))
    }
    ```
    `connect_websocket` creates the `web_sys::WebSocket` with `binary_type =
    Arraybuffer`, wires the `onopen` / `onmessage` / `onclose` / `onerror` closures, and
    `spawn_local`s a pump that owns the socket and closures:
    - **Open:** the result is delivered through a `oneshot`. A constructor exception
      (invalid URL, mixed-content rejection) maps to `InvalidUrl`. A close or error
      before open maps to `ConnectFailed`.
    - **Outbound:** the channel is bounded to 32 slots, matching the native transport.
      Each `String` becomes one `send_with_str` text frame. When the channel ends (ACP
      dropped the sink, e.g. on `disconnect()` or a failed connection), close the
      socket with 1000.
    - **Inbound text:** push `Ok(text)`.
    - **Inbound binary:** push `Err(io::ErrorKind::InvalidData)` ("binary WebSocket
      messages are not supported"). `Lines` fails the connection on a transport error,
      which ends the outbound channel and closes the socket.
    - **Close:** a clean close drops the inbound sender, so EOF flows through ACP to
      `AcpEvent::ConnectionClosed`. An abnormal close pushes an error carrying the code
      and reason first.
    - **Keepalive:** none is needed, because browsers answer server pings automatically.
      `send_with_str` buffers internally, so there is no write deadline.
13. **Expose the API** (`crates/acp-wasm/src/lib.rs`):
    ```rust
    #[wasm_bindgen]
    pub struct AetherClient { handle: AcpClientHandle, initialize_response: InitializeResponse }

    #[wasm_bindgen]
    impl AetherClient {
        pub async fn connect(url: String, on_event: js_sys::Function) -> Result<AetherClient, ClientError>;
        #[wasm_bindgen(getter, js_name = initializeResponse)] pub fn initialize_response(&self) -> JsValue;
        #[wasm_bindgen(js_name = newSession)] pub fn new_session(&self, request: JsValue) -> Result<Promise, ClientError>;
        #[wasm_bindgen(js_name = resumeSession)] pub fn resume_session(&self, request: JsValue, replay: bool) -> Result<Promise, ClientError>;
        pub fn prompt(&self, request: JsValue) -> Result<Promise, ClientError>;
        pub fn cancel(&self, session_id: String) -> Result<(), ClientError>;
        pub fn disconnect(&self) -> Promise;
    }
    ```
    - **`connect`:** runs `connect_websocket(url)`, then
      `connect_acp_client(transport, initialize_request())`. The initialize request
      mirrors wisp's `Session::initialize_request` (`ProtocolVersion::V2` plus
      elicitation capabilities). `initializeResponse` exposes the full response, so JS
      reads the agent info, capabilities, auth methods and the `RemoteServerInfo` meta
      from one place.
    - **Methods:** return `Promise`s via `future_to_promise` over
      `AcpClientHandle::request` futures. Those are `'static` (`+ use<R>`), so no
      `&self` borrow crosses an await. Requests and responses convert with
      `serde-wasm-bindgen`.
    - **Types:** every `JsValue` parameter and return carries its ACP v2 TS type:
      ```rust
      #[wasm_bindgen(js_name = newSession, unchecked_return_type = "Promise<NewSessionResponse>")]
      pub fn new_session(
          &self,
          #[wasm_bindgen(unchecked_param_type = "NewSessionRequest")] request: JsValue,
      ) -> Result<Promise, ClientError>;
      ```
      A `typescript_custom_section` imports those types from
      `@agentclientprotocol/sdk/experimental/v2` and declares the `AetherClientEvent`
      union that types `on_event`.
    - **Event pump:** a `spawn_local` loop drains `event_rx` and calls `on_event` with a
      tagged object:
      - `{ type: "session_update", notification }`
      - `{ type: "elicitation_request", request, respond }`
      - `{ type: "context_cleared" | "sub_agent_progress" | "auth_methods_updated" | "mcp_notification" | "git_diff_event", params }`
      - `{ type: "connection_closed" }`

      The callback is taken at `connect`, so no events are lost before subscription.
    - **Elicitations:** `respond` is a one-shot JS function (`Closure::once_into_js`)
      that owns the event's `Responder` and converts the JS response with
      `serde-wasm-bindgen`.
    - **Errors:** methods fail with `ClientError`, a `thiserror` enum that converts into
      a JS `Error` with a stable `code` property:
      - `connect_failed`: a `WebSocketError`, or the connection ending before
        initialize completes
      - `protocol`: `AcpClientError::Protocol`
      - `invalid_argument`: a JS value that doesn't deserialize
14. **Add the npm package `@aether-agent/browser` in `packages/aether-browser/`** (covered
    by the existing `packages/*` pnpm workspace glob):
    - `package.json` with a build script
      `wasm-pack build ../../crates/acp-wasm --target web --out-dir ../../packages/aether-browser/pkg --out-name browser`.
      Its `exports` and `types` point into `pkg/`, and `@agentclientprotocol/sdk` is a
      peer dependency, because the generated `.d.ts` imports its v2 types.
    - The generated `.d.ts` is the typed API. The package has no hand-written source.
    - `test/usage.ts` exercises the public surface and is compiled with `tsc --noEmit`
      to check that the generated types resolve.
    - This package stays separate from `@aether-agent/sdk`, which is Node-only and
      should not carry the wasm module or its Rust build.

### Phase 3b — Shared conversation reducer

Browser UIs should behave like `wisp`: the same turn handling, replay behaviour and
sub-agent display. The reduction rules move out of `wisp` into
`acp_utils::conversation`, which builds on both targets.

- **`Conversation`** holds one session's items (messages keep their `ContentBlock`s,
  with streamed text merged; tool calls with their sub-agent trees; host notices), each
  with a revision and open/sealed state, plus the turn (`TurnPhase`), the agent's
  `Activity` (phase and streamed thought), compactions, context usage and the latest
  plan. Hosts route events to it by session: `apply_update`, `apply_sub_agent_progress`,
  `clear`, `connection_closed`, and the prompt lifecycle (`start_prompt`,
  `accept_prompt`, `reject_prompt`). `apply_update` returns `TurnFinished` when a turn
  it was waiting on reaches idle.
- **`wisp`** keeps only its own concerns: `ForegroundOperation` loses its prompt phases
  (session and workspace operations, attachment preparation), and the views keep the
  display rules (thought tail, elapsed time, spinner, plan grace period, the bell,
  flattening blocks to text). A `running` state while idle now adopts the turn during a
  resume too, so reattaching to a live turn shows it as running.
- **The facade** tracks a conversation per session, drives the turn from its own
  `prompt()` calls (echo, accept/reject; `turn_in_progress` unless idle), restarts it on
  `resumeSession`, and emits `conversation_changed` after each change. Snapshots reuse
  the JS object of every item whose id and revision did not change.
- **`acp_utils::content`** builds on wasm; only its `aether-llm` conversions stay
  native-only.

### Phase 4 — Test server, CI, docs

15. **Test server.** Add an `acp-utils` example `fake_agent_ws`
    (`required-features = ["testing", "websocket"]`) that serves `FakeAgent` over the
    native `WebSocketTransport` on a given port. The scenario is chosen by request path:
    - `/basic`: advertises `RemoteServerInfo`, and a prompt streams updates to an idle
      state.
    - `/elicitation`: a prompt triggers an elicitation, and the agent echoes the answer
      as an agent message chunk.
    - `/binary`: sends one binary frame on the raw socket after the handshake.

    Extend the `FakeAgent` builder for any scenario behavior it lacks.
16. **`just wasm-test` recipe:**
    - starts `fake_agent_ws` and exports its URL as `FAKE_AGENT_WS_URL` (the tests read
      it with `env!`)
    - runs `wasm-pack test --node crates/acp-wasm` (Node ≥ 22 has a global `WebSocket`)
    - builds `packages/aether-browser` and runs its `tsc --noEmit` check
    - stops the server

    The wasm suite runs the same way locally and in CI.
17. **CI** (`.github/workflows/ci.yml`): add a wasm job that installs `wasm-pack` and
    Node ≥ 22 (the wasm target comes from `rust-toolchain.toml`) and runs
    `just wasm-check wasm-test`.
18. **Docs:**
    - `crates/acp-utils/README.md`: wasm support, and which modules are native-only.
    - `crates/acp-wasm/README.md`: browser quickstart, from `aether server` to
      `AetherClient.connect("ws://127.0.0.1:8765", onEvent)`.
    - `packages/aether-browser/README.md`.
    - `packages/website/src/content/docs/aether/running/remote.mdx`: a browser-client
      section covering the `wss://` gateway requirement, the single-client rule and how
      a 409 appears in browsers, and that there is no automatic reconnect.

## Testing Plan

### Unit and integration tests (native, `cargo nextest --all-features`)

- **ACP bump and target gating:** the existing `acp-utils` suites (`client_session`,
  `client_turns`, `client_disconnect`, `protocol_transport`, `websocket`,
  `remote_metadata`), `aether-cli/tests/integration/acp_remote/*` and `wisp`'s
  `runtime_acp` must all pass without modification. `client_disconnect` covers the
  `abortable` driver: `ConnectionClosed` on drop, and no `session/cancel` or
  `session/close` on `disconnect()`.
- **Moved types:** the existing `mcp-utils` / `mcp-servers` tests for display meta and
  server status keep passing against `utils::{display_meta, mcp_status}`.
- **Shared client behavior:** permission auto-approve, replay ordering and the
  disconnect contract live in `AcpClientHandle`, which native and wasm share. The
  native suites above are their coverage.

### Wasm tests (`crates/acp-wasm/tests/*.rs`, `wasm-bindgen-test` under Node, against `fake_agent_ws`)

Each test file starts with `#![cfg(target_family = "wasm")]`. The tests cover only the
browser transport and the JS glue.

- **Transport:**
  - connect/open succeeds
  - connecting to an unused port fails with `WebSocketError::ConnectFailed`
  - text frames round-trip as JSON-RPC
  - a binary frame from the server (`/binary`) fails the connection (no silent drop)
  - a server close emits `connection_closed`
- **Facade:**
  - connect → `initializeResponse` carries `RemoteServerInfo` in its meta
  - `newSession` → `prompt` → streamed `session_update` events → idle state →
    `Promise` resolves
  - an elicitation round-trip (calling the event's `respond` resolves the agent's
    request)
  - `disconnect()` resolves and emits `connection_closed`
- **Package:** `tsc --noEmit` over `test/usage.ts`.

### Edge cases to verify

- Binary frames are rejected on both transports (native closes `Unsupported`, the
  browser errors the stream, which fails the connection and closes the socket). They
  are never silently dropped.
- Requests and notifications after close reject with code `protocol` instead of
  panicking.
- If the event channel's receiver is gone, an elicitation responder is answered with
  `internal_error` (existing `ClientHandlers` behavior), not left hanging.
- An elicitation response that doesn't deserialize answers the agent with
  `invalid_params` and throws `invalid_argument` in JS.
- Tests use no timeouts. Synchronize on channel close and `oneshot` rendezvous.
- New errors are `thiserror` enums (`WebSocketError` and the facade's `ClientError`).
  Don't use `anyhow`.

## Files to Modify/Create

| File | Changes | Add / Modify / Remove |
|------|---------|----------------------|
| `Cargo.toml` (workspace) | Pin `agent-client-protocol =2.2.0` with `unstable_protocol_v2` + `unstable_session_compaction`; add wasm workspace deps; add `crates/acp-wasm` member | Modify |
| `Cargo.lock` | `cargo update -p agent-client-protocol`; new wasm deps | Modify |
| `rust-toolchain.toml` | Add `targets = ["wasm32-unknown-unknown"]` | Modify |
| `justfile` | `wasm-check` and `wasm-test` recipes; `wasm-check` in `ci` | Modify |
| `crates/utils/Cargo.toml`, `src/lib.rs` | Native-target tokio `process`; native-only `shell_expander`; export `display_meta`, `mcp_status` | Modify |
| `crates/utils/src/display_meta.rs`, `src/mcp_status.rs` | Moved from `mcp-utils` | Add |
| `crates/mcp-utils/src/display_meta.rs`, `src/status.rs` | Moved to `utils` | Remove |
| `crates/mcp-utils`, `crates/mcp-servers`, `crates/aether-core`, `crates/aether-cli` (sources) | Import `utils::{display_meta, mcp_status}` | Modify |
| `crates/acp-utils/Cargo.toml` | Native-target `llm`/`rmcp`; wasm-target ACP `wasm_js` and `wasm-bindgen-futures`; drop `aether-mcp-utils`, `clankerdiff-core`; `fake_agent_ws` example | Modify |
| `crates/acp-utils/src/lib.rs` | Native-only `content`, `elicitation` | Modify |
| `crates/acp-utils/src/notifications.rs` | Re-export display/status types from `utils`; native-only `SessionUsageParams` | Modify |
| `crates/acp-utils/src/client/session.rs` | Target-`cfg`'d `spawn`; `abortable` driver replacing `JoinHandle` | Modify |
| `crates/acp-utils/src/testing/fake_agent.rs` | Builder extensions for wasm scenarios | Modify |
| `crates/acp-utils/examples/fake_agent_ws.rs` | Fake agent WebSocket server for wasm tests | Add |
| `crates/acp-utils/README.md` | Wasm support and native-only modules | Modify |
| `crates/acp-wasm/Cargo.toml`, `src/lib.rs`, `src/websocket.rs`, `README.md` | `AetherClient` facade and browser transport | Add |
| `crates/acp-wasm/tests/*.rs` | Transport + facade wasm tests | Add |
| `crates/acp-utils/src/conversation/*`, `tests/conversation.rs` | Shared conversation reducer, moved from `wisp` | Add |
| `crates/wisp/src/app/*`, `src/conversation/*`, `src/renderer/*` | Reduce through `acp_utils::conversation`; keep display rules | Modify |
| `crates/acp-wasm/src/conversation.rs` | Per-session conversations and `conversation_changed` snapshots | Add |
| `packages/aether-browser/package.json`, `tsconfig.json`, `test/usage.ts`, `README.md` | `wasm-pack` build, type check | Add |
| `packages/website/src/content/docs/aether/running/remote.mdx` | Browser-client section | Modify |
| `.github/workflows/ci.yml` | Wasm job | Modify |

## Additional Notes

- **Not included:**
  - a server-side auth/TLS gateway
  - an auto-reconnect policy
  - porting the TUI to the browser
  - publishing the npm package (release wiring is a follow-up)
  - browser-hosted MCP servers
  - session settings (config options, auth methods, MCP server status, available
    commands) in the shared model; the facade still delivers them as raw events
- **Follow-ups:**
  - release wiring for `packages/aether-browser`
  - a browser MCP story (the `aether-sdk` inline MCP server is Node-only)
  - generated TS types for the Aether extension payloads, which the
    `AetherClientEvent` union types as `params: unknown`
- **Risks:**
  1. **Schema churn.** Moving from schema 1.7 to 1.9.1 may rename v2 session fields.
     Doing Phase 0 alone first contains this.
  2. **`clankerdiff-protocol` on wasm is unverified.** Its graph (`blake3`,
     `pulldown-cmark`, `arborium-theme`, `async-channel`) looks portable, but the first
     `just wasm-check` is the gate. If it fails, mark the `GitDiff*` payloads and the
     `AcpEvent::GitDiffEvent` variant native-only, like the other native-only modules.
  3. **Randomness.** `uuid` 1.26 uses `js-sys` crypto directly on
     `wasm32-unknown-unknown` via `wasm_js`. If another crate in the wasm graph pulls in
     `getrandom` 0.3/0.4, its `wasm_js` backend must be configured too.
  4. **Host builds never compile the wasm configuration**, so they can mask wasm
     regressions. Keep `just wasm-check` in `just ci`.
- **Fastest de-risking path:** Phase 0 plus Phases 1–2 through step 11, i.e. a green
  `just wasm-check` against the scaffolded `acp-wasm` crate, before building the
  transport or the facade API.

---

*Research basis:*
- *`crates/acp-utils`: client/session, websocket, notifications, testing, Cargo.toml.*
- *`crates/utils` and `crates/mcp-utils`: tokio usage, `status`/`display_meta`.*
- *`crates/aether-cli`: server/state/testing.*
- *`@agentclientprotocol/sdk/experimental/v2`: the v2 TS type exports.*
- *ACP 2.2.0 source:*
  - *`Lines` bounds, `jsonrpc.rs:6251`, and its `ConnectTo` impl, which fails the
    connection on an incoming transport error.*
  - *`ConnectionTo::spawn` and the task actor, `jsonrpc/task_actor.rs`.*
  - *the `wasm_js` feature, and the absence of `unstable_tool_call_name`.*
- *Tokio 1.53 wasm feature `compile_error!`, `lib.rs:467-479`.*
- *`uuid` 1.26 wasm target deps.*
- *`wasm-bindgen`: the `JsValue` `!Send` marker, `Closure::once_into_js`,
  `unchecked_param_type` / `unchecked_return_type` and `typescript_custom_section`.*
