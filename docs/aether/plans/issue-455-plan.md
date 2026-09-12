# ACP v2 Migration Plan (Issue #455)

## Overview

### Problem statement

Migrate the entire repository from Agent Client Protocol v1 to **ACP v2 only**
(no v1 compatibility), per the [v2 migration guide](https://agentclientprotocol.com/protocol/v2/migration).
The Rust `agent-client-protocol` crate is already at 2.0.0 (schema 1.5.0), which
ships both `schema::v1` and the draft `schema::v2` module — but the workspace only
enables `unstable_elicitation`, so all Rust code currently imports `schema::v1`.
The TypeScript SDK (`@agentclientprotocol/sdk` 1.3.0) already bundles a `dist/v2`
schema. Everything that speaks ACP must move to v2 wire format and v2 semantics.

Motivation: v2's prompt-lifecycle redesign (prompt response = acceptance ack,
progress/completion via `state_update` notifications) is a prerequisite for remote
connections where the client (wisp) can detach while the agent (aether) keeps running.

### Success criteria / acceptance conditions

- `agent-client-protocol` is used with `unstable_protocol_v2`; **zero** remaining
  references to `agent_client_protocol::schema::v1` in Rust code (grep clean).
- Agent (`aether-cli`) negotiates `ProtocolVersion::V2`, serves v2 method names
  (`auth/login`, `auth/logout`, no `session/load`), and implements the v2 prompt
  lifecycle (immediate `{}` ack → `user_message` ack → `running` → `idle` + stop reason).
- Shared client (`acp-utils`) and TUI (`wisp`) drive turn lifecycle from
  `state_update`, handle all new/renamed `SessionUpdate` variants, and resume via
  `session/resume` + `replayFrom`.
- TS SDK (`packages/aether-sdk`) speaks v2 (init params, prompt ack, state-driven
  results, new permission shape); `fakeAether.mjs` and `e2e.mjs` updated.
- `just check`, `just lint`, `just test` (Rust) and `pnpm typecheck` + `pnpm test`
  (TS SDK) all pass; existing integration tests ported to v2 types and passing.

---

## Technical Approach

### High-level architectural decisions

1. **Flag-day migration, no dual-stack.** The issue explicitly says v1 compat is not
   wanted. Every `schema::v1` import becomes `schema::v2` in one coordinated change
   per crate, in dependency order: `acp-utils` → `aether-cli` → `wisp` → TS SDK.
   No version negotiation (agent always answers `V2`; clients always send `V2`).
2. **One feature flag addition.** Add `unstable_protocol_v2` to the workspace
   `agent-client-protocol` dependency. No other unstable features are needed:
   `AuthMethod::Agent`, `PlanUpdateContent::Items`, stdio/http MCP, and elicitation
   are all stable v2; only `unstable_elicitation` (already on) is still required.
   Note: enabling `unstable_protocol_v2` **removes** `ProtocolVersion::LATEST`
   (upstream forces an explicit `V1`/`V2` choice) and adds `ProtocolVersion::V2`.
3. **New prompt-lifecycle state machine.** This is the core redesign, touching
   `aether-cli/src/acp/session/actor.rs` (agent) and
   `acp-utils/src/client/session.rs` + `wisp/src/app/acp_reducer.rs` (client):
   - Agent: `route_prompt` validates, hands the responder to the actor, and the actor
     responds `{}` **immediately on acceptance** (before running the turn), then emits
     `user_message` (agent-owned `messageId`), `state_update(running)`, existing
     streaming updates, and finally `state_update(idle + stopReason)`.
   - Client: `PromptCompleted(StopReason)` can no longer come from the prompt
     response. The client must track "turn in flight" per session and synthesize
     completion from the idle `state_update`. Cancellation confirmation likewise
     moves from the prompt response to idle + `cancelled`.
4. **Upsert/patch semantics via `MaybeUndefined`.** v2 flattens `ToolCallUpdateFields`
   into `ToolCallUpdate` and uses `MaybeUndefined<T>` (`Undefined` = omitted/unchanged,
   `Null` = cleared, `Value` = replaced) for tool-call fields, message `content`, and
   terminal updates. Builders take `impl IntoMaybeUndefined<T>`; plain values convert
   via `.into()`/`IntoMaybeUndefined`. Message `content` uses
   `MaybeUndefined<Vec<...>>`. Chunks (`ContentChunk`, `ToolCallContentChunk`) always
   append.
