# Unified Proposal: Complete Deferred MCP Tools Through Real Bash

## Overview

### Problem statement

The `feat/deferred-tools-use-bash` prototype proved the desired product experience: deferred MCP tool schemas stay out of the model's initial context, the model progressively discovers them through `aether mcp`, and calls compose inside real Bash with pipes, redirects, `&&`, command substitution, and scripts. Its implementation was too broad because catalog reads, tool routing, lifecycle control, gateway construction, agent synchronization, and CLI parsing were all pushed through the `McpManager` actor and because the gateway endpoint was created after the coding server.

The current `feat/deferred-tools` branch has already completed the most valuable prerequisites:

- `ToolFilter` and a single `ToolCatalog` have been extracted into `mcp-utils`.
- The catalog already partitions tools into `ModelVisible` and `Deferred` and checks filters/routes.
- Built-in in-memory MCP servers are represented by reusable specs and constructed lazily from `RuntimeServices` during `McpBuilder::spawn`.
- A preliminary `McpHandle` is available to those factories.

Do not redo those changes and do not port the prototype's manager rewrite. Complete the feature by combining the strongest ideas from the two source proposals:

1. Split MCP's data plane from its lifecycle actor and turn `McpHandle` into the sole typed session capability.
2. Keep `ToolCatalog` pure and authoritative; publish catalog/client snapshots through `tokio::sync::watch`.
3. Keep real Bash plus a small `aether mcp` subprocess speaking MCP over a private Unix-domain socket.
4. Bind the gateway before lazy built-in construction so the coding server receives immutable environment overrides directly.
5. Keep gateway and CLI adapters thin, use cached catalog data for discovery, and support JSON-only invocation rather than the prototype's heuristic argument parser.
6. Replace the filesystem proxy and `proxy__call_tool` outright; there is no compatibility requirement.

### Success criteria and acceptance conditions

- Deferred tools and their schemas are absent from the model-visible tool definitions; no synthetic `proxy__call_tool` is added.
- `aether mcp --help`, `aether mcp <server> --help`, and `aether mcp <server> <tool> --help` progressively reveal only connected, allowed, deferred servers/tools and the complete input schema.
- A deferred tool can be called as `aether mcp <server> <tool> --json '{...}'` or with a JSON object on stdin, and its JSON result composes with `jq`, pipelines, redirects, `&&`, command substitution, and scripts under real `bash -c`.
- Model-visible tools are rejected through the deferred route, deferred tools are rejected through the model-visible route, and the same `ToolFilter` decision governs discovery and execution.
- The gateway and CLI depend on `McpHandle`/snapshot APIs, never on `McpManager` or its private actor command enum.
- One `ToolCatalog` drives model-visible definitions, deferred discovery, instructions, statuses, and route authorization. Help requests do not call upstream `tools/list`.
- General MCP `notifications/tools/list_changed` refreshes that one catalog and updates both model-visible and deferred projections.
- The socket is session-scoped, private (`0700` parent directory), below Unix socket path limits, and removed on session shutdown. Only the socket path and Aether executable directory are injected into Bash; Aether-owned OAuth credentials are not.
- Dropping/disconnecting the CLI cancels its nested MCP call; MCP Task completion is reduced to the final tool result correctly.
- `McpCommand` is private and contains lifecycle/control operations only. Direct tool calls, prompts, and status/catalog reads no longer pay actor/oneshot/`JoinSet` overhead.
- Agent tool/instruction synchronization is implemented once in `McpSession`, not separately in ACP, headless, or subagent hosts.
- The old `$AETHER_HOME/tool-proxy` writer, `tool_proxy.rs`, `proxy__call_tool`, proxy instructions, and proxy tests are removed.

## Technical Approach

### 1. One catalog, separate control and data planes

Keep `McpManager` as the control plane for connection/authentication/reconnection/shutdown and catalog mutation. Publish an immutable data-plane snapshot whenever connection state, tools, instructions, or status changes:

```rust
#[derive(Clone)]
pub struct McpSnapshot {
    catalog: Arc<ToolCatalog>,
    clients: Arc<HashMap<String, Arc<RunningService<RoleClient, McpClient>>>>,
}

#[derive(Clone)]
pub struct McpHandle {
    control_tx: mpsc::Sender<ManagerCommand>,
    snapshot_rx: watch::Receiver<Arc<McpSnapshot>>,
}
```

`ToolCatalog` remains free of transport handles. `McpSnapshot` combines that pure policy/catalog projection with the current server clients for routing. The manager owns a `watch::Sender<Arc<McpSnapshot>>`; every handle owns a cheap receiver clone. Snapshot methods return catalog projections and resolve a route against the same snapshot, preventing a policy/client time-of-check/time-of-use split.

Change `ToolRoute` to encode intent and identity rather than using a bare enum:

```rust
pub enum ToolRoute {
    ModelVisible { namespaced_name: String },
    Deferred { server: String, tool: String },
}
```

