# Plan: Support TypeScript 7 (`tsc --lsp`) as default TS LSP (issue #494)

## Overview

### Problem statement

TypeScript 7 ships a native language server inside the `tsc` binary (`tsc --lsp --stdio`,
verified on `tsc` 7.0.2 in this environment). It is significantly faster than the current
default, `typescript-language-server` (a Node wrapper around tsserver). We should make the
native server the default/preferred LSP for the whole TS family (JavaScript, JSX,
TypeScript, TSX) while keeping `typescript-language-server` as a fallback, and add **pull
diagnostics** (`textDocument/diagnostic`) support alongside the existing **push-only**
(`textDocument/publishDiagnostics`) pipeline.

Today the daemon is push-only end to end:

- `ProcessTransportActor::handle_lsp_message` (`crates/aether-lspd/src/process_transport.rs:323-328`)
  matches `PublishDiagnostics::METHOD` and emits `TransportEvent::PublishedDiagnostics`.
- `WorkspaceSession::run_session_events` (`workspace_session.rs:255-257`) stores it in
  `DiagnosticsStore` (`diagnostics_store.rs:22-27`).
- Consumers pull from that cache via `DaemonRequest::GetDiagnostics → WorkspaceRegistry::
  get_diagnostics → WorkspaceSession::get_diagnostics → DiagnosticsStore::get`, with
  freshness provided by `wait_for_uri_fresh` (600 ms settle window, `diagnostics_store.rs:8,51-95`).
- `initialize` advertises only `publishDiagnostics.relatedInformation`
  (`process_transport.rs:216-245`). There is no `textDocument.diagnostic` /
  `workspace.diagnostic` client capability, no `textDocument/diagnostic` request is ever
  issued, and unknown server→client requests (e.g. `workspace/diagnostic/refresh`) get a
  `Method not found` (-32601) error (`process_transport.rs:291-301`).

### Empirical findings (probed against installed `tsc` 7.0.2, do not re-derive from docs)

- LSP entry point: `tsc --lsp --stdio` (`tsc --lsp --help` shows `-stdio | -socket | -pipe`).
  Plain `tsc --lsp` without `--stdio` prints usage and does nothing.
- `initialize` result advertises `"diagnosticProvider": {"identifier":"typescript",
  "interFileDependencies":true,"workspaceDiagnostics":false}`, `serverInfo.name =
  "typescript-go"`. So **document pull is supported, workspace pull is not**.
- `textDocument/diagnostic` after `didOpen` returns
  `{"kind":"full","items":[{range, severity:1, code:2322, source:"ts", message:"Type
  'string' is not assignable to type 'number'."}]}` — standard `lsp-types`
  `DocumentDiagnosticReport` shape. `lsp-types 0.97` (the single workspace version,
  `Cargo.toml:68`) already has all needed types (`DocumentDiagnosticParams`,
  `DocumentDiagnosticReport`, `DocumentDiagnosticReportResult`, `DiagnosticClientCapabilities`,
  `DiagnosticWorkspaceClientCapabilities`, `DocumentDiagnosticRequest`,
  `WorkspaceDiagnosticRefresh`).
- Push still exists: the server emits `textDocument/publishDiagnostics` (observed for
  `tsconfig.json`; document publishes flow through the normal notification path) **and**
  answers pull. So the correct design is **pull-after-open merged with the push cache**,
  not pull-instead-of-push.
- Server→client traffic observed: `client/registerCapability` for
  `workspace/didChangeConfiguration` (id `ts1`, harmless — current code replies OK to
  unknown registrations) and `window/logMessage` spam. `workspace/diagnostic/refresh`
  was not observed (consistent with `workspaceDiagnostics:false`), but the server is
  entitled to send it, so we must answer it instead of returning `Method not found`.
- `tsc` resolves the same way `typescript-language-server` does today: repo-local
  `node_modules/.bin/tsc` (via existing `resolve_command` in `process_transport.rs:396-404`)
  or `PATH` (`/usr/local/bin/tsc` in this environment). No new resolution machinery needed.

### Success criteria / acceptance conditions

1. Fresh checkout with only TypeScript ≥ 7 installed (no `typescript-language-server`)
   gets working TS/JS diagnostics, hover, definition, references, document symbols through
   `lsp_check_errors` / `lsp_symbol` — i.e. `tsc --lsp --stdio` is used by default.