5. **Deletes are pure deletions.** `session/load`, `McpServer::Sse`, `ToolCall` create
   variant, `SessionUpdate::Plan`, `AuthenticateRequest`, `CancelNotification`,
   `SessionNotification`, `AvailableCommandInput::Unstructured`, v1 capability builders —
   all removed with no shim. The client fs/terminal surface needs no work: the repo
   never implemented it (verified by grep — zero usages).

### Design patterns to employ

- Keep the existing layering: protocol mapping stays in
  `aether-cli/src/acp/protocol/*`, per-session state stays in the `SessionActor`,
  client connection logic stays in `acp-utils/src/client/*`, UI reduction stays in
  `wisp/src/app/acp_reducer.rs`. This is a type-and-semantics swap, not a refactor.
- Keep `Fake` test harnesses (`AcpTestHarness`, `TestPeer`, wisp `testing.rs`
  builders) but port them to construct v2 types; they remain the primary integration-test
  mechanism per repo testing guidelines.
- Preserve the `_aether/*` extension methods unchanged (custom `_`-prefixed methods
  are still legal in v2).

### Key technical considerations and trade-offs

- **v2 is still draft upstream** (`unstable_protocol_v2`, "may change at any time").
  Pin the exact resolved versions (`agent-client-protocol 2.0.0`,
  `agent-client-protocol-schema 1.5.0`) and expect follow-up bumps. Acceptable per issue.
- **No `sessionөд/load` replay path on the client after migration.** `acp-utils`
  `ClientCommand::LoadSession` + `ReplayState` capture keyed on `session/load` response
  must be reworked: `resume_session` gains an optional `replayFrom`, and replay capture
  should trigger on `resume_session` with `ReplayFrom::Start`. `LoadedSession` is
  replaced by a `ResumedSession { session_id, response: ResumeSessionResponse, replay }`.
- **Message-ID ownership.** The agent owns all `messageId`s, including the ack of the
  *user* message. The actor must mint a `messageId` per prompt (e.g. `uuid`) and reuse
  per-turn message IDs already present in `AgentEvent::Message` (`message_id` field).
  v2 `ContentChunk::new(content, message_id)` takes the ID as a required constructor arg
  (v1's was `Option` + setter).
- **In-flight prompt rejection stays.** `handle_in_flight_command` still rejects a
  second prompt with "prompt already in progress"; the client `Busy` guard stays, but
  "prompting" now spans ack → idle update rather than request → response.
- **Diffs.** v1 `Diff { path, old_text, new_text }` → v2 `Diff { changes: Vec<DiffChange>,
  patch: Option<DiffPatch> }`. Agent must classify add/delete/modify from presence of
  old/new text and supply a `git_patch` text when feasible (synthesize a minimal
  `diff --git` section from path + old/new text if no real git diff is available).
  Wisp `ToolCall::apply_update` / `FileDiff::from_texts` must be reworked to render
  `patch.text` and drive file trees from `changes` (handle patch-less diffs).
- **Terminal updates are new surface, display-only.** The agent does not currently
  produce terminal output over ACP, and wisp ignores `ToolCallContent::Terminal`. The
  migration does **not** need to add terminal streaming; just handle
  `TerminalUpdate`/`TerminalOutputChunk` gracefully on the client (ignore or minimal
  transcript state) and don't emit them from the agent. (Spawning actual terminal
  streaming is follow-up work, noted below.)
- **Permission requests.** The agent never sends them (client auto-approves). Still,
  the `RequestPermissionRequest` shape changes (`title` required, `toolCall` →
  `subject: ToolCallUpdate`) so the `acp-utils` handler signature and
  `auto_approve_option` must be ported; same for the TS SDK `autoApprovePermissions`
  and `fakeAether.mjs` permission stub.
- **Config option renames.** `SessionConfigOption.id` → `config_id`,
  `SessionConfigOptionValue::ValueId` → `Id` (note: v2 **requires** `type: "id"` on the
  wire; v1 defaulted when absent), `SessionConfigSelectGroup.group` → `group_id`
  (check usages — `session_config_view.rs` handles grouped options).
- **TS SDK connection classes.** Verify whether `ClientSideConnection` in
  `@agentclientprotocol/sdk@1.3.0` speaks v1 only or has a v2 counterpart
  (`dist/v2/acp.*` exists — inspect its exports first; use the v2 connection/client
  types if they exist, else bump the SDK dependency to a version with v2 support).