`McpSnapshot::resolve(route, arguments)` must:

1. Form/parse the namespaced name.
2. Require a connected catalog entry.
3. Call `ToolCatalog::route_permitted` so exposure and `ToolFilter` are checked together.
4. Resolve the client from the same snapshot.
5. Return `CallToolRequestParams` with the MCP-local tool name and supplied JSON object.

A snapshot can temporarily keep an `Arc<RunningService>` alive, so shutdown must first publish a snapshot without the server and then explicitly cancel/close the removed running service. New calls fail resolution immediately; in-flight calls terminate rather than talking to a zombie connection.

### 2. A typed `McpHandle`, not a public actor protocol

Move the current `McpHandle` into a focused `mcp_handle.rs` module and expose methods such as:

```rust
impl McpHandle {
    pub fn snapshot(&self) -> Arc<McpSnapshot>;
    pub fn subscribe(&self) -> watch::Receiver<Arc<McpSnapshot>>;
    pub fn call(
        &self,
        route: ToolRoute,
        arguments: Map<String, Value>,
        options: CallToolOptions,
    ) -> ToolCallStream;
    pub async fn list_prompts(&self) -> Result<Vec<Prompt>, McpHandleError>;
    pub async fn get_prompt(
        &self,
        name: &str,
        arguments: Option<Map<String, Value>>,
    ) -> Result<GetPromptResult, McpHandleError>;
    pub async fn authenticate_server(&self, name: &str) -> Result<(), McpHandleError>;
}
```

`call`, `list_prompts`, and `get_prompt` resolve clients from the current snapshot and call the existing `call_tool`/rmcp APIs directly. Preserve streaming progress, cancellation, task events, timeout, and trace metadata in `CallToolOptions`. Normalize lookup/policy/transport failures in a specific `McpHandleError` enum.

After consumers migrate, make `ManagerCommand` private and leave only authentication/lifecycle commands that mutate manager-owned state. Remove `ExecuteTool`, `ListPrompts`, `GetPrompt`, and `GetServerStatuses` from the actor, along with the tool-execution `JoinSet` and oneshot plumbing in `run_mcp_task`.

### 3. Centralized session-to-agent synchronization

Evolve `McpSpawnResult` into `McpSession`. It owns the `McpRuntime`, host-facing event receiver, and catalog subscription. `McpSession::connect_agent(agent_tx)` runs one synchronization task that:

- Watches `McpSnapshot` changes.
- Sends `AgentCommand::UpdateTools` from `catalog.tools().model_visible`.
- Diffs/sends `AgentCommand::UpdateMcpInstructions` from `catalog.model_instructions()`.
- Keeps the latest snapshot available from `McpRuntime`.
- Forwards only host-facing events (statuses, OAuth failures, elicitation, connection-ready) to ACP/TUI/headless consumers.

This removes the catalog/snapshot mutation logic from `aether-cli/src/acp/agent_runtime.rs` and gives subagents the same behavior. Do not make gateway implementation conditional on connecting an agent; the session's snapshot/router remains useful to headless callers and external adapters.

### 4. Cached discovery plus general catalog refresh

Gateway discovery must read the cached catalog populated at connection time. Do not re-fetch every deferred server during each help request, and do not create feature-specific before/after catalog deltas.

Implement `notifications/tools/list_changed` as a general manager capability:

- `McpClient::on_tool_list_changed` sends an internal refresh request containing the server identity.
- `run_mcp_task` starts the server's `list_tools` call outside the control loop.
- Completion is applied only if the result still belongs to the current client (use `Arc::ptr_eq` or a connection generation token to discard stale reconnect results).
- A successful result atomically replaces that server's catalog tools, republishes `McpSnapshot`, and thereby updates both direct agent definitions and deferred gateway discovery.
- A failed refresh logs a warning and retains the last healthy catalog entry.

This preserves responsiveness without coupling refresh to the deferred gateway.

### 5. MCP-over-UDS gateway with early binding

Reuse the prototype's transport concept, not its manager plumbing. Add a generic `UnixSocketMcpTransport` in `mcp-utils`:

- Allocate `${XDG_RUNTIME_DIR}/aether/aether-<short-uuid>/ipc.sock`, falling back to `temp_dir()` when needed.
- Create the session directory with mode `0700` and verify generated paths fit `sockaddr_un.sun_path` before binding.
- Parse only absolute inherited paths from `AETHER_MCP_IPC_SOCKET`.
- Serve each accepted rmcp connection in its own task.
- Use RAII guards to abort the accept loop and remove the socket and session directory on drop.

Bind this transport in `McpBuilder::spawn` after the handle/watch channels exist but before `resolve_servers` invokes any lazy in-memory factory. Extend `RuntimeServices` with immutable shell environment entries (gateway socket when deferred tools exist; empty otherwise). Start the gateway after `McpManager` is registered/spawned. The gateway is optional when there are no deferred tools and must not require a progressive-instruction renderer to exist.