2. `lsp_check_errors` for a TS file with a type error returns the error on the **first**
   call after `didOpen` (no polling), sourced from pull when push has not arrived yet.
3. Existing push-only servers (`typescript-language-server` fallback, rust-analyzer,
   pyright, gopls, clangd, fake test server) behave exactly as before — no extra latency
   or errors when pull is unsupported (server answers `Method not found`).
4. `cargo test -p aether-lspd` and the `mcp-servers` TS e2e suites pass against real
   `tsc` 7; contract tests reflect the new install guidance.
5. `just lint`, `just fmt`, `just check` clean.

## Technical Approach

### Architectural decisions

1. **New `ServerKind::TypeScriptNative` alongside the existing
   `ServerKind::TypeScriptLanguageServer`** (`language_catalog.rs:87-116`). All four TS-family
   languages (`JavaScript`, `JavaScriptReact`, `TypeScript`, `TypeScriptReact`) point at the
   native kind by default. Do **not** reuse one `ServerKind` with different args: the two
   servers are different processes with different commands, different socket identities, and
   different capability sets. Separate kinds keep `WorkspaceKey`, `socket_path`,
   `LspRegistry::slot`, and env overrides correct for free.
2. **Spawn-time fallback, not version probing.** `WorkspaceRegistry::get_or_spawn`
   (`workspace_registry.rs:113-140`) tries `tsc --lsp --stdio`; on `LspSpawnFailed` it falls
   back to `typescript-language-server --stdio`. Rationale: no `tsc --version` subprocess on
   the hot path, no PATH/`node_modules/.bin` duplication (reuse `resolve_command`), and a
   broken/old `tsc` degrades gracefully. Env overrides (`AETHER_LSPD_SERVER_COMMAND_…` /
   `…_ARGS_…`, consumed generically in `resolved_config_for_language`,
   `language_catalog.rs:491-508`) let users/tests force either server.
3. **Pull supplements push; push stays authoritative for non-TS servers.** New flow in
   `WorkspaceSession::get_diagnostics` for a file URI: ensure-open → **try pull**
   (`textDocument/diagnostic` via the existing `transport.request_raw`) → convert report to
   `Diagnostic`s → `diagnostics.publish` into the same `DiagnosticsStore` → return merged
   cache. Push notifications keep flowing through the unchanged handler. This reuses the
   version/settle machinery and the `PublishDiagnosticsParams` formatting path in
   `mcp-servers` untouched.
4. **Lazy pull-capability detection per session, no `initialize` plumbing.** Parsing and
   threading `InitializeResult.diagnosticProvider` out of `ProcessTransportActor::initialize`
   would touch the actor, `ProcessTransport`, and `WorkspaceSession::spawn` signatures.
   Instead the session keeps an `AtomicBool`/tri-state (`PullSupport::Unknown | Supported |
   Unsupported`); the first pull that fails with LSP `Method not found` (-32601) or
   `Request cancelled`/invalid-params flips it to `Unsupported` and all later calls skip
   pulling (push-only path, zero added latency). Any successful pull flips to `Supported`.
   Simple, robust against servers that advertise pull but fail per-document, and trivially
   unit-testable.
5. **Advertise pull capabilities in `initialize`** (`process_transport.rs:216-245`): add
   `text_document.diagnostic = Some(DiagnosticClientCapabilities {
   dynamic_registration: None, related_document_support: Some(true) })` and
   `workspace.diagnostic = Some(DiagnosticWorkspaceClientCapabilities {
   refresh_support: Some(true) })`. Verified field names against `lsp-types 0.97` sources
   (`TextDocumentClientCapabilities.diagnostic`, `WorkspaceClientCapabilities.diagnostic`).
   Without this, `tsc` logs reduced capabilities and may withhold pull results.
6. **Answer `workspace/diagnostic/refresh`** in `ProcessTransportActor::handle_lsp_message`:
   reply OK (like `WorkDoneProgressCreate`) and emit a new
   `TransportEvent::DiagnosticRefreshRequested` (or `::PullRefreshRequested`) that
   `run_session_events` turns into `refresh.enqueue(all known open docs)`-ish behavior.
   Minimal viable: reply OK + enqueue a refresh generation bump so the next
   `get_diagnostics(None)` is not stale. Full per-document re-pull happens lazily on the
   next `get_diagnostics` anyway.