---

## Implementation Steps

### Step 0 — Workspace dependency + recon

1. In `/Cargo.toml:49`, change the ACP dependency to
   `agent-client-protocol = { version = "2.0.0", features = ["unstable_elicitation", "unstable_protocol_v2"] }`.
2. Run `cargo check -p aether-acp-utils` to confirm `schema::v2` resolves; confirm
   `ProtocolVersion::V2` exists and `ProtocolVersion::LATEST` is gone (fix any use).
3. Inspect TS SDK v2 entry point:
   `node_modules/.pnpm/@agentclientprotocol+sdk@1.3.0_zod@4.4.3/node_modules/@agentclientprotocol/sdk/dist/v2/acp.*`
   — determine the v2 `ClientSideConnection` equivalent and v2 `PROTOCOL_VERSION`.
   If the v2 client is missing/incomplete, upgrade `@agentclientprotocol/sdk` first.

### Step 1 — `crates/acp-utils` (shared client + testing)

4. `src/client/event.rs`: switch imports to `schema::v2`; `SessionNotification` →
   `UpdateSessionNotification`; `SessionUpdate` stays by name but gains variants.
   Replace `AcpEvent::PromptCompleted(StopReason)` with a state-driven event, e.g.
   `AcpEvent::PromptCompleted { session_id: SessionId, stop_reason: StopReason }`
   (session id is needed now that completion arrives as a notification, not a
   request response). Keep `ReplayableEvent::SessionUpdate(Box<UpdateSessionNotification>)`.
5. `src/client/session.rs`:
   - Imports → v2: `AuthenticateRequest` → `LoginAuthRequest`,
     `CancelNotification` → `CancelSessionNotification`, drop `LoadSession*`.
   - `LoadedSession` → `ResumedSession { session_id, response: ResumeSessionResponse, replay }`.
   - Replace `load_session()` with replay-aware resume: `resume_session()` stays
     (no replay); add `load_session()`-equivalent as
     `resume_session_with_replay()` or a `replay: bool` param that sets
     `replay_from = Some(ReplayFrom::Start(ReplayFromStart::new()))` and captures
     notifications into `ReplayState` exactly as today (rename `ClientCommand::LoadSession`
     → `ClientCommand::ResumeWithReplay`).
   - `prompt()` return type becomes v2 `PromptResponse` (empty); `run_prompt` no longer
     emits completion from the response. Instead track per-connection turn state:
     after a successful prompt ack, watch incoming `UpdateSessionNotification`s for
     `SessionUpdate::StateUpdate(StateUpdate::Idle(idle))` for that session and emit
     `AcpEvent::PromptCompleted` with `idle.stop_reason` (default to `EndTurn` if
     `None`? — decide: use `unwrap_or(StopReason::EndTurn)` and document).
     Keep the `Busy` guard: a second `Prompt` while a turn is in flight → `Busy`.
     `cancel()` sends `CancelSessionNotification`; after cancel, keep accepting updates
     until the idle+`cancelled` update (do not synthesize completion locally).
   - Permission handler: new `RequestPermissionRequest::{ session_id, title, options }`
     shape; keep auto-approve-first-allow logic (`auto_approve_option` unchanged logic,
     new type). Response construction unchanged (`Selected`/`Cancelled`).
   - `agent_name()`: `initialize_response.info` (was `agent_info`); capabilities
     accessors: `initialize_response.capabilities.session: Option<SessionCapabilities>`
     (was `agent_capabilities.prompt_capabilities` / `.session_capabilities`).
     Presence-check prompt caps: `capabilities.session.as_ref().and_then(|s| s.prompt.as_ref())`.
6. `src/testing.rs` (`TestPeer`): port `next_session_notification()` etc. to
   `UpdateSessionNotification`; add helpers to emit `StateUpdate` idle/running and
   `PlanUpdate` for tests.
7. `src/elicitation.rs`, `src/notifications.rs`, `src/content.rs`, `src/websocket.rs`:
   recompile against v2 (`ContentBlock`/`ContentChunk` paths change; `message_id`
   required in test constructors). No logic change expected except chunk constructors.
