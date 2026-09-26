# Issue #527 — Support a wasm / browser-based ACP client

## Overview

### Problem statement

`acp-utils` and `wisp` contain an ACP client used by the TUI (`wisp`) and the native
remote client (`aether client`). A second use case exists: connecting to an agent
running `aether server` (ACP v2 over WebSocket, default `ws://127.0.0.1:8765`) from a
**browser-based ACP client compiled to wasm** (`wasm32-unknown-unknown`).

Current blockers:

1. The workspace pins `agent-client-protocol = "=2.1.0"` (`Cargo.toml:49`). JS-hosted
   wasm support (`wasm32-unknown-unknown` via Web Crypto UUID backend) only landed in
   **ACP 2.2.0** (`wasm_js` feature → `uuid/js`, PR #308, released 2026-09-18).
2. `acp-utils/src/client/session.rs` (`connect_acp_client`) is hard-wired to tokio:
   `tokio::spawn`, `tokio::sync::{mpsc, oneshot}`, `tokio_util::CancellationToken`,
   `JoinHandle`. None of these work on a single-threaded browser event loop.
3. `acp-utils/src/websocket.rs` (`WebSocketTransport<T>`) is built on
   `tokio-tungstenite` + `tokio::io::{AsyncRead, AsyncWrite}` + `tokio::time`. Browsers
   expose `web-sys::WebSocket` (message-based, no raw TCP socket, automatic pong) —
   there is no `AsyncRead/AsyncWrite` socket to wrap.
4. Upstream `ConnectTo<R>: Send + 'static` with
   `connect_to(...) -> impl Future + Send` (ACP 2.1.0 **and** 2.2.0 `component.rs:107`)
   is retained in 2.2.0. On `wasm32-unknown-unknown` everything is single-threaded so
   `Send` is trivially satisfiable, but the code must still **type-check** with those
   bounds while **spawning** via `spawn_local`-style executors, not `tokio::spawn`.
5. `acp-utils` lib-level deps (`aether-llm` with `tokio net`, `rmcp`, `mcp-utils`,
   `clankerdiff-protocol`, `utils` with `tokio process`) leak native-only transitive
   deps into every consumer, even for pure helpers (`content`, `meta`, `notifications`,
   `config_meta`). A wasm build that depends on `acp-utils` as-is will fail or drag in
   unusable native code.
6. `wisp`'s ACP reducer (`wisp/src/app/acp_reducer.rs`) is coupled to ratatui/TUI
   state (`App`, overlays, screens). There is no platform-agnostic conversation-state
   reducer a browser UI could reuse.

There is intentionally **no** existing wasm scaffolding: no `wasm-bindgen`/`web-sys`/
`js-sys`/`getrandom(js)` deps, no `#[cfg(target_family = "wasm")]` code, no wasm32
target in `rust-toolchain.toml`, no browser WebSocket code in `packages/aether-sdk`
(the SDK is Node-only stdio: `spawn aether acp` + `ndJsonStream`; the reusable part is
only its re-export of `@agentclientprotocol/sdk/experimental/v2` types).

### Success criteria / acceptance conditions

- [ ] Workspace builds and all existing native tests pass on `agent-client-protocol`
      2.2.x with `unstable_protocol_v2` (+ retained unstable features).
- [ ] A new wasm-compatible core crate (working name `aether-acp-core`) compiles for
      `wasm32-unknown-unknown` and for host targets **without** tokio,
      tokio-tungstenite, llm, rmcp, or clankerdiff dependencies.
- [ ] `connect_acp_client` (or its successor) works through a **spawner-generic**
      connection layer: identical behavior on native (tokio) and wasm
      (`wasm-bindgen-futures::spawn_local`); no `Send`-bound relaxation hacks that
      fork from upstream traits.
- [ ] A `web-sys`-backed `ConnectTo` transport speaks the exact wire protocol
      `aether server` expects (text frames = JSON-RPC lines, binary rejected, close
      handshake, 20 s server pings tolerated) and round-trips against the existing
      `AcpWebSocketTestServer` / `aether server`.
- [ ] A `wasm-bindgen` facade exposes a high-level browser API (`connect`, session
      lifecycle, `prompt`, `cancel`, event subscription) returning JS `Promise`s, with
      generated TS types consumable from `packages/`.
- [ ] `wasm-pack test` (node + headless browser) covers connect/initialize,
      session new/resume, prompt/update stream, elicitation, permission auto-approve,
      disconnect/reconnect semantics (single-client 409, cancel-on-disconnect).
- [ ] Docs updated (`acp-utils/README.md`, new crate READMEs, `remote.mdx` browser
      section); no regression in `just ci` (fmt, clippy, nextest, doc-check).

## Technical Approach

### High-level architectural decisions

1. **Bump, don't fork, ACP.** Move the single workspace pin
   (`Cargo.toml:49`) from `=2.1.0` to `2.2.x`. ACP 2.2.0's only wasm-relevant delta is
   the opt-in `wasm_js` feature (`uuid/js` → Web Crypto randomness) plus schema
   1.7→1.9 and stabilization of `unstable_tool_call_name` (now always on in v1;
   still gate v2 with `unstable_protocol_v2`). The `ConnectTo: Send` bound is
   **unchanged** — the plan works *within* it instead of fighting it.
2. **Extract, don't untangle in place.** Leave `acp-utils`'s native client and
   `WebSocketTransport` untouched for `wisp`/`aether-cli`. Create a new small crate
   `crates/acp-core` (name TBD, e.g. `aether-acp-core`) containing only wasm-safe code:
   extension wire types, meta helpers, event enum, spawner trait, spawner-generic
   `connect` function, `web-sys` transport (gated), and the pure conversation reducer.
   `acp-utils` re-exports from it (or depends on it) so native code converges over time
   without a flag-day refactor.
3. **Spawner abstraction over executor choice.** Introduce:
   ```rust
   pub trait Spawner: Clone + Send + Sync + 'static {
       fn spawn(&self, future: impl Future<Output = ()> + Send + 'static);
   }
   ```
   with `TokioSpawner` (`tokio::spawn`) and `WasmSpawner`
   (`wasm_bindgen_futures::spawn_local`, asserting single-threaded use). The client
   driver takes `impl Spawner` instead of calling `tokio::spawn` directly. Channel
   primitives switch from `tokio::sync` to `futures::channel::{mpsc, oneshot}` — the
   same primitives ACP's own `jsonrpc.rs` uses internally, so they are already proven
   wasm-safe.
4. **Message-based WebSocket bridge, not byte-stream shim.** The browser has no
   `AsyncRead/AsyncWrite` socket. Implement `WebSysTransport` directly against
   `agent_client_protocol::Lines` (the `Sink<String> + Stream<io::Result<String>>`
   pair `ConnectTo` adapters accept): outbound `futures::channel::mpsc` → `ws.send()`,
   inbound `ws.onmessage` → `mpsc::UnboundedSender`. No `PollSender`, no manual ping
   (browsers auto-pong; server's 20 s ping just needs an open socket), explicit binary
   rejection to mirror `WebSocketError::BinaryMessage`.
5. **Pure reducer with effects-as-data.** Extract conversation folding
   (`SessionUpdate` → snapshot) into free functions over `schema::v2` types returning
   `(ConversationSnapshot, Vec<ClientEffect>)`, where effects (`RespondElicitation`,
   `AnswerPermission`, `NotifyUi`, …) are data, not TUI calls. `wisp`'s
   `App::on_acp_event` becomes one consumer; the wasm facade serializes snapshots to
   JS. This avoids porting ratatui/crossterm to wasm.
6. **Thin `wasm-bindgen` facade, not a framework.** One new crate
   (`crates/acp-wasm`, TBD `aether-acp-wasm`) with `#[wasm_bindgen]` classes only:
   connection management, request methods → `Promise`, event subscription via
   `js_sys::Function` callbacks, `serde-wasm-bindgen` payload conversion. All protocol
   logic stays in `acp-core` (unit-testable without a browser).

### Design patterns to employ

- **Facade** (`acp-wasm` over `acp-core`), **Adapter** (`WebSysTransport` → `Lines`),
  **Strategy** (`Spawner` impls per platform), **Reducer/effects-as-data** for
  conversation state, **Test builder + Fake** for tests (follow repo convention:
  extend `FakeAgent`/`TestPeer`-style builders, assert on state not mock call counts).
- Keep all public Rust APIs `Send`-compatible (matching upstream `ConnectTo`) so the
  same `acp-core` compiles natively and for wasm without `cfg` forks in business logic;
  isolate `cfg(target_family = "wasm")` to the spawner impl + transport + facade glue.

### Key technical considerations and trade-offs

- **No `Send`-bound removal.** Upstream kept `ConnectTo: Send + 'static` and
  `+ Send` futures in 2.2.0. Do not attempt a non-`Send` fork of the protocol crate;
  on wasm32 everything spawned via `spawn_local` still type-checks as `Send`
  (single-threaded `Send` is vacuous). The issue's "without Send bounds" is achieved
  *operationally* (no thread-pool spawn) rather than by changing trait bounds.
- **`Responder` in `AcpEvent::ElicitationRequest`.** `Responder<T>` is `Send`; keep it
  in the core event enum so native and wasm share the type. The wasm facade must
  answer it from JS (store responder in a `HashMap<elicitation_id, Responder>` on the
  Rust side, expose `respondElicitation(id, result)`).
- **Channels: `futures::channel` vs `tokio::sync`.** `futures::channel::mpsc` is
  already what ACP core uses (`jsonrpc.rs:24`); prefer it in `acp-core`. Capacity and
  backpressure semantics differ from `tokio::sync::mpsc` (bounded 32 in
  `websocket.rs`); preserve the 32-slot bound and `ConnectionClosed`-on-drop behavior
  the TUI relies on (`ConnectionEvents::drop` → emit `ConnectionClosed`).
- **Cancellation.** `tokio_util::CancellationToken` has no wasm-safe equivalent in the
  tree. Replace with a `futures`-based closed-signal (`futures::future::Shared` /
  `oneshot` + `AtomicBool`) owned by `ClientConnection`; keep the `disconnect()`
  contract: abort driver, do **not** send `session/cancel` or `session/close`
  (matches `AcpClientHandle::disconnect` today).
- **`block_task()`.** Never `await` inside a `HandleDispatchFrom` handler (deadlocks
  the dispatch loop — see ACP `jsonrpc.rs` docs). Existing code only calls it from the
  connection-setup closure and `request()` futures; preserve that discipline.
- **Heavy deps stay out of `acp-core`.** `content.rs` depends on `aether-llm`
  (`tokio net`), `elicitation.rs` on `rmcp`, `notifications.rs` on
  `clankerdiff-protocol` + `mcp-utils`. Either duplicate the tiny serde structs needed
  for wasm in `acp-core` or split those modules so the serde-only wire types live in
  `acp-core` and the `llm`/`rmcp` conversions stay in `acp-utils` as extension traits.
  Prefer the split (no drift between two copies of wire types).
- **Server contract (must mirror exactly).** From `aether-cli/src/acp/server.rs`,
  `state.rs`, `remote.mdx`: single attached client (second handshake gets HTTP 409
  `client already attached`); `RemoteServerInfo{cwd, session_id}` advertised in init
  meta; resume-with-replay for history; detach does not stop the turn; pending
  permission/elicitation requests are cancelled on disconnect; server sends 20 s
  pings; raw listener has no auth/TLS (browser deployments need a TLS/auth gateway —
  document, don't build, in this issue).
- **TS surface.** Don't extend the Node-only `@aether-agent/sdk` (spawns processes).
  Ship generated TS bindings from the wasm crate (`wasm-bindgen --target web` /
  `bundler`) plus a hand-written `packages/aether-acp-wasm/` wrapper exposing the same
  `AetherMessage` union shape (`session_update | usage | elicitation_complete | result
  | error`) so a future browser UI can swap transports.

## Implementation Steps

### Phase 0 — Bump ACP to 2.2.x (prerequisite)

1. **Update the workspace pin.** In `Cargo.toml:49`, change
   `agent-client-protocol = { version = "=2.1.0", features = [...] }` to
   `version = "2.2.0"` (or `">=2.2.0, <3"` per repo pinning convention — confirm with
   maintainer; repo currently uses exact `=`), and **drop `unstable_tool_call_name`**
   from the feature list (stabilized in schema 1.8; keeping it is a hard error on
   2.2.0). Keep `unstable_protocol_v2` and `unstable_session_compaction`.
   ```toml
   agent-client-protocol = { version = "2.2.0", features = ["unstable_protocol_v2", "unstable_session_compaction"] }
   ```
2. **Refresh lockfile.** Run `cargo update -p agent-client-protocol` (expect
   `agent-client-protocol-schema` 1.7→1.9, derive updates). Inspect `cargo tree` for
   `uuid` version split (needed for `wasm_js` later).
3. **Fix breaking changes.** Grep for `tool_call_name`-gated APIs and schema renames;
   update `acp-utils`, `wisp`, `aether-cli` call sites. Run `cargo check --workspace
   --all-features`, then `just test` for `acp-utils`, `wisp`, `aether-cli`
   (`acp_remote`, `client_session`, `websocket` suites are the canaries).
4. **Verify native WebSocket round-trip unchanged** (`aether server` ↔ `aether client`
   + `acp-utils/tests/websocket.rs`) before touching anything else.

### Phase 1 — Create `crates/acp-core` (wasm-safe subset, no tokio)

5. **Scaffold `crates/acp-core`.** `cargo new --lib crates/acp-core`
   (`name = "aether-acp-core"`). Deps: `agent-client-protocol` (workspace, with
   `wasm_js` enabled on wasm — see step 6), `futures`, `serde`/`serde_json`,
   `thiserror`, `tracing` (wasm-safe). **No** `tokio`, `tokio-util`,
   `tokio-tungstenite`, `llm`, `rmcp`, `mcp-utils`, `clankerdiff-*`, `utils`. Add
   `getrandom` with `js` feature gated to wasm (transitive via `uuid`; explicit only
   if lockfile shows v3 without js default).
6. **Wire the `wasm_js` feature.** In `acp-core/Cargo.toml`:
   ```toml
   [target.'cfg(target_family = "wasm")'.dependencies]
   agent-client-protocol = { workspace = true, features = ["wasm_js"] }
   ```
   (Workspace declares base features; target-dep adds `wasm_js`. Verify `cargo tree
   -e features -p aether-acp-core` shows `uuid/js`.) Confirm `cargo check -p
   aether-acp-core --target wasm32-unknown-unknown` passes with `rustup target add
   wasm32-unknown-unknown` (CI note: toolchain is 1.98 per `rust-toolchain.toml`).
7. **Move pure wire types into `acp-core`.** Relocate serde-only items, keeping
   conversion impls in `acp-utils`:
   - `meta.rs` (`to_meta`/`from_meta`) — verbatim move (deps: `serde`, `serde_json` only).
   - `config_option_id.rs` — verbatim move (no deps).
   - `config_meta.rs` — move struct defs; `utils::ReasoningEffort` dep is a problem:
     either move the `ReasoningEffort` enum (check `crates/utils` for tokio coupling —
     it has `tokio process`; if the enum itself is pure, extract just the enum) or
     change `reasoning_levels: Vec<String>` in core with a `From` conversion in
     `acp-utils`. Prefer extraction if the enum is dependency-free.
   - `notifications.rs` — **split**: move `_aether/*` request/notification structs +
     `RemoteServerInfo` + `AetherCapabilities` into core; leave `McpNotification`
     display conversions and `clankerdiff`/`mcp_utils` re-exports in `acp-utils` as
     `impl` blocks / `From` impls against core types.
   - `AcpEvent` (`client/event.rs`) — move enum into core; the
     `Responder<CreateElicitationResponse>` field stays (it is `Send + 'static`).
   - `AcpClientError` — move; it wraps `agent_client_protocol::Error` only.
   Re-export everything from `acp-utils` (`pub use aether_acp_core::...`) so existing
   imports (`acp_utils::client::AcpEvent`, `acp_utils::notifications::...`) keep
   working during migration.
8. **Add the `Spawner` trait + impls** in `acp-core/src/spawn.rs`:
   ```rust
   pub trait Spawner: Clone + Send + Sync + 'static {
       fn spawn(&self, fut: impl Future<Output = ()> + Send + 'static);
   }
   #[derive(Clone, Copy, Default)] pub struct TokioSpawner;
   impl Spawner for TokioSpawner {
       fn spawn(&self, fut: impl Future<Output = ()> + Send + 'static) {
           tokio::spawn(fut); // tokio = optional dep, enabled via `tokio-spawn` feature
       }
   }
   #[cfg(target_family = "wasm")] #[derive(Clone, Copy, Default)] pub struct WasmSpawner;
   #[cfg(target_family = "wasm")] impl Spawner for WasmSpawner {
       fn spawn(&self, fut: impl Future<Output = ()> + Send + 'static) {
           wasm_bindgen_futures::spawn_local(fut);
       }
   }
   ```
   Feature layout: `acp-core` features `tokio-spawn = ["dep:tokio"]`,
   `wasm-spawn = ["dep:wasm-bindgen-futures"]`; native consumers enable the former,
   the wasm crate the latter. Unit-test both spawners natively (wasm spawner test runs
   under `wasm-pack test --node` later).
9. **Write the spawner-generic `connect` function** (`acp-core/src/client.rs`),
   ported from `acp-utils/src/client/session.rs` with mechanical substitutions:
   - `tokio::sync::{mpsc, oneshot}` → `futures::channel::{mpsc, oneshot}`;
   - `tokio::spawn` → `spawner.spawn`;
   - `CancellationToken` → `ClosedFlag: Arc<AtomicBool> + futures::future::Shared`-style
     waker (simplest correct: `futures::channel::oneshot` closed-receiver + `AtomicBool`
     for sync `is_closed()` checks; document why not `CancellationToken`);
   - `JoinHandle` abort → store an `AbortHandle` (`futures::future::Abortable` +
     `AbortHandle`; `disconnect()` calls `abort()` then awaits close-signal).
   - Signature:
   ```rust
   pub async fn connect_acp_client<S: Spawner>(
       spawner: S,
       agent: impl ConnectTo<Client> + 'static,
       init_request: InitializeRequest,
   ) -> Result<AcpClient, AcpClientError>
   ```
   - Keep `AcpClientHandle::{prompt, new_session, resume_session,
     resume_session_with_replay, request, cancel, notify, disconnect}` semantics
     identical, including `request()` returning `impl Future + Send` and
     `ConnectionEvents::drop` emitting `AcpEvent::ConnectionClosed`.
   - Keep `ClientHandlers` dispatch chain identical (auto-approve permission logic
     verbatim) so TUI behavior is preserved.
10. **Re-platform `acp-utils` client on core.** Change `acp-utils::client` to call
    `aether_acp_core::client::connect_acp_client(TokioSpawner, ...)` and re-export
    types. All existing `acp-utils` tests (`client_session`, `client_turns`,
    `client_disconnect`, `protocol_transport`) must pass unmodified (they already use
    `Channel::duplex` + `LocalSet`, which exercises the same code path).

### Phase 2 — Pure conversation reducer

11. **Extract reducer** into `acp-core/src/reducer.rs`. Port the *state-folding* half
    of `wisp/src/app/acp_reducer.rs` (`on_session_update` chunk/snapshot projection by
    message id, `ContextCleared` reset, `SubAgentProgress` gating,
    `AuthMethodsUpdated` replacement, `ConnectionClosed` → `ConnectionLost`) into:
    ```rust
    pub struct ConversationSnapshot { /* messages, tool calls, progress, auth methods, connection: ConnectionState */ }
    pub enum ClientEffect { RespondPermission { .. }, PresentElicitation { .. }, NotifyUi(String), GitReviewForward(..), .. }
    pub fn reduce(snapshot: &mut ConversationSnapshot, event: ReducerEvent) -> Vec<ClientEffect>
    ```
    where `ReducerEvent` mirrors `AcpEvent` minus the TUI-owned `Responder` handling
    (elicitation responder is stored by the caller, referenced by id in the effect).
    `wisp` keeps its overlay/screen code; it calls `reduce()` then interprets effects.
12. **Property/unit tests** (native): replay recorded `SessionUpdate` sequences
    (reuse vectors from `wisp/tests/runtime_acp.rs` + `acp-utils/tests/client_turns.rs`)
    and assert snapshot equality; test out-of-order chunks, replay-vs-live mode,
    connection-close mid-turn.

### Phase 3 — `web-sys` WebSocket transport

13. **Implement `WebSysTransport`** in `acp-core/src/transport_wasm.rs`
    (`#[cfg(target_family = "wasm")]`):
    ```rust
    pub struct WebSysTransport { socket: web_sys::WebSocket }
    impl<U: Role> ConnectTo<U> for WebSysTransport {
        async fn connect_to(self, peer: impl ConnectTo<U::Counterpart>) -> Result<(), Error> {
            // outbound: mpsc::channel(32) Sink<String> → ws.send_with_str
            // inbound:  onmessage closure → UnboundedSender<io::Result<String>>
            //   (reject Blob/ArrayBuffer with BinaryMessage-equivalent internal error)
            // onclose/onerror → close inbound sender (drives ConnectionClosed)
            // then ConnectTo::<U>::connect_to(Lines::new(outbound, inbound), peer).await
        }
    }
    ```
    Details: set `binary_type = Arraybuffer` so binary frames are detectable and
    rejected (parity with `WebSocketError::BinaryMessage`); no keepalive writes
    (browser auto-pongs the server's 20 s pings); propagate `CloseEvent` code/reason
    into the internal error for 409 diagnosis (surface HTTP-409-equivalent: a gateway
    should translate the server's 409 into a close code the client maps to
    `ServerOccupied`); backpressure: ` bufferedAmount` check → internal error matching
    native capacity-close behavior.
14. **Native test double.** Add `acp-core` tests using `Channel::duplex()` as the
    `peer` side to prove `Lines`-level framing without a browser; browser behavior
    covered in Phase 5 with `wasm-pack test`.

### Phase 4 — `wasm-bindgen` facade (`crates/acp-wasm`)

15. **Scaffold `crates/acp-wasm`** (`cdylib` + `rlib`, `crate-type = ["cdylib",
    "rlib"]`). Deps: `aether-acp-core` (with `wasm-spawn`), `wasm-bindgen`,
    `wasm-bindgen-futures`, `js-sys`, `web-sys` (features `WebSocket, MessageEvent,
    CloseEvent, ErrorEvent, BinaryType`), `serde-wasm-bindgen`, `serde`, `serde_json`,
    `getrandom` (`js` on wasm), `tracing-web` (optional, for console logs).
16. **Expose the high-level API** (`acp-wasm/src/lib.rs`):
    ```rust
    #[wasm_bindgen] pub struct WasmAcpClient { inner: AcpClientHandle, ... }
    #[wasm_bindgen] impl WasmAcpClient {
        #[wasm_bindgen(js_name = connect)] pub async fn connect(url: String) -> Result<WasmAcpClient, JsValue>;
        //  - opens web_sys::WebSocket, awaits `open`, runs WebSysTransport + connect_acp_client(WasmSpawner, initialize_request())
        //  - initialize_request(): ProtocolVersion::V2 + elicitation caps (mirror wisp Session::initialize_request) + RemoteServerInfo expectation
        #[wasm_bindgen(js_name = newSession)] pub async fn new_session(&self, cwd: String) -> Result<JsValue, JsValue>;
        #[wasm_bindgen(js_name = resumeSession)] pub async fn resume_session(&self, session_id: String) -> Result<JsValue, JsValue>;
        #[wasm_bindgen(js_name = prompt)] pub async fn prompt(&self, session_id: String, text: String) -> Result<JsValue, JsValue>;
        pub fn cancel(&self, session_id: String) -> Result<(), JsValue>;
        pub fn disconnect(&self);
        #[wasm_bindgen(js_name = onEvent)] pub fn on_event(&self, callback: js_sys::Function);
        #[wasm_bindgen(js_name = respondElicitation)] pub fn respond_elicitation(&self, id: String, result: JsValue) -> Result<(), JsValue>;
    }
    ```
    Event delivery: Rust `AcpEvent` → `reducer::reduce` → snapshot patch object via
    `serde-wasm-bindgen::to_value`, invoked as `callback.call1(this, value)`. Prompt
    responses resolve the returned `Promise` on idle `StateUpdate` (same rule as
    `aether-sdk`'s `session.ts`: wait for `isIdle`, then emit `result`), or stream
    patches if the consumer subscribed.
17. **TS packaging.** Add `packages/aether-acp-wasm/` (pnpm workspace): build script
    (`wasm-pack build ../../crates/acp-wasm --target web --out-dir pkg`), hand-written
    `src/index.ts` wrapper typing the `WasmAcpClient` surface with the `AetherMessage`
    union from `aether-sdk/src/types.ts`, `vitest` browser-mode smoke tests. Do not
    wire into `@aether-agent/sdk` (Node-only); document the two-client split in the
    package README.

### Phase 5 — Hardening, docs, CI

18. **Reconnect + multi-tab semantics.** Document and implement minimal client-side
    policy: on `ConnectionClosed`, surface `ConnectionLost` (do not auto-reconnect —
    matches `remote.mdx` "no automatic reconnect"); on connect failure with gateway
    409-equivalent, surface `ServerOccupied`; pending elicitation responders resolve
    with `Error::internal_error()` on drop (parity with `ClientHandlers::emit`).
    `resumeSession` after reconnect replays history (mirror
    `resume_session_with_replay`).
19. **Docs.** Update `crates/acp-utils/README.md` (core vs utils split),
    new `crates/acp-core/README.md` + `crates/acp-wasm/README.md` (browser quickstart:
    `aether server` → `new WasmAcpClient().connect("ws://127.0.0.1:8765")`),
    `packages/website/src/content/docs/aether/running/remote.mdx` (browser-client
    section: gateway/TLS guidance, 409 behavior, no auto-reconnect).
20. **CI.** Add `wasm32-unknown-unknown` target + `wasm-pack test` job
    (node + headless Firefox/Chrome) scoped to `acp-core`/`acp-wasm`; add
    `cargo check -p aether-acp-core --target wasm32-unknown-unknown` to the existing
    lint workflow. `just ci` must stay green on host.

## Testing Plan

### Unit tests (native, `cargo nextest --all-features`)

- **ACP bump canary:** existing `acp-utils/tests/{client_session,client_turns,
  client_disconnect,protocol_transport,websocket}.rs` and
  `aether-cli/tests/integration/acp_remote/*` pass unmodified on 2.2.0.
- **`acp-core` client parity:** duplicate the `connect_acp_client` contract tests
  against the spawner-generic function with `TokioSpawner`: init handshake,
  prompt/response, resume-with-replay, cancel notify, `ConnectionClosed` emission on
  driver drop, elicitation responder error-on-full-channel. Reuse `FakeAgent`/
  `TestPeer` builders (extend, don't fork — they already use `spawn_local`, proving
  the no-thread-pool path works).
- **Reducer:** snapshot-equality tests over recorded update streams (chunked text,
  tool-call snapshots by message id, replay vs live, `ContextCleared` reset,
  `ConnectionClosed` mid-turn → `ConnectionLost`); elicitation/permission produce the
  expected `ClientEffect` without touching TUI types. Test only the public
  `reduce()` API.
- **Transport framing (native):** `Lines`-level test: outbound strings → socket mock,
  inbound `Ok(text)` → ACP messages, binary/close → mapped errors. Assert backpressure
  (32-slot) and graceful-drain behavior match `websocket.rs` (don't return `Ok` while
  buffered outbound remains).

### Integration tests

- **Native E2E (existing harness):** `AcpTestHarness::serve_websocket` +
  `connect_acp_client(TokioSpawner, …)` through `Session::connect_remote_to`-equivalent
  in `acp-core`; single-slot 409, detach/reattach of a running turn, shutdown-cancels
  semantics — mirror `aether-cli/tests/integration/acp_remote/websocket.rs` so any
  divergence between `acp-utils` and `acp-core` clients is caught.
- **Wasm E2E (`wasm-pack test --node`, then headless browser):**
  - connect → initialize → `RemoteServerInfo` present;
  - `newSession` → `prompt` → streamed `SessionUpdate`s → idle `result`;
  - `resumeSession` with replay returns history;
  - elicitation round-trip (`respondElicitation` from JS resolves the Rust
    `Responder`);
  - permission auto-approve preserved;
  - server-close → `ConnectionClosed` event; second-client 409 → `ServerOccupied`.
- **Browser smoke (manual/CI):** minimal HTML page in `packages/aether-acp-wasm/`
  connecting to a local `aether server`, one prompt turn, disconnect/reconnect —
  screenshot/log artifact in CI.

### Edge cases to verify

- Binary WebSocket frames rejected (native closes `Unsupported`; wasm maps to internal
  error) — never silently dropped.
- `send_notification`/`send_request` after close → typed error, not panic.
- `disconnect()` sends neither `session/cancel` nor `session/close` (both transports).
- Elicitation responder dropped with full event channel → requester gets internal
  error, not a hang (existing `emit` behavior).
- 409/second-client, gateway TLS termination (`wss://`), and `bufferedAmount`
  backpressure all surface as actionable JS errors with stable `code` strings.
- No timeouts in tests (repo rule); use channel-close and oneshot rendezvous for
  synchronization. No `anyhow`; new errors are `thiserror` enums
  (`AcpCoreError`, `TransportError`, …).

## Files to Modify/Create

| File | Changes | Add / Modify / Remove |
|------|---------|----------------------|
| `Cargo.toml` (workspace) | Bump `agent-client-protocol` `=2.1.0` → `2.2.0`; drop `unstable_tool_call_name` feature; document `wasm_js` opt-in strategy | Modify |
| `Cargo.lock` | Regenerate via `cargo update -p agent-client-protocol` (schema 1.7→1.9, derive, uuid) | Modify |
| `rust-toolchain.toml` | Optionally add `wasm32-unknown-unknown` target note (or CI-only install) | Modify |
| `crates/acp-core/Cargo.toml` | **New crate** `aether-acp-core`: ACP + futures + serde deps; `tokio-spawn`/`wasm-spawn` features; `target.cfg(wasm)` deps (`wasm-bindgen-futures`, `web-sys`, `getrandom/js`, ACP `wasm_js`) | Add |
| `crates/acp-core/src/lib.rs` | Module root: `client`, `event`, `error`, `spawn`, `meta`, `config_option_id`, `config_meta`, `notifications`, `reducer`, `transport_wasm` | Add |
| `crates/acp-core/src/client.rs` | Spawner-generic `connect_acp_client`, `AcpClient`, `AcpClientHandle`, `ClientHandlers` (ported from `acp-utils/src/client/session.rs` onto `futures::channel` + `Spawner`) | Add |
| `crates/acp-core/src/spawn.rs` | `Spawner` trait + `TokioSpawner` + `WasmSpawner` | Add |
| `crates/acp-core/src/event.rs` | `AcpEvent` (+ `From<UpdateSessionNotification>`) moved from `acp-utils` | Add |
| `crates/acp-core/src/error.rs` | `AcpClientError` moved from `acp-utils` | Add |
| `crates/acp-core/src/meta.rs` | Moved verbatim from `acp-utils/src/meta.rs` | Add |
| `crates/acp-core/src/config_option_id.rs` | Moved verbatim from `acp-utils` | Add |
| `crates/acp-core/src/config_meta.rs` | Moved (with `ReasoningEffort` resolution per step 7) | Add |
| `crates/acp-core/src/notifications.rs` | Serde-only `_aether/*` wire types + `RemoteServerInfo` + `AetherCapabilities` (split from `acp-utils`) | Add |
| `crates/acp-core/src/reducer.rs` | Pure `ConversationSnapshot` + `reduce()` + `ClientEffect` (extracted from `wisp` reducer) | Add |
| `crates/acp-core/src/transport_wasm.rs` | `WebSysTransport: ConnectTo` (`cfg(target_family = "wasm")`) | Add |
| `crates/acp-core/README.md` | Crate purpose, feature flags, spawning model | Add |
| `crates/acp-core/tests/*.rs` | Client parity, reducer, transport framing tests | Add |
| `crates/acp-utils/Cargo.toml` | Depend on `aether-acp-core`; make `tokio`/`tokio-util` client-only as today; keep `websocket` feature for native transport | Modify |
| `crates/acp-utils/src/lib.rs` | Re-export core types (`pub use aether_acp_core::{...}`) | Modify |
| `crates/acp-utils/src/client/session.rs` | Delegate to `acp-core::client` with `TokioSpawner`; keep `connect_acp_client` signature as thin wrapper | Modify |
| `crates/acp-utils/src/notifications.rs` | Keep conversion impls (`llm`/`rmcp`/`clankerdiff`/`mcp_utils`) against core wire types; delete moved structs | Modify |
| `crates/acp-utils/src/content.rs`, `elicitation.rs`, `meta.rs`, `config_meta.rs`, `config_option_id.rs`, `client/event.rs`, `client/error.rs` | Re-export from core or keep as conversion-only shims | Modify |
| `crates/acp-utils/README.md` | Document core/utils split + feature flags | Modify |
| `crates/wisp/src/app/acp_reducer.rs` | Consume `acp-core::reducer::reduce()`; keep overlay/screen interpretation | Modify |
| `crates/acp-wasm/Cargo.toml` | **New crate** `aether-acp-wasm` (`cdylib`+`rlib`, wasm-bindgen, web-sys, serde-wasm-bindgen) | Add |
| `crates/acp-wasm/src/lib.rs` | `WasmAcpClient` facade: connect/session/prompt/cancel/event-subscribe/elicitation-response | Add |
| `crates/acp-wasm/README.md` | Browser quickstart | Add |
| `packages/aether-acp-wasm/package.json`, `src/index.ts` | **New package**: wasm-pack output wrapper with typed `AetherMessage` surface + vitest browser smoke tests | Add |
| `packages/website/src/content/docs/aether/running/remote.mdx` | Browser-client section (gateway/TLS, 409, no auto-reconnect) | Modify |
| `.github/workflows/*.yml` | wasm target + `wasm-pack test` CI job | Modify |

## Additional Notes

- **Documentation updates needed:** crate READMEs (`acp-core`, `acp-wasm`,
  `acp-utils`), `remote.mdx` browser section, new `packages/aether-acp-wasm/README.md`.
  `cargo doc` must stay warning-free (`just doc-check`).
- **Naming is provisional.** `aether-acp-core` / `aether-acp-wasm` / `Spawner` /
  `WebSysTransport` / `WasmAcpClient` are working names; the implementer may rename
  with reviewer approval as long as the native re-export compatibility holds.
- **What this plan deliberately does not include:** server-side auth/TLS gateway,
  auto-reconnect policy beyond surfacing `ConnectionLost`, porting the TUI to the
  browser, or a Node-bridge proxy for `aether acp` stdio (rejected: the WebSocket
  `aether server` endpoint already exists and is the correct browser target).
- **Follow-up tasks likely to spawn:** (1) share more `wisp` conversation logic
  through the pure reducer once the wasm UI needs it; (2) unify `WebSocketTransport`
  (native) onto `acp-core::client` internals to delete the duplicated driver;
  (3) evaluate `agent-client-protocol-tokio` 0.11.x helpers for the native transport
  once on 2.2.x; (4) browser MCP inline-server story (currently Node-only express in
  `aether-sdk/src/mcp/` — out of scope here).
- **Risk register:** (a) `unstable_session_compaction` / v2 schema churn between 1.7
  and 1.9 may rename session fields — contain by doing Phase 0 first and freezing the
  API diff before extraction; (b) `uuid/js` Web-Crypto requires secure context
  (`https://` or `localhost`) — document for `wss://` gateway deployments;
  (c) fastest way to de-risk the whole plan is Phase 0 + a throwaway
  `cargo check -p aether-acp-core --target wasm32-unknown-unknown` spike before
  writing the facade.

---

*Research basis: `crates/acp-utils` (client/session, websocket, notifications,
testing), `crates/wisp` (session, runtime, acp_reducer), `crates/aether-cli`
(server/state/agent/client), `packages/aether-sdk` (Node stdio transport), ACP
2.1.0 vs 2.2.0 registry sources (component `Send` bounds retained; `wasm_js`
changelog #308; `unstable_tool_call_name` stabilized), workspace `Cargo.toml`,
`justfile`, and `remote.mdx` server contract.*