### Design patterns to employ

- Existing patterns only: `TransportEvent` enum extension + `run_session_events` match arm
  (same as `PublishedDiagnostics` / `FileWatcherBatch`); `LspConfig` builder for the new
  server spec; builder-pattern test helpers already used in `testing.rs`.
- Conversion as a pure free function `fn pull_report_to_diagnostics(report:
  DocumentDiagnosticReportResult, uri: &Uri) -> PullDiagnostics` returning `(Vec<Diagnostic>,
  Option<String> /* result_id */, Vec<(Uri, Vec<Diagnostic>)> /* related */)` — pure,
  unit-tested without I/O, placed near the bottom of `workspace_session.rs` (per repo style:
  `pub` items top, private helpers bottom). Related documents publish into the store under
  their own URIs.
- `unchanged` handling without result-id bookkeeping: send `previous_result_id: None` on
  every pull, so a well-behaved server always answers `full`. If `unchanged` arrives anyway,
  keep the cached entry (do not clear). No `HashMap<Uri, String>` result-id cache — not
  worth the state for v1.

### Key technical considerations and trade-offs

- **Push/pull race (last-writer-wins).** A push arriving between pull-response and `get`
  overwrites the pulled entry or vice versa. Acceptable: both originate from the same server
  state for the same open document version; versions only diverge under concurrent edits,
  where the next `get_diagnostics` re-pulls. Do not add merge/conflict logic.
- **Do not change the daemon protocol.** `DaemonRequest::GetDiagnostics` keeps its shape;
  pull happens entirely inside `WorkspaceSession`. No `client.rs`, `protocol.rs`, or
  `mcp-servers/src/lsp/registry.rs` changes required.