8. Port `tests/client_session.rs`, `tests/client_disconnect.rs`, `tests/tokio_agent.rs`,
   `tests/websocket.rs` to v2 types (`InitializeRequest::new(V2, info)`,
   `.capabilities(..)`/`.info(..)` builders, `UpdateSessionNotification`,
   prompt-ack + idle-update flow, resume-with-replay instead of load).

### Step 2 — Agent: `crates/aether-cli/src/acp`

9. `agent.rs`: swap imports to v2; replace `AuthenticateRequest` handler with
   `LoginAuthRequest`, **add** `LogoutAuthRequest` handler (new `state.logout()`;
   for OAuth-codex flow, logout = clear stored credential for the provider and
   broadcast updated auth methods + config options); **delete** `LoadSessionRequest`
   handler; `CancelNotification` → `CancelSessionNotification`.
10. `state.rs::initialize`: read `args.capabilities` / `args.info` (were
    `client_capabilities`/`client_info`); respond
    `InitializeResponse::new(ProtocolVersion::V2, Implementation::new("Aether", "0.1.0"))`
    `.capabilities(AgentCapabilities::new().session(SessionCapabilities::new()
    .prompt(prompt_caps).mcp(McpCapabilities::new().stdio(...).http(...))))`.
    Notes: capability markers are now objects (`PromptImageCapabilities::new()` instead
    of `bool`); drop `.list()/.resume()/.close()` markers (baseline is implied by
    advertising `session`); drop `.load_session(true)`; McpCapabilities `.sse(true)` →
    `.stdio(McpStdioCapabilities::new()).http(McpHttpCapabilities::new())`; keep the
    Aether `_meta` capabilities (rename getters: `response.capabilities.session`
    is `Option` — adjust the two `initialize_*` tests). `auth_methods` stays, but
    `AuthMethodAgent::new(id, display)` now sets `method_id` (check: v2 constructor
    takes `impl Into<AuthMethodId>` — same call shape).
11. `state.rs::resume_session`: honor `req.replay_from`: if
    `Some(ReplayFrom::Start(_))`, replay stored events via `replay_to_client` before
    responding (merge old `load_session` body in); otherwise plain resume. Delete
    `state.rs::load_session`. Keep `register_session` behavior.
12. `session/factory.rs`: `create`/`restore` take v2 `NewSessionRequest` /
    `ResumeSessionRequest` (fields `cwd: AbsolutePath`, `mcp_servers` unchanged by name);
    collapse `load()` into `resume()` with a `replay: bool` derived from `replay_from`.
    `SessionMeta.cwd` assignment from `AbsolutePath` (convert via `PathBuf::from` /
    `.as_ref()` — check `AbsolutePath` API).
13. Prompt lifecycle (`state.rs::route_prompt`, `session/actor.rs`):
    - `route_prompt` keeps validation, then sends `SessionCommand::Prompt { content,
      responder }` as today, but `handle_prompt` must **respond `{}` immediately after
      acceptance** (after slash-command expansion + persisting the user event, before
      looping on runtime events) instead of holding the responder to turn end.
    - After ack, emit in order: `UserMessage { message_id: <new uuid>, content:
      Value(prompt blocks mapped to v2 ContentBlock) }`, then
      `StateUpdate::Running`, then existing streaming updates, then
      `StateUpdate::Idle { stop_reason: Some(reason) }`zor on turn end / cancel /
      error paths (map `TurnOutcome::Cancelled` → `Cancelled`, else `EndTurn`).
    - `respond_prompt` is deleted; error paths after ack must still terminate the turn
      with an idle update (plus an error `agent_message` chunk, mirroring current
      `Turn::Ended Failed` behavior) since there is no longer a response channel to
      report failure on. Pre-acceptance failures (unknown session, bad media, busy)
      still use `respond_with_error`.
    - All `ContentChunk::new(content)` calls become
      `ContentChunk::new(content, message_id)` with the turn's agent/thought message IDs;
      `SessionNotification::new` → `UpdateSessionNotification::new`.