The Aether-specific gateway `ServerHandler` belongs in `aether-core` and is a thin adapter over `McpHandle`:

- `tools/list`: project cached, allowed deferred tools from `handle.snapshot()` and preserve description, full schema, and annotations.
- `_aether_list_servers`: a private adapter tool used by the CLI to return deferred server names/descriptions.
- `tools/call`: split `server__tool`, call `McpHandle::call(ToolRoute::Deferred { ... })`, and reduce `Complete` or `TaskComplete` to the final `CallToolResult`.
- Cancel the `CancellationToken` from a drop guard if the request future/client connection disappears.
- Emit structured tracing fields for route, server, tool, outcome, and duration, but never arguments, result content, credentials, or auth headers.

The adapter owns no filtering, OAuth, refresh, instructions, or connection state.

### 6. Immutable Bash environment and real shell semantics

Add an immutable `BashEnvironment` to the coding server. Build it in the lazy coding factory from:

- A `PATH` with the current `aether` executable directory prepended once.
- `RuntimeServices`' gateway environment entries.

Pass it through `DefaultCodingTools::with_bash_environment` to `execute_command_in_dir_with_env`. There is no shared `RwLock`, late `extend_environment` callback, shell parsing, or command interception.

Land Bash process-group cancellation as a small independent prerequisite: start `bash -c` in its own process group, retain `kill_on_drop`, and kill the group on timeout/drop. This ensures cancelling the outer Bash tool also removes a nested `aether mcp` process, whose disconnected gateway request then cancels the underlying MCP call. Preserve existing permission validation and output behavior.

Do not adopt Brush as part of this feature. An embedded interpreter would change the most-used execution engine and would still need the UDS path for scripts that launch real Bash.

### 7. Progressive CLI with deliberately small argument grammar

Preserve the prototype's useful layered discovery UX while dropping its ambiguous coercion/parser surface:

```text
aether mcp --help
aether mcp <server> --help
aether mcp <server> <tool> --help
aether mcp <server> <tool> --json '{"state":"open"}'
printf '%s' '{"state":"open"}' | aether mcp <server> <tool>
```

Rules:

- Empty arguments mean `{}`.
- Accept exactly one JSON object from `--json` or non-terminal stdin; reject arrays/scalars.
- `--json` and piped stdin are mutually exclusive.
- Do not add `key=value`, `--key value`, heuristic JSON coercion, standalone connection configuration, or completion in the initial implementation.
- Help uses a bounded discovery timeout. Calls use `--timeout <seconds>` (default 600) plus a small client-side shutdown allowance.
- Print one JSON value to stdout. Prefer `structured_content`; otherwise serialize the content blocks. Print diagnostics to stderr.
- Return exit code `2` for CLI usage/schema-input errors and `1` for unavailable session, transport, timeout, or tool errors.
- Outside an active session, report that `aether mcp` requires the inherited `AETHER_MCP_IPC_SOCKET`; never search globally for sockets.

Render progressive-discovery model instructions from the same catalog server summaries under the reserved instruction key `progressive-discovery`. The text must describe the JSON-only grammar and shell composition examples. Reject a real server named `progressive-discovery` when deferred tools are configured so instruction ownership is unambiguous.

### Trade-offs

- **Real Bash + subprocess + UDS** adds a tiny process/IPC cost, but it is the only simple design that faithfully supports arbitrary shell composition and scripts without reimplementing shell semantics.
- **Cached catalog discovery** may briefly show the last healthy definitions after an upstream refresh failure, but it keeps help deterministic and fast. General `list_changed` handling closes the freshness gap for compliant servers.
- **Snapshots holding clients** make reads/routing lock-free and eliminate actor tax. Explicit close-on-removal is required to avoid stale clients extending connection lifetime.
- **JSON-only invocation** is less friendly for manual typing than the prototype's four input syntaxes, but is predictable for agents, much smaller, and composes naturally with `jq`.
- **Unix sockets** intentionally target Unix platforms for this implementation. A Windows transport is a follow-up rather than a reason to invent a custom cross-platform protocol now.

## Phased Delivery

Implement and review the proposal in the following phases. Each phase must leave the branch green and can be landed independently before starting the next one.

### Phase 1: Complete the MCP runtime capability boundary

**Scope:** Implementation Steps 1–2.

- Add `McpSnapshot` and publish it through `watch`.
- Finish the typed `McpHandle` facade.
- Migrate direct tool calls and prompt consumers away from `Sender<McpCommand>`.
- Make the actor protocol private and control-plane-only.

**Exit gate:** Existing model-visible tools, prompts, authentication, cancellation, MCP Tasks, and trace propagation behave unchanged; no consumer outside the MCP runtime imports `McpCommand`; snapshot/route tests pass. Keep the existing filesystem proxy working during this phase so this is a behavior-preserving architecture refactor.

### Phase 2: Centralize synchronization and catalog freshness

**Scope:** Implementation Steps 3–4.