- **Socket identity changes.** `socket_identity_for_language` for the TS family becomes
  `"tsc"` (the new kind's `as_str`) instead of `"typescript-language-server"`. Old sockets
  are orphaned harmlessly (daemon idle-timeout reaps them). Tests asserting the old socket
  name / shared identity need updating (see steps).
- **Old `tsc` (< 7) without `--lsp`.** Spawn succeeds but the process exits / fails
  `initialize` → transport `Closed` → existing wedge/replace logic treats it as a dead
  session; fallback to TLS triggers on spawn failure. If `tsc` 6 stays resident but never
  answers `initialize`, the per-request timeout + `declare_wedged` path still recovers, but
  every first request pays the timeout. Mitigation: fallback also triggers when the
  transport closes during `initialize` (same `Err` arm), not only on process-spawn failure.
- **Performance.** Pull adds one RTT to file-scoped `get_diagnostics` only when the session
  believes pull is supported. Push-only servers pay nothing after the first probe. The
  existing 600 ms settle wait (`DIAGNOSTICS_SETTLE_DURATION`) stays as the push fallback;
  when pull succeeds we can return immediately without the settle wait (freshness comes from
  the response itself) — keep the settle wait only for the push-fallback branch.
- **Backwards compat explicitly not a concern** (repo guidance): old socket names, old
  install strings, and `typescript-language-server`-specific test pins are updated, not
  shimmed.

## Implementation Steps

1. **Catalog: add the native server kind and repoint the TS family.**
   File: `crates/aether-lspd/src/language_catalog.rs`.
   - Add `ServerKind::TypeScriptNative` with `as_str() → "tsc"`, `env_key() →
     "TYPESCRIPT_NATIVE"` (yields `AETHER_LSPD_SERVER_COMMAND_TYPESCRIPT_NATIVE` /
     `AETHER_LSPD_SERVER_ARGS_TYPESCRIPT_NATIVE` via the existing generic machinery).
   - Add `ServerSpec { kind: TypeScriptNative, command: "tsc", display_name: "TypeScript
     native language server (tsc)", args: &["--lsp", "--stdio"], installation_instructions:
     Some("Install TypeScript 7+ … `npm install --save-dev typescript@^7` …") }`.
   - Repoint `LanguageSpec.server_kind` for `JavaScript`, `JavaScriptReact`, `TypeScript`,
     `TypeScriptReact` to `Some(ServerKind::TypeScriptNative)`.
   - Keep the `TypeScriptLanguageServer` spec untouched as the fallback (update its install
     text only if it references versions).
   - Update unit tests in-file: `get_config_for_known_languages` now expects `command ==
     "tsc"`, `args == ["--lsp", "--stdio"]`; `typescript_family_shares_server_kind` still
     passes; add `native_and_legacy_have_distinct_socket_identities` asserting
     `TypeScriptNative.as_str() != TypeScriptLanguageServer.as_str()`.
   - Update `src/docs/language_catalog.md` table row + pooling paragraph (`tsc` instead of
     `typescript-language-server`).

2. **Fallback resolution in the registry.**
   File: `crates/aether-lspd/src/workspace_registry.rs` (`get_or_spawn`, lines ~113-140).
   - After building the primary config via `resolved_config_for_language`, if
     `server_kind_for_language(binding.language) == Some(TypeScriptNative)`, attempt
     `WorkspaceSession::spawn` with it; on `Err(DaemonError::LspSpawnFailed(_))` **or**
     transport-closed-during-init, resolve the legacy TLS config (same language → look up
     the TLS `ServerSpec` directly, honoring `AETHER_LSPD_SERVER_COMMAND_TYPESCRIPT_LANGUAGE_SERVER`
     overrides) and spawn that instead.
   - Factor a small helper `fn spawn_for_command(root, command, args, extensions)` to avoid
     duplicating the `Arc::new(WorkspaceSession::spawn(…))` + insert block.
   - Pseudo-code:
     ```rust
     let primary = resolved_config_for_language(binding.language)?;
     match spawn_session(&binding.key.workspace_root, &primary) {
         Ok(s) => insert(s),
         Err(e) if is_ts_native(binding.language) => {
             tracing::warn!(%e, "tsc LSP unavailable, falling back to typescript-language-server");
             let legacy = legacy_ts_config()?; // honors TLS env overrides
             insert(spawn_session(&root, &legacy)?)
         }
         Err(e) => Err(e),
     }
     ```
   - Add unit test: `WorkspaceKey` sharing unchanged (existing test stays); fallback itself
     is covered by integration tests (step 8), not unit tests (spawning is I/O).

3. **Advertise pull capabilities at initialize.**
   File: `crates/aether-lspd/src/process_transport.rs` (`initialize`, lines ~216-245).
   - Add imports: `lsp_types::{DiagnosticClientCapabilities, DiagnosticWorkspaceClientCapabilities}`.
   - Set `text_document.diagnostic = Some(DiagnosticClientCapabilities {
     dynamic_registration: None, related_document_support: Some(true) })`.
   - Set `workspace.diagnostic = Some(DiagnosticWorkspaceClientCapabilities {
     refresh_support: Some(true) })`.
   - No other capability changes. Verify serialized JSON contains
     `"diagnostic":{"relatedDocumentSupport":true}` and `"workspace":{"diagnostic":
     {"refreshSupport":true}}` with a serialization unit test (construct the same
     `ClientCapabilities` the actor builds — factor a `fn base_client_capabilities() -> ClientCapabilities`
     so the test does not duplicate the literal).

4. **Handle server→client `workspace/diagnostic/refresh`.**
   File: `crates/aether-lspd/src/process_transport.rs` (`TransportEvent`,
   `handle_lsp_message` match arms ~273-331).
   - New variant `TransportEvent::DiagnosticRefreshRequested`.
   - In the `(true, Some(method))` arm add
     `WorkspaceDiagnosticRefresh::METHOD => { let _ = self.send_ok_response(&id).await; let _
     = self.event_tx.send(TransportEvent::DiagnosticRefreshRequested).await; }`
     (import `lsp_types::request::WorkspaceDiagnosticRefresh` + `Request` trait for `METHOD`).
   - File: `crates/aether-lspd/src/workspace_session.rs` (`run_session_events`, ~253-289):
     on `DiagnosticRefreshRequested`, re-enqueue refresh for currently tracked open
     documents. `DocumentLifecycle` has no "list open" API — add `pub(crate) fn open_uris(&self)
     -> Vec<Uri>` (lock, filter `open_holders > 0`). Then `refresh.enqueue(open_uris)`.
   - Unit test `open_uris` in `document_lifecycle.rs` tests module.

5. **Pull-on-read in the session + pull-support tri-state.**
   File: `crates/aether-lspd/src/workspace_session.rs`.
   - Add field `pull_support: Arc<AtomicU8>` (0 unknown, 1 supported, 2 unsupported) or a
     tiny `PullSupport` enum behind `AtomicU8`; default Unknown. Thread through `spawn`
     (constructed there, no signature change for callers).
   - Rewrite `sync_documents_for_diagnostics(uri: Some)` path: after `ensure_document_open`
     returns `Some(version_before)`, call new `pull_document_diagnostics(&self.transport,
     &self.pull_support, uri).await`:
     - If pull supported/unknown → `transport.request_raw("textDocument/diagnostic",
       json!({"textDocument":{"uri":uri}}))`. Use `lsp_types::request::DocumentDiagnosticRequest::METHOD`
       for the method string and `DocumentDiagnosticParams { text_document:
       TextDocumentIdentifier{uri}, identifier: None, previous_result_id: None, ..Default::default() }`
       for params (serialize via `serde_json::to_value`).
       - `Ok(value)` → deserialize to `DocumentDiagnosticReportResult`; on success mark
         Supported, convert (step 6), `diagnostics.publish` a synthesized
         `PublishDiagnosticsParams { uri, diagnostics, version: None }` (+ related docs),
         then return cache **without** the 600 ms settle wait.
       - `Err(TransportError::Lsp(e)) if e.code == -32601 (MethodNotFound)` → mark
         Unsupported, fall through to existing `wait_for_uri_fresh` push path.
       - `Err(TransportError::Closed)` → propagate (existing wedge handling in
         `WorkspaceRegistry::get_diagnostics` covers it).
       - Deserialize failure → `tracing::debug!`, fall back to push path (do not mark
         Unsupported; the server may just shape-shift on one document).
     - If pull already Unsupported → existing push path unchanged.
   - `refresh_uri` (background worker, ~190-203): after `sync_document`, also attempt the
     same pull (best-effort, ignore errors) before `wait_for_uri_fresh`, so workspace-wide
     `get_diagnostics(None)` warms from pull too. Keep the existing waits untouched.
   - Timeouts: rely on the outer `WorkspaceRegistry::get_diagnostics` request-timeout
     wrapper; do not add a second timeout. Note: pull adds at most one server RTT before the
     existing 20 s `DIAGNOSTICS_TIMEOUT` budget.

6. **Pure report conversion.**
   File: `crates/aether-lspd/src/workspace_session.rs` (bottom, private).
   ```rust
   struct PulledDiagnostics { primary: Vec<lsp_types::Diagnostic>, related: Vec<(Uri, Vec<lsp_types::Diagnostic>)> }
   fn convert_pull_report(report: DocumentDiagnosticReportResult) -> PulledDiagnostics {
       match report {
           DocumentDiagnosticReportResult::Report(DocumentDiagnosticReport::Full(r)) =>
               PulledDiagnostics { primary: r.full_document_diagnostic_report.items,
                   related: flatten_related(r.related_documents) },
           DocumentDiagnosticReportResult::Report(DocumentDiagnosticReport::Unchanged(_)) =>
               // caller keeps cache; signal with empty + flag — simplest: return empty vecs
               // and let caller skip publish when `is_unchanged`
               PulledDiagnostics::UNCHANGED,
           DocumentDiagnosticReportResult::Partial(p) =>
               PulledDiagnostics { primary: vec![], related: flatten_related(p.related_documents) },
       }
   }
   ```
   - `flatten_related`: `Option<HashMap<Uri, DocumentDiagnosticReportKind>>` → keep only
     `Full` variants' items; drop `Unchanged`.
   - Caller: on `Unchanged`, skip `publish` (cache already holds last full). On `Full`,
     `diagnostics.publish(PublishDiagnosticsParams { uri: uri.clone(), diagnostics: primary,
     version: None })` and publish each related pair.
   - Unit tests (no I/O): full-with-items, full-empty (clears stale errors — publish with
     empty vec overwrites, which is exactly the "error fixed" path), unchanged-keeps-cache
     (assert caller skips publish), related-documents split, partial-result handling.

7. **Fake-server pull support for hermetic tests.**
   File: `crates/aether-lspd/tests/common/fake_lsp_server.py`.
   - `initialize` result: add `"diagnosticProvider": {"identifier": "fake", "interFileDependencies":
     false, "workspaceDiagnostics": false}` to capabilities so capability-sniffing tests (if
     any) see pull support.
   - Add `textDocument/diagnostic` handler mirroring `publish()` semantics: return
     `{"kind":"full","items":[{range 0,0-0,5, severity 1, message "error token"}]}` when the
     stored doc text contains "error" (case-insensitive), else `{"kind":"full","items":[]}`.
     Unknown URIs → empty full report (not an error).
   - Add `workspace/diagnostic/refresh` client-request simulation? Not needed — the daemon
     receives it from the server, never sends it in tests. Cover the daemon side with a Rust
     test driving `ProcessTransportActor::handle_lsp_message` indirectly or via a scripted
     fake that sends the request (optional; at minimum assert the OK response + event with a
     focused async test if the actor is reachable — otherwise cover via e2e).
   - New e2e tests in `crates/aether-lspd/tests/e2e/` (extend `e2e_lifecycle.rs` or new
     `e2e_pull_diagnostics.rs` wired into `main.rs`): pull returns diagnostics without waiting
     for push; fallback when fake started with `--fail-on textDocument/diagnostic` (push path
     still works); `unchanged` keeps cache (fake flag e.g. `--diagnostic-unchanged`).

8. **Real-`tsc` integration tests + test-project updates.**
   - `crates/aether-lspd/src/testing.rs`: bump `TYPESCRIPT_PACKAGE` to `typescript@7.0.2`
     (matches the workspace bump in #357 which moved the repo itself to 7.0.2 but left the test
     pin at 6.0.3), delete `TYPESCRIPT_LANGUAGE_SERVER_PACKAGE` usage from the default path;
     `NodeProject::new` installs only `typescript@7.0.2`. Keep a constructor or flag for a
     TLS-based project (needed for the fallback test): e.g. `NodeProject::new_with_legacy_server`
     installing both pins as today.
   - `crates/mcp-servers/tests/integration/lsp_ts_diagnostics_e2e.rs`: update module docs
     (`tsc --lsp` instead of `typescript-language-server`); existing three tests should pass
     unmodified against native — keep them as-is (they are server-agnostic). Add
     `test_ts_pull_diagnostics_single_call_returns_errors` (open + single
     `lsp_check_errors`, assert code 2322 surfaces) if not subsumed by the existing no-poll
     regression test.
   - `lsp_ts_operations_e2e.rs`: docs touch-up only; operations go through unchanged
     `request_raw`.
   - New fallback test: project built with `new_with_legacy_server` + env forcing
     `AETHER_LSPD_SERVER_COMMAND_TYPESCRIPT_NATIVE` to a nonexistent binary → diagnostics
     still work via TLS. Env mutation is process-global (`configure_fake_server` precedent):
     isolate in its own test binary or gate with serial execution + save/restore.
   - `.contextbridge/Dockerfile`: bump `TYPESCRIPT_VERSION` 6.0.3 → 7.0.2 and either drop
     `TYPESCRIPT_LANGUAGE_SERVER_VERSION` (preferred) or keep it solely for the fallback test
     image. Check `.config/nextest.toml` for TS-related setup too.

9. **Contract/install-text updates.**
   - `lsp_check_errors_contract.rs::lsp_check_errors_returns_typescript_installation_instructions_when_server_fails`:
     point the bogus binary at `node_modules/.bin/tsc` (or keep TLS path for the legacy branch)
     and assert the new message (`TypeScript native language server (tsc)`,
     `npm install --save-dev typescript@^7`). Decide per step 2's error surfacing: the
     initialization-failure message comes from `server_metadata_for_language` — update the
     native spec's `installation_instructions` accordingly, keep the TLS spec's text for the
     legacy test (add a second test for the native message rather than replacing).
   - `crates/mcp-servers/src/docs/lsp_registry.md` item 1: `tsc --lsp` first, TLS as fallback.

10. **Docs build + verification.**
    - `cargo doc` influenced files: `language_catalog.md`, `lsp_registry.md` edits must keep
      intra-doc links valid.
    - Run: `just fmt`, targeted `cargo test -p aether-lspd` (incl. new unit tests),
      `cargo build -p aether-lspd` then the ignored TS suites
      (`cargo test -p mcp-servers -- --ignored lsp_ts_diagnostics` etc. — they `npm install`
      pinned tooling locally), `just lint`, `just check`.

## Testing Plan

### Unit tests (all in-crate, no I/O, fast)

- `language_catalog.rs`: native config is `tsc --lsp --stdio`; TS family (all four) resolves
  to `TypeScriptNative`; native vs legacy socket identities differ; JS/JSX/TS/TSX still share
  one socket with each other; install text mentions `typescript@^7`.
- `process_transport.rs`: `base_client_capabilities()` serializes
  `textDocument.diagnostic.relatedDocumentSupport=true` and
  `workspace.diagnostic.refreshSupport=true`; `handle_register_capability`-adjacent: new
  `workspace/diagnostic/refresh` arm replies `{"result":null}` with the incoming id (test at
  message-handling level if accessible, else via fake-server e2e).
- `document_lifecycle.rs`: `open_uris` returns exactly docs with `open_holders > 0`.
- `diagnostics_store.rs`: unchanged (no modifications planned); existing settle tests guard
  the push path.
- `workspace_session.rs`: `convert_pull_report` — full with items, full empty, unchanged →
  skip-publish signal, related-docs split (full kept, unchanged dropped), partial result.

### Integration tests (fake server, hermetic)

- Extend `fake_lsp_server.py` as in step 7. New cases:
  - pull returns error diagnostic for text containing "error", empty after fix (mirrors push).
  - `--fail-on textDocument/diagnostic` → daemon returns push diagnostics (fallback path,
    pull marked Unsupported, no user-visible error).
  - initialize without `diagnosticProvider` (flag `--no-diagnostic-provider`) → daemon never
    sends pull (assert via fake's request log or behavior).
- Daemon e2e (`aether-lspd/tests/e2e/`): file-scoped `get_diagnostics` reflects pull
  immediately after `didOpen` even if push is delayed (fake flag `--delay-push-ms`); related
  documents populate the store.

### E2E tests (real `tsc` 7, `#[ignore]`d, need `npm` + built `aether-lspd`)

- Existing `lsp_ts_diagnostics_e2e.rs` (3 tests) + `lsp_ts_operations_e2e.rs` (6 tests) run
  green against `typescript@7.0.2` with zero logic changes — the strongest signal that native
  is a drop-in default.
- New: single-call freshness test asserting TS2322 text (`Type 'string' is not assignable to
  type 'number'`) appears without polling; fallback test (broken `tsc` override → TLS serves).
- Keep one TLS-based e2e alive (via `new_with_legacy_server`) so the fallback path is not
  dead code.

### Edge cases to verify

- `tsc` missing entirely → `LspSpawnFailed` → TLS fallback → if TLS also missing, error
  message contains the **native** install guidance (primary) — confirm which message surfaces
  and assert it in the contract test.
- `tsc` 6.x (no `--lsp`): process exits at startup → fallback; document the behavior in the
  plan's follow-ups if flaky in CI (prefer requiring TS ≥ 7 in the fallback test image).
- Empty pull (`items: []`) after a fix must clear previously pushed errors (publish-empty
  overwrites — assert in e2e: error → fix → `has_no_errors` without polling).
- `unchanged` report must never wipe the cache.
- Concurrent `get_diagnostics` for two files in one session: pull tri-state is session-wide
  (not per-uri) — concurrent first-pulls may double-send; harmless (idempotent reads).
- Non-TS languages never send pull (guard the pull attempt on `server_kind ==
  TypeScriptNative` OR the tri-state; cheapest: only attempt pull when session's server kind
  is native — plumb the kind into `WorkspaceSession::spawn` as a `bool supports_pull_hint`).
  Recommended: plumb the boolean (one extra `spawn` arg) AND keep the tri-state for runtime
  fallback. This avoids a wasted `textDocument/diagnostic` probe on every rust-analyzer /
  pyright / gopls / clangd session.

## Files to Modify/Create

| File | Change | Add/Modify/Remove |
|---|---|---|
| `crates/aether-lspd/src/language_catalog.rs` | Add `ServerKind::TypeScriptNative` (`tsc`, env `TYPESCRIPT_NATIVE`, args `--lsp --stdio`, display + install text); repoint 4 TS-family `LanguageSpec`s; update in-file tests | Modify |
| `crates/aether-lspd/src/docs/language_catalog.md` | Server table + pooling paragraph: `tsc` default, TLS fallback | Modify |
| `crates/aether-lspd/src/workspace_registry.rs` | Spawn-time fallback native→TLS in `get_or_spawn` + helper; keep timeout/wedge logic | Modify |
| `crates/aether-lspd/src/process_transport.rs` | Advertise `textDocument.diagnostic` + `workspace.diagnostic` caps (factor `base_client_capabilities()`); handle `workspace/diagnostic/refresh` → OK + new `TransportEvent::DiagnosticRefreshRequested` | Modify |
| `crates/aether-lspd/src/workspace_session.rs` | Pull tri-state + `pull_document_diagnostics`; wire into `get_diagnostics` file path and `refresh_uri`; handle refresh-requested event; pure `convert_pull_report` + related handling | Modify |
| `crates/aether-lspd/src/document_lifecycle.rs` | Add `open_uris()` + unit test | Modify |
| `crates/aether-lspd/src/diagnostics_store.rs` | No change (reused as-is) | — |
| `crates/aether-lspd/src/protocol.rs`, `client.rs`, `client_connection.rs` | No change (pull is session-internal) | — |
| `crates/aether-lspd/tests/common/fake_lsp_server.py` | `diagnosticProvider` capability + `textDocument/diagnostic` handler (+ `--fail-on`-compatible) | Modify |
| `crates/aether-lspd/tests/e2e/e2e_pull_diagnostics.rs` (new, wire into `e2e/main.rs`) | Pull + fallback + unchanged hermetic tests | Add |
| `crates/aether-lspd/src/testing.rs` | `TYPESCRIPT_PACKAGE` → `typescript@7.0.2`; default `NodeProject` installs TS7 only; add legacy-TLS constructor for fallback tests | Modify |
| `crates/mcp-servers/tests/integration/lsp_ts_diagnostics_e2e.rs` | Doc-comment server name; add single-call pull freshness test | Modify |
| `crates/mcp-servers/tests/integration/lsp_ts_operations_e2e.rs` | Doc-comment server name only | Modify |
| `crates/mcp-servers/tests/integration/lsp_check_errors_contract.rs` | Native install-guidance test (+ keep/adjust legacy one); bogus-binary path | Modify |
| `crates/mcp-servers/src/docs/lsp_registry.md` | Architecture item 1: `tsc --lsp`, TLS fallback | Modify |
| `.contextbridge/Dockerfile` (+ possibly `.config/nextest.toml`) | TS 7.0.2; drop or demote `TYPESCRIPT_LANGUAGE_SERVER_VERSION` | Modify |
| `docs/aether/plans/issue-494-plan.md` | This plan | Add |

## Additional Notes

- **Documentation updates needed**: `language_catalog.md`, `lsp_registry.md` (above);
  `protocol.md`/`client.md` need no changes (no protocol change). Consider a short note in
  the daemon README that TS uses pull-merged diagnostics.
- **No new dependencies.** Everything needed is in `lsp-types 0.97`. Do not add `anyhow` /
  `color-eyre` (repo error-handling rule); use existing `TransportError` / `LspErrorResponse`.
- **Style reminders for the implementer**: `pub` items at file top, private helpers at
  bottom; almost no `//` comments (explain "why", never "what"); `?` + combinators over
  nested `match`; tests assert via public API with fakes (`fake_lsp_server.py`), no timeouts
  in tests, no mocks.
- **Possible follow-ups (not in scope)**: `workspace/diagnostic` (server says
  `workspaceDiagnostics:false`, skip); result-id/`previous_result_id` round-tripping if pull
  latency matters; `diagnosticProvider.identifier` honoring if multi-server TS setups appear;
  removing the now-legacy TLS path once TS7 adoption is universal; bumping the repo's own
  dev-dependency pins alongside.
- **Open question for the task owner (answer before coding)**: should the fallback to
  `typescript-language-server` be silent (warn log only, as planned) or surfaced in the
  `lsp_check_errors` output when native is unavailable? Plan assumes silent-with-warn; if
  surfaced, the implementer must thread a warning through `PublishDiagnosticsParams` —
  say so and this plan will be amended.