14. `protocol/events.rs` (AgentEvent → SessionUpdate mapping):
    - `ToolCall` create → first-seen `ToolCallUpdate::new(id).title(..).kind(..)
      .status(..)` (flattened fields, `MaybeUndefined` builders; `content` as
      `Value(vec)` to replace, chunks append).
    - `ToolCallUpdateFields` construction → flattened `ToolCallUpdate` builder calls.
    - `SessionUpdate::Plan(Plan::new(entries))` →
      `SessionUpdate::PlanUpdate(PlanUpdate::new(PlanUpdateContent::Items(
      PlanItems::new(plan_id, entries))))` — mint a stable per-session `plan_id`
      (e.g. `"plan"` or per-turn uuid; check v2 `PlanItems::new` signature) and map
      entry status incl. new `cancelled`.
    - `ToolCallContent::Diff { path, old_text, new_text }` → v2
      `Diff::new(changes).with_patch(DiffPatch::new(git_patch_text))` where `changes`
      = one of add/delete/modify derived from old/new presence (paths must be absolute;
      `file_type: Text` + `mime_type` where known). Synthesize minimal
      `diff --git a/<p> b/<p>` text when only old/new text is available.
    - `map_chunk_to_notification` Live/Replay skip logic stays, but chunks now carry
      required `message_id`.
    - `plan_status_to_acp` gains `cancelled` mapping.
15. `protocol/replay.rs`: user history → `UserMessage` upsert(s) (whole-message, with
    agent-owned message IDs regenerated or persisted — decide: regenerate fresh IDs per
    replay and keep chunks out of replay to avoid duplication), agent events via
    `NotificationMode::Replay` as today; terminal state (not emitted) needs nothing.
16. `protocol/commands.rs`: `AvailableCommandInput::Unstructured(UnstructuredCommandInput::new(hint))`
    → `AvailableCommandInput::Text(TextCommandInput::new(hint))` (check v2 ctor name).
17. `protocol/mcp.rs`: `McpServer::{Stdio, Http}` only; **delete the `Sse` arm**
    (log + skip as unsupported); `McpServerStdio.command` is now `AbsolutePath`
    (`.to_string_lossy()` still works via deref — verify); `args`/`env`/`headers`
    optional (same access).
18. `protocol/content.rs`: map to v2 `ContentBlock` (same five variants; check
    `resource_link.icons` — nothing to do unless constructing them).
19. `session/model.rs`, `session/config.rs`, `session/slash_commands.rs`,
    `session/actor.rs` remainder: `SessionConfigOption::select(id, ...)` stays but
    field is now `config_id`; `SessionConfigOptionValue::ValueId` → `Id` (wire now
    requires `type: "id"`); `set_session_config_option` value matching updated;
    `SessionNotification` → `UpdateSessionNotification` in `broadcast_config_options`
    and `send_available_commands`.
20. `testing.rs` harness + `tests/integration/*` (`acp_session_lifecycle`,
    `acp_cancellation`, `acp_agent_switching`, `acp_stdio`, `slash_commands`):
    port to v2 (init V2, no load, resume+replayFrom, prompt-ack then idle assertions).

### Step 3 — Client TUI: `crates/wisp`

21. `session/mod.rs::connect`: `InitializeRequest::new(ProtocolVersion::V2, info)`
    `.capabilities(client_caps)`; read agent caps from
    `response.capabilities.session` (presence checks, not bools); prompt caps from
    `session.prompt`; `new_session` response `config_options` is now `Vec` (not
    `Option` — drop `unwrap_or_default`).
22. `runtime/agent.rs`: `CancelNotification::new` → `CancelSessionNotification::new`;
    `AuthenticateRequest::new` → `LoginAuthRequest::new`; `LoadSessionRequest` →
    resume-with-replay call; add `LogoutAuthRequest` wiring if the settings overlay
    offers logout (else skip — agent must still *implement* logout per spec).