- Introduce `McpSession::connect_agent` and migrate ACP, headless, and subagent hosts.
- Implement general `notifications/tools/list_changed` refresh against the one catalog.
- Remove duplicated host-side tool/instruction snapshot logic.

**Exit gate:** One session implementation updates agent tools/instructions; hosts still receive status/OAuth/elicitation/readiness events; successful, failed, and stale refresh tests pass; the existing proxy still works from refreshed catalog state.

### Phase 3: Land reusable Unix and Bash prerequisites

**Scope:** Implementation Steps 5–6.

- Add the generic secure rmcp-over-UDS transport.
- Add immutable coding Bash environment overrides and PATH injection.
- Land process-group cancellation as its own reviewable commit.

**Exit gate:** Generic UDS tests pass independently of deferred tools; Bash permission behavior is unchanged; timeout/drop leaves no child process group; standalone coding MCP receives no session socket.

### Phase 4: Add deferred gateway behavior behind the existing configuration

**Scope:** Implementation Step 7.

- Bind the gateway before lazy built-in factories.
- Inject its immutable endpoint into the coding server.
- Add the thin `McpHandle`-backed gateway service and lifecycle/cancellation auditing.
- Initially use the current proxied/deferred catalog partition internally, without yet renaming public configuration.

**Exit gate:** Deferred discovery/calls work over UDS, model-visible routes are rejected, filters and MCP Tasks work, disconnect cancels calls, and the gateway/socket disappear with the session. The old `proxy__call_tool` path may remain temporarily but must not be used by the new gateway tests.

### Phase 5: Add and validate the progressive `aether mcp` UX

**Scope:** Implementation Step 8.

- Add layered help and JSON-only invocation to the CLI.
- Add progressive-discovery instructions.
- Prove real Bash composition end to end.

**Exit gate:** Real-binary integration tests cover help, `--json`, stdin, exit codes, and unavailable sessions; coding Bash tests cover `jq`, pipelines, redirects, `&&`, command substitution, and scripts; only the socket path/PATH are inherited.

### Phase 6: Cut over and remove the filesystem proxy

**Scope:** Implementation Step 9.

- Rename public configuration/status/UI concepts from proxy to deferred.
- Switch docs/examples/settings to `deferTools`.
- Delete `proxy__call_tool`, disk catalog generation, `tool_proxy.rs`, and obsolete tests.

**Exit gate:** No supported deferred path writes under `$AETHER_HOME/tool-proxy`; no synthetic proxy tool is model-visible; configuration/UI/documentation consistently use deferred terminology; repository searches find no feature-related legacy proxy references.

### Phase 7: Final hardening and release gate

**Scope:** Implementation Step 10 and the full Testing Plan.

- Run all architecture searches, integration/regression suites, formatting, and linting.
- Verify OAuth isolation, socket privacy/cleanup, concurrent clients, stale refresh handling, subagent endpoint isolation, and structured nested-call audit events.
- Review the final dependency direction and complexity budget.

**Exit gate:** `just fmt`, `just test`, and `just lint` pass; all success criteria are demonstrated by public-API/integration tests; no second catalog, public actor enum, mutable late environment callback, custom IPC protocol, or per-host synchronization implementation remains.

## Implementation Steps

1. **Introduce the snapshot/router data plane while preserving current proxy behavior.**
   - Add `McpSnapshot` beside `ToolCatalog` in `mcp-utils`; keep client handles in a separate map from the pure catalog.
   - Add route payloads to `ToolRoute` and implement `McpSnapshot::resolve` plus public catalog projection helpers needed by handles/gateway.
   - Create the watch channel in `McpBuilder::spawn`, inject its sender into `McpManager`, and publish after pending registration, connection/auth transitions, tool replacement, server removal, and shutdown.
   - Replace `McpConnectionDetails`' duplicated `instructions`, `tool_definitions`, and `server_statuses` storage with an `Arc<McpSnapshot>` and projection methods, or replace the type entirely with `McpSnapshot` if call sites remain clearer.
   - On server shutdown, publish removal before explicitly cancelling/closing the old client.
   - Add public-API tests proving snapshots are immutable, a new snapshot is observed after changes, stale routes fail after removal, and direct/deferred route checks use the same catalog decision.

2. **Finish `McpHandle` and remove actor tax.**
   - Move `McpHandle` and `McpHandleError` to `crates/aether-core/src/mcp/mcp_handle.rs`.
   - Implement `snapshot`, `subscribe`, route-aware `call`, `list_prompts`, `get_prompt`, and `authenticate_server` as described above.
   - Return a stream from `call` so `Agent` can insert it directly into its `StreamMap`; convert resolution errors into a one-item failed stream.
   - Migrate `Agent`/`AgentBuilder`, CLI runtime, slash-command prompt expansion, examples, test helpers, and the subagent spawner from `Sender<McpCommand>` to cloned `McpHandle`.
   - Make `McpCommand` (renamed `ManagerCommand`) private with only control-plane variants. Delete execute/list/status variants, response channels, and `tool_executions` from `run_mcp_task`.
   - Keep existing cancellation, timeout, MCP Task, progress, trace-context, and result conversion tests passing through the new public handle API.

