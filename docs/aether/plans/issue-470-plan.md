# Issue #470 Plan — Move git diff backend out of wisp, into ACP agent

## Overview

### Problem statement

Aether now supports `aether server` (ACP agent over WebSocket, running e.g. in a
remote sandbox / lambda microVM) plus `aether client` (wisp TUI on a laptop
connecting to that server). Git diff review and file-change watching are still
implemented **in wisp on the ACP client side** (`crates/wisp/src/runtime/dispatcher.rs`
owns a `clankerdiff_git::GitRepository` + `clankerdiff_watch::RepositoryWatcher`
against the *local* working directory). On a remote connection the diff screen
either shows the wrong repository or is disabled entirely (see
`crates/wisp/src/app/input.rs`: `"Git review is unavailable for remote workspaces"`).
The same is true of the status-line git ref (`crates/wisp/src/runtime/git.rs`
shells out to a local `git` binary).

The fix: run `clankerdiff-git` / `clankerdiff-watch` on the **ACP agent side**
(`aether-cli`, next to the session's working directory) and stream review
snapshots to wisp over ACP extension messages. Wisp keeps only rendering
(`clankerdiff-ratatui` + `GitDiffScreen`) and forwards user intents
(stage / unstage / commit / discard / scope change / refresh) back to the agent.

### Success / acceptance criteria

1. `aether client` attached to a remote `aether server` can open the git-diff
   screen (`toggle_git_diff` keybinding) and sees the **server's** working-tree
   diff, live-updating as files change on the server.
2. Stage / unstage / stage-all / commit / discard actions from the remote TUI
   mutate the **server's** repository.
3. The status-line git ref on a remote connection reflects the server's branch/HEAD.
4. Local flow (`aether` default TUI → in-process `aether acp` agent) keeps working
   with no user-visible regression; existing diff keybindings, scope switching,
   review submission (`ReviewOutcome::Submitted` → prompt), and theme handling
   are unchanged.
5. `clankerdiff-git` / `clankerdiff-watch` are removed from wisp's dependency
   tree (moved to `aether-cli`); wisp retains `clankerdiff-core` +
   `clankerdiff-ratatui` for types and rendering only.
6. New ACP extension methods are covered by round-trip + integration tests
   (fake transport and real temp git repo against `AcpTestHarness`).

## Technical Approach

### Architecture

```
 TODAY                                  AFTER
 ─────                                  ─────
 wisp (client)                          wisp (client)
 ├─ GitDiffScreen (render)               ├─ GitDiffScreen (render, unchanged UI)
 ├─ CommandDispatcher                   ├─ CommandDispatcher
 │   ├─ GitRepository ─┐ local git      │   └─ ACP requests/notifications ─┐
 │   └─ RepositoryWatcher ─┘ + notify   │                                 │
 └─ git branch --show-current           │                                 ▼
                                        │              aether-cli (agent, per session)
                                        │               ├─ SessionActor
                                        │               │   └─ GitDiffService (NEW)
                                        │               │        ├─ GitRepository
                                        │               │        └─ RepositoryWatcher ─┐
                                        │               └─ _aether/git_diff_update ────┘
```

- **Source of truth moves; rendering stays.** `GitDiffScreen`
  (`crates/wisp/src/screens/git_diff/mod.rs`) already consumes
  `GitWatchEvent { review_id, result: GitWatchResult }` where
  `GitWatchResult = Result<RepositoryState, Arc<WatchError>>` and
  `RepositoryState { snapshot: Arc<RepositorySnapshot>, error: Option<...> }`.
  The wire payload mirrors this shape, so the screen changes minimally
  (error type mapping only).
- **Transport follows the existing `_aether/*` extension pattern.**
  All new wire types live in `crates/acp-utils/src/notifications.rs` (or a new
  `git_diff.rs` module re-exported from there), shared by both sides — exactly
  like `PromptSearchParams`, `WorkspaceListParams`, `McpNotification`, etc.
  Wisp receives updates via `acp-utils` `ClientHandlers` → new
  `AcpEvent::GitDiffUpdate` variant; wisp sends intents via
  `AcpClientHandle::request(...)` like every other `AgentCommand`.
- **Agent-side ownership: `SessionActor`, not `AcpState`.** The actor already
  serializes per-session mutation through its command channel, owns `cwd`,
  owns the `SessionIo` connection handle (which nulls out on detach), and is
  where prompt/config/MCP routing lives. A new `GitDiffService` struct owned
  by the actor gives us single-writer apply serialization for free (replacing
  wisp's `TaskSupervisor::spawn_git_mutation` per-repo queue) and natural
  cleanup on session close/shutdown. One active watch per session
  (last-open-wins); this matches the single-attached-client invariant enforced
  by `ClientSlot`.
- **Serialization: reuse clankerdiff types directly where possible.**
  Verified against clankerdiff 0.2.1 sources in the cargo registry:
  - `DiffDocument`, `FileDiff`, `FileStatus`, `StageState`, `DiffScope`,
    `RepositoryAction`, `RepoPath` (all in `clankerdiff-core`, re-exported by
    `clankerdiff-ratatui::diff`) already derive `Serialize`/`Deserialize`.
  - `RepositorySnapshot { scope, document: Arc<DiffDocument> }` does **not**
    derive serde; neither do `GitError` / `WatchError` (thiserror only, and
    `GitError::Spawn`/`Io` contain `io::Error` which cannot be serialized).
  - So the wire snapshot is a purpose-built struct (see below), and errors are
    sent as `{ kind, message }` pairs. Per the issue, we *may* propose upstream
    clankerdiff changes (serde on `RepositorySnapshot`/`RepositoryState`, or a
    `kind()` discriminant on the error enums) to shrink the mapping code —
    but the plan does not depend on it.

### Wire protocol (all under `_aether/`, camelCase, mirroring existing style)

```rust
// acp-utils, new module (see Files table)
pub struct GitDiffSnapshot {          // mirrors RepositoryState
    pub scope: DiffScope,             // serde already ✓
    pub document: DiffDocument,       // serde already ✓
    pub error_message: Option<String>,// from RepositoryState::error_message()
}
pub struct GitDiffErrorBody { pub kind: String, pub message: String }
// kind = "notRepository" | "gitFailed" | "watchStopped" | "unknownSession" | ...

// client → agent (JsonRpcRequest)
#[request(method = "_aether/git_diff_open",  response = GitDiffOpenResponse)]
pub struct GitDiffOpenParams { pub session_id: String, pub scope: DiffScope }
pub struct GitDiffOpenResponse { pub snapshot: GitDiffSnapshot }

#[request(method = "_aether/git_diff_set_scope", response = GitDiffOpenResponse)]
pub struct GitDiffSetScopeParams { pub session_id: String, pub scope: DiffScope }

#[request(method = "_aether/git_diff_apply", response = GitDiffApplyResponse)]
pub struct GitDiffApplyParams { pub session_id: String, pub action: RepositoryAction }
pub struct GitDiffApplyResponse { pub snapshot: GitDiffSnapshot } // post-apply reload
// apply failures → ACP Error::invalid_params / internal_error with GitDiffErrorBody in `data`

// client → agent (JsonRpcNotification, fire-and-forget)
#[notification(method = "_aether/git_diff_close")]
pub struct GitDiffCloseParams { pub session_id: String }

// agent → client (JsonRpcNotification, pushed by watcher task)
#[notification(method = "_aether/git_diff_update")]
pub struct GitDiffUpdateParams { pub session_id: String, pub snapshot: GitDiffSnapshot }

// workspace git ref (replaces wisp's local `git branch --show-current`)
#[request(method = "_aether/workspace_status", response = WorkspaceStatusResponse)]
pub struct WorkspaceStatusParams { pub session_id: String }
pub struct WorkspaceStatusResponse { pub git_ref: Option<String> }
```

Design notes / trade-offs:

- **Why requests for open/set_scope/apply and notifications for close/update?**
  Matches existing precedent (`workspace_list`/`workspace_move` are requests;
  `mcp_event`/`session_usage` are server→client notifications; `mcp_request` is
  a client→server notification). Open needs to return the initial snapshot
  synchronously so the screen paints immediately; updates then stream.
- **Why `session_id`-keyed instead of client-generated `review_id`?**
  The agent has no notion of wisp's `RequestId`; sessions are its identity unit
  and only one client is attached at a time. One watch per session keeps agent
  state trivial (no review-id registry, no leak on client crash — a new open
  replaces the old watch). Wisp keeps its `review_id` locally to ignore stale
  arrivals, unchanged.
- **Apply returns the post-apply snapshot** rather than relying solely on the
  watcher push, so the UI settles deterministically even under debounce.
  Watcher pushes are the live-update path for *external* (non-UI) changes.
- **Detach behavior:** `SessionIo::send` already drops notifications when
  `connection` is `None`. On re-attach (`SessionCommand::Attach` with replay),
  the actor re-sends the latest retained snapshot so a reconnected TUI with an
  open diff screen recovers without reopening.
- **Out of scope (explicitly):** `@` file picker index, dropped-file/paste
  attachments, and prompt-search `cwd` display all read the *local* filesystem
  and stay gated on `WorkspaceAccess::Local` (see `input.rs`, `submission.rs`).
  Only git diff + watch + workspace git-ref move. Call this out in the PR
  description so reviewers don't expand scope.

### Capability advertisement

Extend `AetherCapabilities` with `pub git_diff: bool` (agent sets `true`).
Wisp replaces the `WorkspaceAccess::Remote` hard-block in `input.rs`
(`"Git review is unavailable for remote workspaces"`) with a capability check:
if `!capabilities.git_diff`, notify `"Git review is unavailable for this agent"`.
Local agents always advertise `true`, so local behavior is unchanged and old
agents degrade gracefully with a notice instead of a broken screen.

### Upstream clankerdiff proposal (optional, follow-up)

File against `jcarver989/diff`: (1) `#[derive(Serialize, Deserialize)]` on
`RepositorySnapshot` (needs `Arc<DiffDocument>` handling — serde supports
`Arc<T>` already); (2) a non-serializing `kind()` / `code()` discriminant on
`GitError` and `WatchError` so downstream wire mappings don't string-match on
`Display`. If accepted, our `GitDiffErrorBody::kind` mapping collapses to a
match on that discriminant. Do this after the core move lands, not before.

## Implementation Steps

Each step is independently reviewable; suggested commit order follows the numbers.

### Step 1 — Shared wire types in `acp-utils`

1. Create `crates/acp-utils/src/git_diff.rs` (or extend `notifications.rs`;
   prefer a new module re-exported from `lib.rs` to keep `notifications.rs`
   from growing further):
   - `GitDiffSnapshot { scope: DiffScope, document: DiffDocument, error_message: Option<String> }`
     with `from_state(&RepositoryState) -> Self` helper (agent side; gate the
     `clankerdiff-watch` import behind an `agent`-only feature or put the
     constructor in `aether-cli` — **do not** add clankerdiff deps to the base
     `acp-utils` client feature; wisp already depends on `clankerdiff-core`
     via `clankerdiff-ratatui::diff` re-exports for `DiffScope`/`DiffDocument`).
   - `GitDiffErrorBody { kind: String, message: String }` + `from_git(&GitError)` /
     `from_watch(&WatchError)` constructors mapping discriminants to stable
     `kind` strings (keep the `Display` message as `message`).
   - The six request/notification structs above with `#[request]` /
     `#[notification]` derives and `_aether/*` method names.
2. Extend `AetherCapabilities` with `git_diff: bool` (serde defaulted, so old
   payloads parse as `false`).
3. Add unit tests in-module: method-name assertions (follow the existing
   `wire_method_names_are_prefixed` test), serde round-trips for every new
   type, `GitDiffErrorBody` mapping for each `GitError`/`WatchError` variant
   (construct variants directly — no real repo needed).

### Step 2 — Client-side receive path (`acp-utils`)

1. Add `AcpEvent::GitDiffUpdate(GitDiffUpdateParams)` in `client/event.rs`.
2. Handle it in `ClientHandlers::handle_dispatch_from` (`client/session.rs`):
   `.if_notification(async |params: GitDiffUpdateParams| emit(AcpEvent::GitDiffUpdate(params)))`.
   No other client changes needed (`AcpClientHandle::request` is generic).

### Step 3 — Agent-side `GitDiffService` (`aether-cli`)

1. Add `clankerdiff-git` + `clankerdiff-watch` to `crates/aether-cli/Cargo.toml`
   (versions pinned to workspace `0.2.1`, matching wisp today).
2. New file `crates/aether-cli/src/acp/session/git_diff.rs`:
   ```rust
   pub struct GitDiffService {
       session_id: String,
       repository: Option<GitRepository>,
       watcher: Option<RepositoryWatcher>,
       latest: Option<GitDiffSnapshot>,   // retained for reattach replay
       scope: DiffScope,
   }
   impl GitDiffService {
       pub fn new(session_id: String) -> Self;
       pub async fn open(&mut self, cwd: &Path, scope: DiffScope) -> Result<GitDiffSnapshot, GitDiffErrorBody>;
       pub async fn set_scope(&mut self, cwd: &Path, scope: DiffScope) -> Result<GitDiffSnapshot, GitDiffErrorBody>;
       pub async fn apply(&mut self, action: RepositoryAction) -> Result<GitDiffSnapshot, GitDiffErrorBody>;
       pub fn close(&mut self);           // drop watcher + repository
       pub fn latest(&self) -> Option<&GitDiffSnapshot>;
       pub fn take_updates(&mut self) -> Option<watch::Receiver<RepositoryState>>; // actor subscribes once per open
   }
   ```
   - `open`: `GitRepository::discover(cwd)` → `RepositoryWatcher::spawn(repo, scope, WatchOptions::default())`
     → snapshot via `from_state`. On `WatchError::Git(e)` return the error body;
     the actor maps unknown-session / invalid-params to ACP errors.
   - The actor (not the service) owns the `tokio::spawn` loop that forwards
     `watch::Receiver<RepositoryState>` changes into `SessionIo::send(GitDiffUpdateParams)`,
     so sends automatically stop on detach and the task dies with the actor.
3. Wire into `session/actor.rs`:
   - New `SessionCommand::{ GitDiffOpen { scope, reply }, GitDiffSetScope { scope, reply }, GitDiffApply { action, reply }, GitDiffClose }`
     (replies are `oneshot::Sender<Result<GitDiffSnapshot, GitDiffErrorBody>>`).
   - Actor loop owns `GitDiffService`; on `Attach { replay: true, .. }`, if
     `latest()` is `Some`, re-send it as `GitDiffUpdateParams`.
   - `cwd` for discover comes from the actor's existing `cwd` field (same dir
     used for the agent runtime).
4. Wire into `acp/agent.rs` dispatch: `if_request` arms for the four
   request/notification types, routing through `AcpState` → registry lookup →
   `SessionCommand` send (same shape as `on_mcp_request` / `workspace_list`).
   Unknown session → `Error::invalid_params().data("unknown session: …")`.
5. Wire into `acp/state.rs`:
   - `initialize()` advertises `AetherCapabilities { git_diff: true, .. }`.
   - New methods `git_diff_open / git_diff_set_scope / git_diff_apply / git_diff_close`
     following the `workspace_list`/`workspace_move` pattern (registry lookup,
     oneshot round-trip to the actor).
   - New `workspace_status()` method: resolve `session_cwd` from the store,
     run `git branch --show-current` (fallback `rev-parse --short HEAD`) via
     `tokio::process::Command` in `spawn_blocking`/async task — i.e. move the
     body of `wisp::runtime::git::resolve_workspace_status` here. Reuse the new
     `workspace/git.rs`-adjacent helper if one exists; otherwise add
     `workspace/git_status.rs` with a pure function + unit tests using temp repos.
6. Extend `AcpTestHarness` (`acp/testing.rs`) + `TestPeer` (`acp-utils/testing.rs`)
   minimally: a `next_git_diff_notification()` receiver for
   `GitDiffUpdateParams` (mirrors `next_mcp_notification`).

### Step 4 — Wisp: replace local backend with ACP calls

1. `command.rs`:
   - Delete `GitCommand::Apply` (local `RepositoryAction`) → replace with
     `AgentCommand::GitDiffApply { session_id: SessionId, action: RepositoryAction }`.
   - Change `GitWatchCommand::{Open, Refresh}` to carry `session_id` instead of
     `working_dir: PathBuf` (`Close` unchanged).
   - `CommandResult::GitWatchStarted` (which smuggles `GitRepository` +
     `RepositoryWatcher` across the task boundary) is deleted; `GitWatch` /
     `GitDiff` results stay with the same shape so `App::on_command_result`
     and `GitDiffScreen` are untouched at this step.
2. `runtime/dispatcher.rs`: delete all `clankerdiff_git` / `clankerdiff_watch` /
   `WatchStream` / `git_review` state fields and `start_git_watch` /
   `close_git_review` / `watch_stopped`. New behavior:
   - `GitWatch(Open|Refresh)` → `agent::execute`-style `handle.request(GitDiffOpenParams / GitDiffSetScopeParams)`
     via `tasks.submit_network`, completing to `CommandResult::GitWatch` /
     `GitDiff` (map `GitDiffSnapshot` → local `GitWatchEvent`/`GitDiffEvent`).
   - `GitDiffApply` → `handle.request(GitDiffApplyParams)` → `CommandResult::GitDiff`
     + `CommandResult::GitWatch` with the returned snapshot.
   - `GitWatch(Close)` → `handle.notify(GitDiffCloseParams)`.
   - `ResolveWorkspace { cwd }` → needs `session_id`; change to
     `ResolveWorkspace { session_id }` requesting `_aether/workspace_status`.
     (Wisp no longer shells out to git at all — delete `runtime/git.rs`.)
   - `has_pending_tasks` / `next_result` simplify to pure `tasks` polling
     (the `poll_fn` + `git_updates` select goes away).
3. `runtime/tasks.rs`: delete `ReadTask::GitStart` / `GitRefresh`
   (+ `spawn_git_mutation` + `git_mutations` map — serialization now lives in
   the agent actor) if nothing else uses them.
4. `git_review/protocol.rs`: redefine without clankerdiff errors:
   ```rust
   pub struct GitWatchEvent { pub review_id: RequestId, pub result: Result<GitDiffSnapshot, GitDiffErrorBody> }
   pub struct GitDiffEvent  { pub review_id: RequestId, pub result: Result<(), GitDiffErrorBody> }
   ```
   and adapt `screens/git_diff/mod.rs::on_watch_event` / `on_event` to render
   `snapshot.document` / `snapshot.scope` / `error_message()` from the wire
   snapshot (mechanical field renames; keep the `installed: Arc<RepositorySnapshot>`
   dedup logic by comparing `document` instead — or store the last
   `GitDiffSnapshot` directly).
5. `app/input.rs`: replace the `WorkspaceAccess::Remote` block on
   `toggle_git_diff` with the capability gate; construct
   `GitDiffScreen::new()` without a `working_dir` (it now only needs scope —
   change `GitDiffScreen::new(working_dir: PathBuf)` to `new()` and drop the
   `working_dir` from `GitWatchCommand::Open`). Same for `resolve_workspace` in
   `app/session.rs` (send `session_id`, not `cwd`).
6. `Cargo.toml` (wisp): remove `clankerdiff-git`, `clankerdiff-watch`;
   keep `clankerdiff-core`, `clankerdiff-ratatui`.
7. `session/session_model.rs`: expose `capabilities().git_diff` (already stores
   `AetherCapabilities`; no change needed beyond Step 1).

### Step 5 — Wisp test harness + existing tests

1. `testing.rs` `FakeExecutor`: replace the `FakeGit`-backed `watch_git`/`complete`
   logic with an ACP-fake: record `AgentCommand::GitDiff*` commands, serve queued
   `GitDiffSnapshot`s, synthesize `GitWatchEvent`s. Keep the public `FakeGit`
   type (and its in-memory semantics) but move it to `aether-cli`'s test support
   for agent-side tests — or keep a copy in wisp tests only if `git_contract.rs`
   still needs it (see below).
2. `tests/git_contract.rs`: the `resolve_workspace_status` branch test moves to
   `aether-cli` next to the new `workspace_status` helper; the clankerdiff
   parse/apply contract tests move with the backend (agent-side integration test
   running real `git` in a tempdir). Wisp keeps only rendering/model tests
   (`diff_model.rs`, `tests/tui/git_diff.rs`) rewired to the `FakeExecutor` ACP
   fake.
3. `tests/tui/git_diff.rs`, `review_rendering.rs`, `foundation.rs`: update
   drivers that open `GitDiffScreen::new(working_dir)` to the new `new()` +
   snapshot-injection helpers.

### Step 6 — New integration tests (agent side)

In `crates/aether-cli/tests/integration/` (new file `acp_git_diff.rs`,
following the `acp_remote.rs` harness pattern with `LocalSet`):

1. Open against a temp git repo (reuse `workspace::testing::{init_repo, git}`
   helpers): `git_diff_open` returns a snapshot containing the modified file
   with expected hunk lines.
2. External change → `GitDiffUpdateParams` notification arrives (write a file
   after open, await notification with debounce in mind — assert on state, no
   timeouts; drive via the harness peer channel).
3. `git_diff_apply(StageAll)` then `Commit` → snapshot goes clean; verify with
   `git status --porcelain` on the temp repo.
4. Scope switching (`Both` → `Staged`) returns the staged-only document.
5. Non-repository cwd → `notRepository` error body; unknown session →
   `invalid_params`.
6. `workspace_status` returns the temp repo's branch (`main` in test repos) and
   `None` outside a repo.
7. Remote end-to-end (optional, behind the existing websocket harness):
   `serve_websocket` + real client `git_diff_open` over the socket.

### Step 7 — Cleanup + docs

1. Delete `crates/wisp/src/runtime/git.rs`; remove `pub use git::resolve_workspace_status`
   from `runtime/mod.rs`; fix all callers.
2. Update `crates/wisp/README.md` (remove any "local git only" notes) and
   `crates/aether-cli/README.md` (document the new `_aether/git_diff_*` +
   `_aether/workspace_status` methods in the extension-protocol section, if one
   exists — check before writing).
3. CHANGELOG entries via the repo's normal release flow (do not hand-edit
   per-crate CHANGELOGs if they are release-plz managed — verify).