23. `app/acp_reducer.rs` (`on_session_update`) — the biggest client change:
    - `SessionUpdate::ToolCall` arm deleted (creation now arrives as first
      `ToolCallUpdate` for an unseen id).
    - New arms: `UserMessage`/`AgentMessage`/`AgentThought` whole-message upserts
      (replace-by-`messageId`, `Null`/empty clears); `StateUpdate(Running)` →
      `progress.response_started()`/activity; `StateUpdate(Idle)` → `finish_prompt`
      with `stop_reason` (this replaces `PromptCompleted`-from-response; keep
      `AcpEvent::PromptCompleted` as the internal event but produce it from idle);
      `StateUpdate(RequiresAction)` → show waiting/interrupt hint.
    - `PlanUpdate` (match `PlanUpdateContent::Items(items)`; replace entries for
      `items.plan_id`; ignore/preserve `Other`) replaces `Plan` arm; `PlanRemoved`
      (if compiled — it's behind `unstable_plan_operations`, off by default) → clear.
    - `ToolCallContentChunk` → append single content item to the tool call.
    - `TerminalUpdate`/`TerminalOutputChunk` → ignore for now (log at debug) or stash
      minimal transcript; do not crash on them.
    - `AvailableCommandsUpdate` input union: handle `Text` variant (fall back to no
      hint for `Other`).
24. `conversation/items.rs` + `tool_calls.rs`: key messages by v2 `messageId`
    (apply upsert replace/clear vs chunk append); `from_acp(&ToolCall)` →
    `from_update(&ToolCallUpdate)` (flattened fields via `.value()` accessors on
    `MaybeUndefined`); `apply_update` handles `Null` (clear title/content) vs
    `Undefined` (keep); diff rendering from v2 `Diff { changes, patch }`
    (prefer `patch.text`, file list from `changes`; patch-less → generic file list).
25. `conversation/plan_tracker.rs` (+ `plan_view.rs`, `renderer/layout.rs`): key by
    `plan_id`; each `PlanUpdate` replaces that plan's entries (keep grace-period sort).
26. `session/session_config_view.rs` + `session_model.rs`: `config_id` (was `id`),
    `value` ids (was `value_id`? verify), `group_id` for grouped options; config value
    payloads now always carry `type: "id"`/`"boolean"`.
27. `surfaces/*`, `settings/overlay/*`: elicitation types move to `schema::v2`
    (same names); permission UI still unneeded (auto-approve stays in acp-utils).
28. Port wisp tests: `testing.rs` builders (`ToolCall::new` →
    `ToolCallUpdate::new`, `Plan::new` → `PlanUpdate`, `complete_prompt(StopReason)`
    → ack + idle-update sequence, `load_session` helpers → resume-with-replay),
    all `tests/tui/*` + `tests/runtime.rs` + `tests/diff_model.rs`.

### Step 4 — TypeScript SDK: `packages/aether-sdk`

29. `src/session.ts`: `initialize({ protocolVersion: 2, info: {...}, capabilities: {...} })`
    (drop `clientInfo`/`clientCapabilities` keys, drop `fs`/`terminal` caps);
    `newSession({ cwd })` (drop `mcpServers: []` — now optional); `prompt()` returns
    `{}` → drive the `result` event from the idle `state_update` notification instead
    of `response.stopReason`; `sessionUpdate(notification: acp.UpdateSessionNotification)`
    handler; permission handler updated to `{ title, options }` request shape.
    Use the `dist/v2` client/connection types (per Step 0 recon).
30. `test/fakeAether.mjs`: `protocolVersion: 2`, `capabilities`/`info` keys, no
    `loadSession` (implement `resumeSession` + `replayFrom: { type: "start" }` or plain
    resume), `auth/login` + `auth/logout`, prompt ack `{}` + `state_update` idle,
    permission stub with `title`.
31. `scripts/e2e.mjs`: `sessionUpdate === "tool_call"` → `"tool_call_update"`;
    expect idle `state_update` for completion; message chunks now carry required
    `messageId`.
32. Run `pnpm --filter @aether-agent/sdk typecheck`, `test`, and `e2e` (if runnable).

### Step 5 — Cutover checks

33. Grep-clean verification: `schema::v1` (Rust) and `protocolVersion: 1` /
    `loadSession` / `authenticate(` (TS) return zero hits outside CHANGELOGs.
34. `just check`, `just lint`, `just test`; fix all warnings (workspace denies
    pedantic clippy warns).
35. Manual smoke: `wisp` (default `aether acp`) — new session, prompt streams,
    cancel mid-turn shows `cancelled`, `/resume` reloads history, settings overlay
    model switch, MCP stdio server connects.

---

## Testing Plan

### Unit tests required (new or ported)

- Agent `initialize` advertises v2 baseline (existing
  `initialize_advertises_session_lifecycle_support` rewritten: assert
  `protocol_version == V2`, `capabilities.session.is_some()`, prompt/mcp nested,
  no `loadSession`, no `sse`).
- Prompt lifecycle: ack `{}` returns immediately on acceptance; `user_message` +
  `running` emitted before first chunk; idle + `stopReason` emitted at turn end;
  cancel → idle + `cancelled`; second prompt while in flight → error (not silent).
- Tool-call upsert: first `ToolCallUpdate` creates, later ones patch (omitted keeps,
  explicit null clears); no `ToolCall` create variant emitted.
- Plan: emits `PlanUpdate` with `Items` + stable `plan_id`; entries replace per update.
- Diff: add/delete/modify classification + `git_patch` text present and consistent
  with `changes`.
- Resume: `replayFrom: start` replays full history as updates (incl. `user_message`
  upserts); omitted replays nothing.
- Config: `config_id` wire name, `type: "id"` value payloads, unknown-category tolerance.
- Client: idle `state_update` synthesizes `PromptCompleted` with correct stop reason;
  background updates while idle don't reset turn state; `Busy` during turn.

### Integration tests needed

- Port all of `aether-cli/tests/integration/acp_*.rs`, `acp-utils/tests/*.rs`,
  `wisp/tests/**` to v2 types/flows (same scenarios, new assertions on ack + idle).
- End-to-end wisp TUI canonical conversation test exercising the full
  ack → user_message → running → chunks → idle(EndTurn) sequence.
- TS SDK `fakeAether` round-trip (prompt → chunks → idle result) + permission stub.

### Edge cases to verify

- Cancel arriving before ack vs after ack; cancel while idle (ignored, no idle emitted).
- Prompt error *after* ack (must still idle the turn; client must not hang).
- Unknown `SessionUpdate`/`StateUpdate`/`DiffChange`/`AuthMethod` variants preserved,
  not fatal (v2 open enums) — client renders generically.
- `stop_reason: None` on idle (plain ready signal, e.g. after resume) ≠ turn end.
- Batch JSON-RPC arrays on stdio accepted (v2 explicitly allows batching).
- `additionalDirectories` / `mcpServers` omitted vs `[]` equivalence on new/resume.

---

## Files to Modify/Create

| File | Change | Kind |
|---|---|---|
| `Cargo.toml` (workspace) | Add `unstable_protocol_v2` to `agent-client-protocol` features | Modified |
| `crates/acp-utils/src/client/event.rs` | v2 imports; `UpdateSessionNotification`; state-driven `PromptCompleted` | Modified |
| `crates/acp-utils/src/client/session.rs` | v2 requests/notifications; resume-with-replay; turn tracking via idle update; new permission shape; new capability accessors | Modified |
| `crates/acp-utils/src/client/error.rs` | Check for v1 type refs; port if any | Modified |
| `crates/acp-utils/src/client/tokio_agent.rs` | Recompile (likely no logic change) | Modified |
| `crates/acp-utils/src/testing.rs` | v2 notification helpers (state/plan builders) | Modified |
| `crates/acp-utils/src/content.rs`, `src/elicitation.rs`, `src/notifications.rs`, `src/websocket.rs`, `src/lib.rs` | v2 imports; chunk ctor updates | Modified |
| `crates/acp-utils/tests/*.rs` (4 files) | Port to v2 flows | Modified |
| `crates/aether-cli/src/acp/agent.rs` | v2 handlers; login/logout; delete load; `CancelSessionNotification` | Modified |
| `crates/aether-cli/src/acp/state.rs` | v2 initialize/caps/auth; resume+replay; delete `load_session`; `UpdateSessionNotification` broadcast; tests | Modified |
| `crates/aether-cli/src/acp/session/actor.rs` | Immediate ack; user_message/running/idle emission; chunk IDs; delete `respond_prompt` | Modified |
| `crates/aether-cli/src/acp/session/factory.rs` | v2 request types; merge load into resume+replay | Modified |
| `crates/aether-cli/src/acp/session/model.rs`, `config.rs`, `config_setting.rs` | `config_id`, `Id` values, category check (`ThoughtLevel` unchanged) | Modified |
| `crates/aether-cli/src/acp/protocol/events.rs` | ToolCallUpdate flattening; PlanUpdate; v2 Diff; required message IDs | Modified |
| `crates/aether-cli/src/acp/protocol/replay.rs` | `UserMessage` upserts + replay mode | Modified |
| `crates/aether-cli/src/acp/protocol/commands.rs` | `AvailableCommandInput::Text` | Modified |
| `crates/aether-cli/src/acp/protocol/mcp.rs` | Drop SSE arm; `AbsolutePath` command | Modified |
| `crates/aether-cli/src/acp/protocol/content.rs` | v2 `ContentBlock` mapping | Modified |
| `crates/aether-cli/src/acp/session/slash_commands.rs` | `UpdateSessionNotification` | Modified |
| `crates/aether-cli/src/acp/testing.rs`, `fake_prompt_mcp.rs` | v2 harness types | Modified |
| `crates/aether-cli/tests/integration/*.rs` | Port to v2 | Modified |
| `crates/wisp/src/session/mod.rs` | v2 init (V2+info+caps), new caps accessors | Modified |
| `crates/wisp/src/runtime/agent.rs` | Login/logout/cancel/resume-with-replay calls | Modified |
| `crates/wisp/src/app/acp_reducer.rs` | New update arms; idle-driven completion; plan keyed by id | Modified |
| `crates/wisp/src/conversation/items.rs`, `tool_calls.rs`, `tool_view.rs` | Message upserts; flattened tool updates; v2 diff render | Modified |
| `crates/wisp/src/conversation/plan_tracker.rs`, `plan_view.rs` | Plan keyed by `plan_id` | Modified |
| `crates/wisp/src/session/session_model.rs`, `session_config_view.rs` | `config_id`/`group_id`, `type`-tagged values | Modified |
| `crates/wisp/src/surfaces/*`, `settings/overlay/*` | v2 elicitation imports | Modified |
| `crates/wisp/src/testing.rs`, `tests/**/*.rs` | v2 builders; ack+idle flows; resume helpers | Modified |
| `packages/aether-sdk/src/session.ts` | v2 init/prompt/result/permission/subscription | Modified |
| `packages/aether-sdk/src/types.ts`, `tool.ts`, `headless.ts` (if v1-typed) | v2 type updates | Modified |
| `packages/aether-sdk/test/fakeAether.mjs` | v2 fake agent | Modified |
| `packages/aether-sdk/scripts/e2e.mjs` | v2 update names + idle completion | Modified |
| `packages/aether-sdk/package.json` | Bump `@agentclientprotocol/sdk` if v2 needs it | Modified |
| `docs/aether/plans/issue-455-plan.md` | This plan | Added |

No files removed (only code paths/types within files).

---

## Additional Notes

### Documentation updates needed

- `README.md` / TUI docs: update any stated ACP version or protocol examples to v2.
- Crate `CHANGELOG.md`s are release-generated; no manual edits.
- Note in the plan handoff that v2 remains upstream-unstable: after migration, track
  `agent-client-protocol` releases for breaking v2 changes.

### Follow-up tasks that may be spawned

- **Remote transport (the actual motivation):** v2 core does not define the remote
  transport (separate streamable-HTTP/WebSocket RFD). Wire `acp-utils/websocket.rs`
  into wisp behind a CLI flag once the agent side supports it; enables
  detach/reattach against idle `state_update` semantics.
- **Agent-owned terminal streaming:** emit `TerminalUpdate`/`TerminalOutputChunk`
  from the agent for command execution display; render in wisp (isolated emulator or
  sanitized transcript).
- **Permission UI in wisp:** surface `session/request_permission` (now with
  `title`/`description`/`subject`) instead of silent auto-approve.
- **Background/queued prompts:** v2 lifecycle permits agent-initiated and queued work;
  evaluate multi-client session observation now that completion is notification-based.
- **Elicitation `unstable_*` alignment:** confirm elicitation stays on
  `unstable_elicitation` under v2 (stable v2 defines no client caps; elicitation remains
  unstable in both versions — no change needed, but re-verify at bump time).

### Open questions for the implementer (all decidable locally)

1. Idle update with `stop_reason: None` (ready signal, not turn end): synthesize
   `PromptCompleted(EndTurn)` only if a turn is in flight, else ignore — implement and
   unit-test this rule.
2. Replay `messageId` stability: regenerating IDs per replay is simpler and matches
   "agent owns identity"; document the choice in `replay.rs`.
3. Per-session vs global `plan_id`: use a fixed `"plan"` id per session unless multiple
   concurrent plans are observed; simplest correct for now.
4. `SetSessionConfigOptionResponse` shape in v2: verify ctor (takes `options` vec as v1
   — confirm against vendored source before editing `actor.rs`).
5. `TextCommandInput::new(hint)` ctor name and `PlanItems::new(plan_id, entries)`
   signature: verify against vendored v2 sources (`~/.cargo/registry/src/*/agent-client-protocol-schema-1.5.0/src/v2/`)
   at implementation time; signatures noted in this plan are from inspection and should
   be re-confirmed.