3. **Centralize MCP-to-agent synchronization in `McpSession`.**
   - Rename/evolve `McpSpawnResult` to `McpSession` and add `connect_agent`, `split`, `block_until_ready`, `handle`, and gateway endpoint accessors.
   - Implement one snapshot watch task that updates agent tools/instructions and retains the latest snapshot; forward only host events.
   - Remove `on_mcp_event`'s tool/instruction/snapshot bookkeeping from ACP `AgentRuntime`; have normal, ready, headless, ACP, and subagent construction use `McpSession::connect_agent`.
   - Add integration tests that a connected agent receives initial and updated tools/instructions while the host receives statuses/readiness but not duplicate internal catalog updates.

4. **Add general `tools/list_changed` catalog refresh.**
   - Add a private manager notification channel and `McpClient::on_tool_list_changed` hook.
   - Refresh the notifying server outside the actor loop; tag the work with the source client/generation.
   - Apply only non-stale successful results through the existing `ServerCatalogEntry`/`ToolFilter` construction path, retain the old entry on failure, and republish once.
   - Test that refresh does not block authentication/control dispatch, failed refresh preserves the healthy snapshot, stale reconnect results are ignored, and one refresh changes both direct and deferred projections.

5. **Add and validate the generic Unix-socket MCP transport.**
   - Create `mcp-utils/src/tool_gateway/mod.rs` and `transport.rs` with `UnixSocketPath`, `AETHER_MCP_IPC_SOCKET`, `_aether_list_servers`, secure directory creation, rmcp serving, and RAII cleanup.
   - Add required `uuid`/Tokio feature flags in `mcp-utils/Cargo.toml` and `Cargo.lock`.
   - Add transport integration tests for unique sockets, malformed/relative paths, MCP list/call round trips, `0700` permissions, path length, concurrent clients, and cleanup.

6. **Make Bash environment injection immutable and harden process cancellation.**
   - Add `BashEnvironment` and `execute_command_in_dir_with_env`; store the environment in `DefaultCodingTools` and expose `with_bash_environment`.
   - Extend `RuntimeServices` with the gateway's shell environment. In the lazy coding factory, combine it with a PATH containing the current Aether binary directory before constructing `CodingMcp`.
   - Wire standalone `mcp-servers-stdio` with PATH only and no session socket.
   - In a separate, reviewable change, place Bash in a process group and kill the group on timeout/drop; add the minimal `nix` process/signal features if required.
   - Test immutable overrides, PATH de-duplication/prepending, no socket outside deferred sessions, inherited socket inside deferred sessions, unchanged permission decisions, and child-process cleanup.

7. **Implement the thin deferred gateway over `McpHandle`.**
   - Add `deferred_tool_gateway.rs` (transport lifecycle) and `gateway_service.rs` (rmcp `ServerHandler`) in `aether-core/src/mcp`.
   - Bind before `resolve_servers`, pass environment through `RuntimeServices`, start after the manager task, and retain the handle in `McpRuntime`.
   - Project server/tool discovery from the cached catalog; preserve full schemas/annotations; execute only `ToolRoute::Deferred`.
   - Aggregate streaming/task events to one MCP response, cancel on request drop/disconnect, and add content-free structured audit fields.
   - Do not fail session construction merely because no progressive-instruction renderer is installed.
   - Add gateway integration tests for discovery/call, selective exposure, direct-route rejection, tool filters, Task completion, disconnect cancellation, concurrent slow calls, socket lifetime, and absence when no tools are deferred.

8. **Add the JSON-only `aether mcp` CLI and progressive instructions.**
   - Create `aether-cli/src/mcp_command.rs`, export it from `lib.rs`, and add the `Mcp` subcommand/error mapping in `main.rs`.
   - Connect only through the inherited socket and implement layered help, `--json`/stdin parsing, bounded timeouts, JSON stdout, stderr diagnostics, and exit codes.
   - Add the instruction renderer in `mcp-servers/src/setup.rs`; configure it during built-in registration and ensure its examples match the implemented grammar exactly.
   - Add subprocess integration tests using `CARGO_BIN_EXE_aether` and a fake Unix gateway for every help layer, empty/`--json`/stdin calls, malformed and conflicting input, runtime/tool failures, timeout, and invocation outside a session.
   - Add an end-to-end coding Bash test for pipes to `jq`, redirects, `&&`, command substitution, and executing a generated script containing `aether mcp`.