## Testing Plan

| Layer | What | Where |
|---|---|---|
| Unit (acp-utils) | Method names, serde round-trips, error-kind mapping for every `GitError`/`WatchError` variant | `crates/acp-utils/src/git_diff.rs` `#[cfg(test)]` |
| Unit (agent) | `GitDiffService` open/scope/apply/close against temp repos (real `git` binary, like wisp's current `git_contract.rs`); `workspace_status` branch/HEAD/non-repo cases | `crates/aether-cli/src/acp/session/git_diff.rs` + `workspace/git_status.rs` tests |
| Contract (moved) | Parse/apply parity (modified/staged/untracked/rename/binary, stage-commit round-trip, empty-commit + not-a-repo errors) now exercising the agent backend | New `acp_git_diff.rs` integration tests (Step 6) |
| Integration (ACP) | Full serialize/dispatch path over duplex transport: open → snapshot, external write → update notification, apply → mutation, detach → reattach replays latest, close → no more updates | `acp_git_diff.rs` with `AcpTestHarness` + extended `TestPeer` |
| TUI/model (wisp) | Diff screen renders remote snapshots, scope change emits `GitDiffSetScope`, stage key emits `GitDiffApply`, error bodies render as background/primary errors, capability-gated toggle | Rewired `tests/tui/git_diff.rs`, `diff_model.rs` via ACP `FakeExecutor` |
| Regression | `just test` full workspace; `just lint`, `just fmt` clean | CI |

Edge cases to verify explicitly:

- Empty diff (clean tree) → snapshot with empty `document.files`, screen shows empty state, not an error.
- Rapid external writes → debounced single update (assert final state, never timing).
- Apply while a watcher reload is in flight → actor serializes; UI settles on the
  apply response snapshot.
- Scope change on a closed/stale review → wisp ignores by `review_id`
  (existing logic); agent treats unknown session as `invalid_params`.
- Non-UTF8 paths / binary files / renames → covered by moved contract tests.
- Detached client: no send panics (`SessionIo` drops); reattach replays latest.
- Server without git binary → `Spawn` error body surfaces as background error,
  screen stays usable.
- Old client ↔ new agent and vice versa: unknown `_aether/git_diff_*` methods
  must not crash either side (ACP `method_not_found` path); capability gate
  prevents the UI from opening the screen against old agents.

## Files to Modify/Create

| File | Change | Kind |
|---|---|---|
| `crates/acp-utils/src/git_diff.rs` (new) or `notifications.rs` | Wire types: `GitDiffSnapshot`, `GitDiffErrorBody`, 6 method structs, `AetherCapabilities::git_diff` | Add (prefer new module) |
| `crates/acp-utils/src/lib.rs` | Export new module | Modify |
| `crates/acp-utils/src/client/event.rs` | Add `AcpEvent::GitDiffUpdate` | Modify |
| `crates/acp-utils/src/client/session.rs` | Dispatch `_aether/git_diff_update` → event | Modify |
| `crates/acp-utils/src/testing.rs` | `TestPeer::next_git_diff_notification()` | Modify |
| `crates/aether-cli/Cargo.toml` | Add `clankerdiff-git`, `clankerdiff-watch` | Modify |
| `crates/aether-cli/src/acp/session/git_diff.rs` | `GitDiffService` (discover/spawn/apply/close/retain) | Add |
| `crates/aether-cli/src/acp/session/actor.rs` | `SessionCommand` git variants, actor ownership, detach-replay, update-forward task | Modify |
| `crates/aether-cli/src/acp/agent.rs` | Dispatch 4 new methods/notifications | Modify |
| `crates/aether-cli/src/acp/state.rs` | `git_diff_*` + `workspace_status` methods, advertise `git_diff: true` | Modify |
| `crates/aether-cli/src/workspace/git_status.rs` (new, or extend `git.rs`) | Branch/HEAD resolution moved from wisp | Add |
| `crates/aether-cli/src/acp/testing.rs` | Harness support for git-diff sessions | Modify |
| `crates/aether-cli/tests/integration/acp_git_diff.rs` | New integration suite (Step 6) | Add |
| `crates/wisp/Cargo.toml` | Remove `clankerdiff-git`, `clankerdiff-watch` | Modify |
| `crates/wisp/src/command.rs` | `GitCommand`→`AgentCommand::GitDiffApply`; `GitWatchCommand` carries `session_id` not `working_dir`; delete `GitWatchStarted` | Modify |
| `crates/wisp/src/runtime/dispatcher.rs` | Delete local repo/watcher state; route through `AcpClientHandle` | Modify |
| `crates/wisp/src/runtime/git.rs` | Delete (moved to agent) | Remove |
| `crates/wisp/src/runtime/mod.rs` | Drop `resolve_workspace_status` re-export / `mod git` | Modify |
| `crates/wisp/src/runtime/tasks.rs` | Delete `GitStart`/`GitRefresh`, `spawn_git_mutation` if unused | Modify |
| `crates/wisp/src/runtime/agent.rs` | Execute new `AgentCommand::GitDiff*` via `handle.request/notify` | Modify |
| `crates/wisp/src/git_review/protocol.rs` | Wire-typed `GitWatchEvent`/`GitDiffEvent` (no clankerdiff errors) | Modify |
| `crates/wisp/src/screens/git_diff/mod.rs` | Consume `GitDiffSnapshot`; `new()` without `working_dir` | Modify |
| `crates/wisp/src/app/input.rs` | Capability gate replaces `Remote` block; `GitWatch::Open` with `session_id` | Modify |
| `crates/wisp/src/app/session.rs` + `app/mod.rs` | `ResolveWorkspace { session_id }`; capability plumbing | Modify |
| `crates/wisp/src/testing.rs` | `FakeExecutor` ACP fake; relocate `FakeGit` usage | Modify |
| `crates/wisp/tests/git_contract.rs` | Split: rendering stays, git-backend cases move to agent | Modify |
| `crates/wisp/tests/tui/git_diff.rs` (+ related tui tests) | Drive via ACP fake snapshots | Modify |
| `crates/wisp/README.md`, `crates/aether-cli/README.md` | Document new behavior + extension methods | Modify |

## Additional Notes

- **No other files change.** In particular, `clankerdiff-ratatui` rendering,
  `DiffReviewState`, keybindings, `ReviewOutcome::Submitted → prompt` flow, and
  plan-review screens are untouched — this is a backend move, not a UI redesign.
- **Dependency direction:** `acp-utils` must not gain `clankerdiff-git/watch`
  deps in its default/client features (it would drag the git backend back into
  wisp builds). Wire constructors that need `RepositoryState`/`GitError` live in
  `aether-cli`. `DiffScope`/`DiffDocument`/`RepositoryAction` reach `acp-utils`
  through `clankerdiff-core` (check whether that means adding a light
  `clankerdiff-core` dep to `acp-utils`, or defining wire-local mirrors — prefer
  the direct dep if version-aligned at 0.2.1).
- **Follow-ups (separate issues):** upstream clankerdiff serde proposal (see
  above); moving `@` file index / attachments to agent-side for full remote
  parity; multi-watch per session if a use case ever needs two concurrent diff
  screens.
- **Risk:** `DiffDocument` payload size for huge repos (binary/source caps
  already exist in clankerdiff-git: `MAX_SOURCE_ARCHIVE_BYTES`). The
  notification path sends full snapshots; if profiling shows pain, add
  revision/ETag + delta follow-up — do not pre-optimize.
- **Clarifying questions for the issue author** (non-blocking, confirm in PR):
  1. Is one active diff watch per session sufficient, or must two clients/TUIs
     ever view different scopes of the same session concurrently?
  2. Should git mutations from the diff screen be attributed in the session
     transcript (like review-submission prompts are), or stay silent?