9. **Switch the public feature from proxy terminology to deferred tools and delete the legacy implementation.**
   - Rename `ToolExposure::{Direct, Proxied}` to `{ModelVisible, Deferred}`, `ToolProxyRules` to `DeferredToolRules`, and helpers to `deferred_all`, `has_deferred_tools`, and `defer_all_tools`.
   - Change server and MCP source configuration from `proxy` to `deferTools` with boolean-or-rules semantics; do not retain aliases or fallbacks.
   - Rename status/UI fields from `proxied` to `deferred_tools` (`deferTools` on the wire) and labels to “Model-visible”/“Deferred”.
   - Reserve `progressive-discovery`, not `proxy`, for generated instructions.
   - Delete `tool_proxy.rs`, all directory writes/cleanup, proxy resolution branches, `with_aether_home` plumbing used only by proxy discovery, and `tool_proxy_tests.rs`.
   - Update `mcp.json`, READMEs, examples, settings docs, and website pages to explain deferred schemas, progressive Bash discovery, JSON-only calls, credential isolation, and tool-filter semantics.

10. **Run final architecture and regression gates.**
    - Use LSP diagnostics during each phase, then run `just fmt`, `just test`, and `just lint`.
    - Search for remaining feature uses of `proxy`, `proxied`, `proxy__call_tool`, `$AETHER_HOME/tool-proxy`, and public `McpCommand`; distinguish unrelated HTTP/provider proxy terminology.
    - Confirm the final dependency direction: `mcp-utils` owns catalog/filter/client snapshot and generic UDS transport; `aether-core` owns session handle/router adapter; `mcp-servers` owns coding Bash environment and instructions; `aether-cli` owns parsing/rendering only.
    - Review production churn against the complexity budget: no second catalog, no feature-specific catalog delta loop, no mutable post-construction Bash callback, no custom IPC protocol, and no broad per-host synchronization logic.

## Testing Plan

### Unit/public API tests

- **`ToolCatalog`/`McpSnapshot`:** connected-only discovery; complete schema and annotations; model-visible/deferred partition; allow/deny filters; route mismatch; immutable old snapshots; removed client behavior; instruction/status projections.
- **`McpHandle`:** model-visible and deferred calls; invalid JSON object/name; unavailable manager/client; timeout/cancel; task completion; prompts; auth control operation; trace metadata propagation.
- **Refresh:** successful, failed, stale, and concurrent `list_changed` outcomes without blocking the control loop.
- **CLI parser:** `{}` default; `--json`; stdin object; scalar/array rejection; duplicate input source; malformed JSON; exact exit-code classification; structured-content and content-block output.
- **Bash environment:** PATH prepend/de-duplication, immutable overrides, process-group cleanup, and unchanged permissions.

### Integration tests

- Replace filesystem proxy tests with `aether-core/tests/mcp/ipc_tests.rs` covering the real rmcp-over-UDS boundary.
- Add `agent_sync_tests.rs` for the `McpSession` public API.
- Add `mcp-utils/tests/tool_gateway_ipc.rs` for socket transport/security/lifecycle.
- Add `aether-cli/tests/integration/mcp_command.rs` that launches the real Aether binary against a fake gateway.
- Add coding-server integration coverage that launches real Bash and the real `aether mcp` subprocess, then verifies `jq`, pipeline, redirect, `&&`, command substitution, and script composition.
- Re-run existing OAuth, tool Task, trace context, instructions, config merge, coding permission, ACP, headless, and subagent integration suites after migration.

### Required edge cases

1. No deferred servers: no socket environment, no gateway, no progressive instruction.
2. Deferred server connecting/failed/needs OAuth: hidden from discovery until connected, but status remains host-visible.
3. Selective exposure: model-visible and deferred tools on one server remain mutually exclusive by route.
4. Tool denied by the agent filter: absent from both discovery and execution.
5. Unknown server/tool and reserved-name collision: deterministic typed errors.
6. Tool list changes while a help request or call is active: old snapshot remains valid for that operation; the next read sees the new snapshot.
7. Server removal/reconnect during a call: old connection closes, stale refresh is discarded, new calls use only the new snapshot.
8. Multiple simultaneous CLI clients and one slow call: other help/call requests continue.
9. CLI or Bash cancellation: process group exits, socket disconnects, nested cancellation token fires, and no orphan process/tool task remains.
10. MCP Task events: progress/status are tolerated and `TaskComplete` produces the final JSON.
11. Tool result has no structured content or returns `is_error`: fallback is valid JSON and errors produce nonzero exit.
12. Socket setup partially fails: temporary paths are cleaned and session startup reports a specific error.
13. Managed OAuth credentials/auth headers never appear in the Bash override list, CLI arguments, logs, or audit fields.
14. Subagent runtimes receive their own session socket and do not reuse a parent session endpoint accidentally.

## Files to Modify/Create

| Path | Change | Action |
|---|---|---|
| `crates/mcp-utils/src/client/tool_catalog.rs` | Finish catalog projection APIs, payload-bearing `ToolRoute`, deferred naming, and instruction/status projection. | Modify |
| `crates/mcp-utils/src/client/mcp_snapshot.rs` | Add immutable catalog + client data-plane snapshot and route resolution. | Add |
| `crates/mcp-utils/src/client/manager.rs` | Publish snapshots; keep lifecycle/catalog mutation; remove proxy/file operations; apply safe refresh and explicit connection close. | Modify |
| `crates/mcp-utils/src/client/mcp_client.rs` | Send internal `tools/list_changed` refresh notifications. | Modify |
| `crates/mcp-utils/src/client/connection.rs` | Pass refresh control channel/client identity and expose explicit connection cancellation needed by manager shutdown. | Modify |
| `crates/mcp-utils/src/client/config.rs` | Rename public exposure/config types to deferred terminology and `deferTools`; add helpers used before gateway binding. | Modify |
| `crates/mcp-utils/src/client/error.rs` | Replace proxy errors with route/snapshot/gateway-specific errors. | Modify |
| `crates/mcp-utils/src/client/mod.rs` | Export snapshot, deferred config, and route APIs; stop exporting proxy APIs. | Modify |
| `crates/mcp-utils/src/client/tool_proxy.rs` | Remove filesystem catalog, synthetic proxy tool, and proxy instruction implementation. | Remove |
| `crates/mcp-utils/src/status.rs` | Rename `proxied` status to `deferred_tools`/`deferTools`. | Modify |
| `crates/mcp-utils/src/tool_gateway/mod.rs` | Define socket environment/path/server-description protocol constants and exports. | Add |
| `crates/mcp-utils/src/tool_gateway/transport.rs` | Implement secure rmcp-over-UDS listener, per-client tasks, and cleanup guards. | Add |
| `crates/mcp-utils/src/lib.rs` | Export the Unix gateway module under the client feature/Unix cfg. | Modify |
| `crates/mcp-utils/Cargo.toml` | Add UUID and required Tokio features for Unix socket transport/tests. | Modify |
| `crates/mcp-utils/tests/tool_catalog.rs` | Extend catalog/snapshot/filter/route and deferred naming tests. | Modify |
| `crates/mcp-utils/tests/tool_gateway_ipc.rs` | Add UDS round-trip, security, concurrency, parse, and cleanup tests. | Add |
| `crates/aether-core/src/mcp/mcp_handle.rs` | Implement the typed session capability and specific handle errors. | Add |
| `crates/aether-core/src/mcp/mcp_builder.rs` | Create watch/control channels, bind gateway before factories, add immutable runtime environment, and evolve spawn result into `McpSession`. | Modify |
| `crates/aether-core/src/mcp/run_mcp_task.rs` | Restrict private actor to control/lifecycle and add nonblocking general refresh completion handling. | Modify |
| `crates/aether-core/src/mcp/deferred_tool_gateway.rs` | Own gateway bind/start/endpoint/lifetime around the generic transport. | Add |
| `crates/aether-core/src/mcp/gateway_service.rs` | Implement the thin rmcp `ServerHandler` over `McpHandle`. | Add |
| `crates/aether-core/src/mcp/mod.rs` | Export `McpHandle`, `McpSession`, and gateway public API; hide actor internals. | Modify |
| `crates/aether-core/src/core/agent.rs` | Store `McpHandle` and consume its call stream directly. | Modify |
| `crates/aether-core/src/core/agent_builder.rs` | Accept a typed handle rather than `Sender<McpCommand>`. | Modify |
| `crates/aether-core/src/agent_spec.rs` | Rename MCP file-source `proxy` to `defer_tools`/`deferTools`. | Modify |
| `crates/aether-core/src/testing/mcp_test.rs` | Expose public handle/session/deferred-server test helpers and remove proxy temp-home behavior. | Modify |
| `crates/aether-core/src/testing/fake_mcp.rs` | Support catalog refresh and cancellation assertions through fake in-memory servers. | Modify |
| `crates/aether-core/src/testing/utils.rs` | Migrate test agent setup to `McpHandle`/`McpSession`. | Modify |
| `crates/aether-core/examples/mcp_agent.rs` | Demonstrate typed handle/session setup. | Modify |
| `crates/aether-core/tests/mcp.rs` | Register IPC and agent-sync test modules; unregister proxy tests. | Modify |
| `crates/aether-core/tests/mcp/tool_proxy_tests.rs` | Delete obsolete filesystem proxy tests. | Remove |
| `crates/aether-core/tests/mcp/ipc_tests.rs` | Add deferred gateway discovery/routing/lifecycle/refresh/security tests. | Add |
| `crates/aether-core/tests/mcp/agent_sync_tests.rs` | Add centralized session-agent synchronization tests. | Add |
| `crates/aether-core/tests/mcp/{config_parser_tests,instructions_tests,oauth_tests,task_tests,trace_context_tests}.rs` | Migrate terminology/handle setup and retain regression coverage. | Modify |
| `crates/mcp-servers/src/coding/tools/bash/mod.rs` | Add immutable environment overrides and process-group cancellation. | Modify |
| `crates/mcp-servers/src/coding/default_tools.rs` | Store/pass `BashEnvironment`. | Modify |
| `crates/mcp-servers/src/coding/error.rs` | Add precise process capture/environment errors if needed. | Modify |
| `crates/mcp-servers/src/setup.rs` | Build coding server from `RuntimeServices` gateway env and register progressive instructions. | Modify |
| `crates/mcp-servers/src/bin/stdio.rs` | Prepend Aether executable PATH without exposing a session socket. | Modify |
| `crates/mcp-servers/src/docs/mcp_builder_ext.md` | Document lazy environment and progressive-discovery wiring. | Modify |
| `crates/mcp-servers/src/subagents/tools/spawn_subagent/mod.rs` | Use `McpSession`/`McpHandle` and each subagent's own gateway. | Modify |
| `crates/mcp-servers/tests/integration/test_bash.rs` | Test environment/PATH/process-group and real shell composition. | Modify |
| `crates/mcp-servers/tests/integration/plugins_mcp_agents.rs` | Migrate direct manager-command tests to public handle APIs. | Modify |
| `crates/mcp-servers/Cargo.toml` | Add Unix process/signal features used for process-group cleanup. | Modify |
| `crates/aether-cli/src/mcp_command.rs` | Implement inherited-socket layered help and JSON-only calls. | Add |
| `crates/aether-cli/src/lib.rs` | Export the MCP command module/API. | Modify |
| `crates/aether-cli/src/main.rs` | Add `aether mcp` subcommand and exit-code mapping. | Modify |
| `crates/aether-cli/src/runtime.rs` | Build/connect agents through `McpSession` and pass `McpHandle`. | Modify |
| `crates/aether-cli/src/acp/agent_runtime.rs` | Remove duplicated catalog synchronization and use session/handle methods. | Modify |
| `crates/aether-cli/src/acp/session_actor.rs` | Consume host-only MCP events/new snapshot accessors. | Modify |
| `crates/aether-cli/src/headless/run.rs` | Use typed prompt methods/handle rather than actor commands. | Modify |
| `crates/aether-cli/src/slash_commands.rs` | Call `McpHandle::list_prompts/get_prompt`. | Modify |
| `crates/aether-cli/src/acp/testing.rs` | Migrate runtime test setup to handles/sessions. | Modify |
| `crates/aether-cli/tests/integration/main.rs` | Register the new command integration module. | Modify |
| `crates/aether-cli/tests/integration/mcp_command.rs` | Test the real CLI process against a fake UDS gateway. | Add |
| `crates/aether-project/src/{mcp_config_source_config,aether_settings,agent_catalog}.rs` | Rename source-level proxy settings/builders to `deferTools` and preserve merge semantics. | Modify |
| `crates/acp-utils/src/notifications.rs` | Rename serialized MCP status projection from proxied to deferred where applicable. | Modify |
| `crates/wisp/src/components/server_status.rs` | Render model-visible/deferred status sections and new field names. | Modify |
| `crates/wisp/tests/component_tests/settings_overlay.rs` | Update status/config expectations. | Modify |
| `mcp.json` | Replace `proxy` configuration with `deferTools`. | Modify |
| `README.md`, `crates/aether-core/README.md`, `crates/mcp-utils/README.md`, `crates/mcp-servers/README.md`, `crates/aether-cli/README.md` | Replace proxy examples with deferred progressive-discovery usage. | Modify |
| `crates/mcp-utils/src/docs/mcp_server_config.md`, `crates/aether-core/src/docs/tool_filter.md` | Document `deferTools`, route/filter behavior, and single-catalog semantics. | Modify |
| `packages/website/src/content/docs/aether/settings/mcp-servers.mdx` | Replace filesystem proxy documentation with deferred Bash discovery, JSON CLI, security, and examples. | Modify |
| `packages/website/src/content/docs/aether/settings/{overview,user-project-settings}.mdx` and `packages/website/src/pages/index.mdx` | Update settings snippets and feature wording. | Modify |
| `Cargo.lock` | Record dependency feature/package changes. | Modify |

## Additional Notes

- Implement this as a sequence of reviewable changes matching Steps 1–9. The data-plane/typed-handle refactor, session synchronization, refresh support, generic UDS transport, and Bash process cleanup are independently useful and should not be hidden inside one feature commit.
- Reuse behavioral ideas and focused tests from `origin/feat/deferred-tools-use-bash`, but do not cherry-pick its manager rewrite, connect-time discovery changes, mutable `BashEnvironment`, broad cleanup churn, or multi-syntax CLI parser.
- Do not re-fetch tool schemas during help. Do not let the upstream server's acceptance of an unknown tool bypass catalog authorization; stale/unknown calls fail closed until a valid catalog refresh arrives.
- Do not add a custom JSON socket protocol: rmcp already supplies framing, cancellation behavior, schemas, and a reusable endpoint for future external/session clients.
- Follow-up work, intentionally out of scope: optional key/value CLI sugar based on observed need, shell completion, a human standalone `aether mcp --socket` mode, Windows named-pipe transport, an independently motivated Brush feasibility spike, and richer trace-parent propagation from a Bash tool invocation into its nested CLI process.
