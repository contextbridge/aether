# Issue #514 Plan — Dynamic reasoning & cache-preserving tool switching (OpenAI + Codex)

## Overview

### Problem statement

OpenAI's Responses API now supports changing reasoning effort and swapping tools in/out **without thrashing the prompt cache**:

- **Dynamic reasoning:** a `configuration_update` input item (`{"type":"configuration_update","reasoning":{"effort":"high"}}`) placed before the next user message changes the effective reasoning effort for that and subsequent responses, while the request-level `reasoning.effort` stays unchanged — preserving the original prompt prefix for prompt caching. (GPT-6 family, standard single-agent mode, effort-only.)
- **Cache-preserving tool switching:** three complementary mechanisms:
  1. `tool_choice: {"type":"allowed_tools","mode":"auto"|"required","tools":[...]}` — restrict the callable subset **without modifying the `tools` list**, so the cached prefix is untouched.
  2. `defer_loading: true` on function tools (+ a `{"type":"tool_search"}` tool, optionally namespaces) — deferred tools load on demand via hosted `tool_search_call`/`tool_search_output` items and are **injected at the end of the context window**, preserving the cache.
  3. `additional_tools` developer-role input items — make tools available at a specific point in the input history (advanced/manual round-trip workflows).

Today, Aether does none of this for the OpenAI Responses and Codex providers (which share one builder in `crates/llm/src/providers/openai_responses/mappers.rs`):

- `build_wire_request` patches `reasoning.effort` at the request level on every call, so **any effort change rewrites the cached prefix**.
- `map_tools` hard-codes `defer_loading: None`, never emits a `tool_search` tool or `tool_choice`, and resends the whole tool list wholesale.
- `derive_prompt_cache_key` (`crates/aether-core/src/core/prompt_cache_key.rs`) hashes model + system prompt + full tool name/description/parameters, so **any tool add/remove changes the routing key** even when the API could have preserved the cache.
- `AgentCommand::SetReasoningEffort` just mutates `Context.reasoning_effort`; `UpdateTools` replaces `Context.tools` — neither has a cache-preserving path.

The issue asks us to support these API features so swapping reasoning levels / tools doesn't thrash the (expensive) cache, for both `openai` and `codex` providers.

Sources: [Reasoning guide — dynamic effort via `configuration_update`](https://developers.openai.com/api/docs/guides/reasoning?api-mode=responses), [Function calling — `allowed_tools`](https://developers.openai.com/api/docs/guides/function-calling?api-mode=responses), [Tool search — `defer_loading`/`tool_search`/caching](https://developers.openai.com/api/docs/guides/tools-tool-search).

### Success criteria / acceptance conditions

1. Changing reasoning effort mid-conversation on a supporting OpenAI model sends a `configuration_update` input item and **keeps request-level `reasoning.effort` and `prompt_cache_key` stable** (cache hit preserved); the effective effort applies to the next and subsequent responses until overridden.
2. Restricting the callable tool subset (tool switching off) sends `tool_choice.allowed_tools` **without changing the `tools` array or `prompt_cache_key`**.
3. Deferred tools are sent with `defer_loading: true` plus a `tool_search` tool; hosted search outputs round-trip through history; newly loaded tools append at end-of-context (no prefix rewrite).
4. Behavior is **capability-gated**: models that don't support `configuration_update` (non-GPT-6) or `tool_search` (pre-gpt-5.4) keep today's behavior exactly (request-level effort patch + full tool list).
5. Codex provider gets the same mechanisms where its backend supports them, otherwise explicitly gated off with tests proving which path it takes.
6. `just fmt`, `just lint`, `just check`, `just test` pass; new capture/wire tests assert exact JSON bodies.

## Technical Approach

### High-level architectural decisions

1. **Keep the shared builder.** OpenAI Responses (`ResponsesRequestPolicy::openai()`) and Codex (`::codex()`) share `build_typed_request`/`build_wire_request` in `openai_responses/mappers.rs`. Add the new behavior there behind policy flags + catalog capability checks, not in per-provider forks. Bedrock Mantle (`::mantle()`) must be unaffected — gate strictly on provider/model capability.
2. **Wire-patch what `async-openai 0.41.3` can't express (same pattern as today), but prefer a dependency upgrade if it lands the missing types.** Today's code already serializes `CreateResponse` then patches `body["reasoning"]["effort"]` because the pinned `ReasoningEffort` enum lacks `max`/`disabled`/`none`/`xhigh`. The pinned SDK **does** have `FunctionTool.defer_loading`, `Tool::ToolSearch`, `ToolChoiceParam::AllowedTools`, `Item::ToolSearchCall/ToolSearchOutput`, but has **no** `configuration_update` or `additional_tools` input-item types and `Reasoning` has only `effort`+`summary` (no `mode`/`context`). So:
   - `defer_loading`, `tool_search`, `allowed_tools` → use the typed SDK structs.
   - `configuration_update` (+ `additional_tools` if pursued) → patch onto the serialized `input` array in `build_wire_request`, exactly like the existing effort patch. First spike: check for a newer `async-openai` release with these types; if it exists and is a drop-in upgrade, use it instead of patching. Decision point documented in Step 1.
3. **Separate "declared tools" from "callable subset" in `Context`.** Cache preservation hinges on this: the `tools` array (the cached prefix) stays stable while `tool_choice.allowed_tools` narrows what's callable. Add first-class `Context` state for the allowed subset and deferred flags rather than overloading `set_tools`.
4. **Separate "base (request-level) effort" from "effective effort" in `Context`.** The request-level `reasoning.effort` must stay pinned once a conversation starts (it's part of the cached prefix); per-turn changes travel as `configuration_update` items in `input`. Track both plus the last-emitted update to enforce the API's "no two adjacent `configuration_update` items" rule.
5. **Keep `derive_prompt_cache_key` semantics: key = cacheable prefix.** The key must stay stable across effort changes and `allowed_tools` narrowing (neither alters the prefix), and change only when the declared prefix genuinely changes (system prompt, declared tool set, model). Deferred-tool discovery appends at end-of-context by API design, so the prefix key stays valid.
6. **No backwards-compat shims.** Per repo convention, gate by capability and change behavior outright for supporting models; no feature flags or fallback env vars.

### Design patterns to employ

- **Policy object** (`ResponsesRequestPolicy`): add `supports_dynamic_reasoning: bool`-style capability flags or resolve from the `LlmModel` catalog at build time (preferred — model-driven, not provider-hardcoded).
- **Wire-patch in `build_wire_request`** for SDK gaps (established pattern, documented with a comment + unit test on the JSON shape).
- **Test-builder + Fake-based integration tests**; public-API-only assertions on wire JSON (capture fixtures) and on `prompt_cache_key` stability.

### Key technical considerations and trade-offs

- **Capability matrix is model-dependent, not provider-dependent.** Dynamic reasoning (`configuration_update`) is documented for the **GPT-6 family** (standard, single-agent mode, effort-only). Hosted `tool_search` requires **gpt-5.4+**. `allowed_tools` is general Responses API. Codex subscription models (gpt-5.5/5.6/6-astra per `llm-codegen` `CODEX_SUBSCRIPTION_MODELS`) may support a subset — verify against the Codex backend and gate; do not assume parity.
- **`configuration_update` constraints** (must all be honored):
  - Place the item **before the next user message** in `input`; leave request-level `reasoning.effort` unchanged.
  - Effective until overridden; response's `reasoning.effort` still reports the request-level value.
  - Never emit two adjacent updates (API rejects); coalesce rapid successive changes into one.
  - Incompatible with automatic compaction/truncation and `/responses/compact`; after explicit compaction (which drops history including updates), emit a fresh update if effective ≠ base.
  - Replay/preserve updates in original positions when manually round-tripping history (`filter_encrypted_reasoning`, `with_compacted_summary`, retry clones in `agent.rs`).
- **Tool-search constraints:** only deferred tools load on demand; model still sees deferred function name+description (schema deferred) unless grouped in namespaces/MCP (sees only namespace name+description) — namespace support is a follow-up, not required here. Changing the loaded tool set breaks the cache from that point forward (document; don't try to hide it). Client-executed tool search (`execution: "client"`) is out of scope — hosted only.
- **`allowed_tools` vs `defer_loading`:** use `allowed_tools` for *switching a subset on/off within an already-declared set* (zero prefix churn); use `defer_loading`+`tool_search` for *large/rarely-used surfaces* (pay a discovery step, save input tokens). They compose.
- **`strict` interaction:** OpenAI policy sends `strict: Some(false)`; Codex sends `None` (API defaults true). Deferred tools inherit the same per-policy `strict` — no change.
- **Prompt-cache routing key:** `derive_prompt_cache_key` hashes tool name/description/parameters. With `allowed_tools`, the declared set is unchanged → key stable (this falls out naturally; add a regression test). With deferred tools, the declared set *includes* the deferred definitions + `tool_search` marker, so derive the key over the declared set consistently.
- **Streaming new items:** hosted search yields `tool_search_call` / `tool_search_output` output items. `process_response_stream` currently ignores unknown events (`Ignored`). Decide: surface as new `LlmResponse` variants vs. record into history transparently. Minimal viable: decode them into history items (so `all_turns`/continuity works) and emit a lightweight `LlmResponse` event the agent can observe; do NOT route them through `ToolCallCollector` as function calls.
- **Chat-completions path** (`openai/provider.rs`, `openai_compatible`) does not get these features — Responses-only.

## Implementation Steps

### Step 1 — Capability inventory + SDK spike (no behavior change)

1. Confirm in `async-openai 0.41.3` (workspace pin `^0.41.0`): `FunctionTool.defer_loading`, `Tool::ToolSearch(ToolSearchToolParam)`, `ToolChoiceParam::AllowedTools(ToolChoiceAllowed{mode, tools})`, `Item::ToolSearchCall/ToolSearchOutput` exist; `configuration_update` / `additional_tools` input items and `Reasoning{mode,context}` do **not**. Check crates.io for a newer `async-openai` with these types; if a drop-in minor upgrade provides them, prefer the upgrade over JSON patching (update `Cargo.toml` workspace pin + `Cargo.lock`, re-run `just check`).
2. Determine the model capability source: extend `llm-codegen` (`crates/llm-codegen/src/lib.rs`) + `ModelSpec` (`crates/llm/src/catalog/model_spec.rs`) with two derived predicates, e.g. `supports_dynamic_reasoning()` (GPT-6 family) and `supports_tool_search()` (gpt-5.4+), plus catalog tests. Alternatively resolve by model-name prefix in the mapper as a stopgap — prefer catalog-driven with a mapper fallback documented in code.
3. Verify Codex backend support for each mechanism (docs + a live/stubbed capture test); record the decision (full support vs. `allowed_tools`-only vs. none) in the plan's Additional Notes before implementing Steps 3–5 for Codex.

### Step 2 — `Context` state: effective effort + callable subset + deferred flags

File: `crates/llm/src/context.rs` (+ `crates/llm/src/tools.rs`).

1. Add `ToolDefinition.defer_loading: bool` (default `false`; include in `ToolDefinition::new` as a builder-style setter, e.g. `.defer_loading(true)`, to avoid breaking existing constructors).
2. Add to `Context` (all `#[serde(skip)]` runtime metadata, preserved by `filter_encrypted_reasoning`/`with_compacted_summary` clones):
   - `allowed_tools: Option<Vec<String>>` — names subset; `None` = all declared tools callable (no `tool_choice` emitted).
   - `base_reasoning_effort: Option<ReasoningEffort>` (pinned request-level value once set) + `pending_effort_update: Option<ReasoningEffort>` or `last_emitted_effort` tracking for coalescing/adjacency.
   - Getters/setters: `set_allowed_tools(Option<Vec<String>>)`, `allowed_tools()`, `set_deferred_tools(...)` (or per-tool flag is enough), effort helpers.
3. Keep `set_tools` semantics (declares the prefix set). Document that narrowing callability must use `allowed_tools`, not `set_tools`.

Pseudo-shape:

```rust
pub struct Context {
    messages: Vec<ChatMessage>,
    tools: Vec<ToolDefinition>,          // declared prefix set (stable for cache)
    #[serde(skip)] reasoning_effort: ReasoningEffort, // request-level (base) once pinned
    #[serde(skip)] pending_reasoning_update: Option<ReasoningEffort>, // effective effort ≠ base
    #[serde(skip)] allowed_tools: Option<Vec<String>>, // callable subset; None = all
    ...
}
```

### Step 3 — Mapper: `allowed_tools` tool_choice (cache-preserving subset switching)

File: `crates/llm/src/providers/openai_responses/mappers.rs`.

1. In `build_typed_request`, after building `tools`, set `CreateResponse.tool_choice` from `context.allowed_tools()`:
   - `None` → `None` (today's behavior).
   - `Some(names)` → `ToolChoiceParam::AllowedTools(ToolChoiceAllowed{ mode: Auto, tools: names → [{"type":"function","name":n}] })`. (Consider `Required` only if a caller asks; default `Auto`.)
   - Validate names ⊆ declared tools; unknown names → `LlmError` (fail fast, don't send a half-valid request).
2. Do NOT touch the `tools` array or `prompt_cache_key` on this path.
3. Unit tests: allowed subset emits `tool_choice.allowed_tools` + full `tools` unchanged; `None` omits `tool_choice`; unknown name errors.

### Step 4 — Mapper: `defer_loading` + `tool_search` (hosted, cache-preserving lazy tools)

File: `crates/llm/src/providers/openai_responses/mappers.rs`.

1. In `map_tools`, forward `tool.defer_loading` (`true` → `Some(true)`, else `None`) instead of hard-coded `None`; keep per-policy `strict`.
2. In `build_typed_request`, if any declared tool is deferred **and** the model supports tool search, append `Tool::ToolSearch(ToolSearchToolParam{ ..default })` (hosted/server execution) to the `tools` vec. If unsupported, send deferred tools eagerly (today's behavior) — never emit `tool_search` to an incompatible model.
3. History round-trip: ensure `map_messages` passes through `tool_search_call`/`tool_search_output` items. This requires new `ChatMessage` variants (see Step 6) mapped to `Item::ToolSearchCall/ToolSearchOutput`.
4. Unit tests: deferred tool serializes `defer_loading:true`; `tool_search` present iff (any deferred && supported); unsupported model → eager, no `tool_search` entry.

### Step 5 — Mapper: `configuration_update` (dynamic reasoning without cache thrash)

File: `crates/llm/src/providers/openai_responses/mappers.rs` (+ `build_wire_request` patch).

1. Pinning rule: the first request in a conversation sends today's request-level `reasoning.effort` (base). On subsequent requests where `context.reasoning_effort() != base` **and** model supports dynamic reasoning, keep `body["reasoning"]["effort"]` at base and insert `{"type":"configuration_update","reasoning":{"effort":<new>}}` into `input` **immediately before the next (last) user message** (or appended before a trailing tool-output flush if no new user message — define and test one rule).
2. Coalescing/adjacency: if the previous input already ends with a `configuration_update` at that position, replace/merge rather than appending a second adjacent one. After explicit compaction (history no longer contains the update), re-emit if effective ≠ base.
3. Unsupported models: keep today's behavior (patch request-level effort; no `configuration_update`).
4. Since `async-openai` lacks the type, patch the serialized body in `build_wire_request` (same function as the existing effort patch), with a comment citing the docs + constraint list.
5. Unit tests (wire JSON): effort change on supported model → base effort in `reasoning.effort` + exactly one `configuration_update` before the user message; no adjacent duplicates across two successive builds; unsupported model → request-level patch, no update item; `store:false`/`prompt_cache_key` unchanged.

### Step 6 — History + streaming: round-trip the new items

Files: `crates/llm/src/chat_message.rs`, `crates/llm/src/context.rs`, `crates/llm/src/providers/openai_responses/streaming.rs`, `crates/llm/src/llm_response.rs`.

1. Add `ChatMessage` variants (or structured payload variants) for `ConfigurationUpdate{ effort }`, `ToolSearchCall`, `ToolSearchOutput` sufficient to re-emit them at original positions via `map_messages`.
2. `filter_encrypted_reasoning` / `with_compacted_summary` / `replace_conversation` must preserve these items (they're part of the conversation state the docs require replaying in position).
3. Streaming: decode `tool_search_call`/`tool_search_output` output items (currently `Ignored`) — record into history and surface a minimal `LlmResponse` event (e.g. `LlmResponse::ToolSearch{...}` or reuse `Usage`-adjacent metadata event; decide in implementation, public API only). Add SSE fixture coverage (`tests/fixtures/openai_responses/04_tool_search.sse`).
4. Keep `ToolCallCollector` for real function calls only.

### Step 7 — Agent loop: use the cache-preserving paths

Files: `crates/aether-core/src/core/agent.rs`, `crates/aether-core/src/core/prompt_cache_key.rs`.

1. `SetReasoningEffort`: keep mutating `Context.reasoning_effort`, but rely on Step 5 to emit it as `configuration_update` on supporting models (no agent-side branching beyond capability logging). Ensure retry clones (`agent.rs:375-384`) and `refresh_prompt_cache_key` don't reset the base/effective tracking.
2. Tool updates: when an `UpdateTools` event only narrows/widens within the declared set, translate to `set_allowed_tools` (key stable); only genuine declaration changes call `set_tools` (key changes — correct, prefix really changed). Define the classification rule + tests in `prompt_cache_tests.rs`.
3. `derive_prompt_cache_key`: no hash-input change expected (effort already excluded; `allowed_tools` must NOT enter the hash). Add regression tests: key stable across effort change and across allowed-subset narrowing; key changes on real declaration change.

### Step 8 — Codex + OpenAI provider wiring + docs

Files: `crates/llm/src/providers/codex/provider.rs`, `crates/llm/src/providers/openai/responses_provider.rs`, `crates/llm/src/docs/*`.

1. No per-provider mapper forks: both inherit Steps 3–5 via policy. Add provider-level capture tests asserting: OpenAI GPT-6 → `configuration_update` path; Codex default model → whichever path Step 1 verified (likely `allowed_tools` + `tool_search` at minimum; dynamic reasoning only if the Codex backend honors it — Codex `reasoning.effort` uses `"disabled"` not `"none"`, and Codex defaults `always_include_reasoning=true`/`default_effort=Medium`, so the base-pinning interaction needs its own test).
2. Update provider docs (`streaming_model_provider.md` / `providers.md`) with the capability matrix and the "declare vs. allow" / "base vs. effective effort" mental model.

## Testing Plan

- **Unit (mapper, `openai_responses/mappers.rs` tests):**
  - `allowed_tools`: subset → `tool_choice.allowed_tools{mode:auto,tools:[{type:function,name}]}` + full `tools` array untouched; `None` → no `tool_choice`; unknown name → error.
  - `defer_loading`: deferred fn → `"defer_loading":true` on the wire; eager fns omit it; `tool_search` tool appended iff (any deferred && model supports); unsupported model → no `tool_search`, deferred sent eagerly.
  - `configuration_update`: supported model + effort change → `reasoning.effort` == base, one update item before the user message, `prompt_cache_key`/`store` unchanged; second build without further change → no duplicate; two rapid changes → single coalesced update; unsupported model → request-level patch only.
  - Adjacency/compaction: re-emit after explicit compaction when effective ≠ base.
- **Unit (`context.rs`, `tools.rs`, `catalog/model_spec.rs`):** new setters/getters, projection preservation (`filter_encrypted_reasoning`, `with_compacted_summary`), capability predicates per model.
- **Streaming (`streaming.rs` tests + new fixture `04_tool_search.sse`):** `tool_search_call`/`tool_search_output` decode, history recording, no `ToolCallCollector` pollution; usage still parsed.
- **Provider capture tests** (`codex/provider.rs`, `openai/responses_provider.rs` inline tests + `tests/providers/openai/capture_fixtures.rs`): exact wire bodies for (a) reasoning change, (b) allowed-tools narrowing, (c) deferred tools on supported vs. unsupported models, for both providers.
- **Agent integration (`crates/aether-core/tests/agent/prompt_cache_tests.rs`):** key stable across `SetReasoningEffort`; stable across allowed-subset narrowing; changes on declaration change; refreshes after `UpdateTools` that really changes declarations; session-affinity behavior unchanged.
- **Edge cases:** unknown allowed-tool name; `allowed_tools: Some([])` semantics (define: treat as `tool_choice:none` vs. error — decide in impl, test it); deferred + `strict` interplay; Codex `disabled` vs OpenAI `none`; `Default` effort with Codex `default_effort=Medium` base pinning; adjacent-update rejection avoidance; explicit-compaction re-emit; retry-path clone preservation; Bedrock Mantle parity check (no behavior change).
- **Gates:** `just fmt && just lint && just check && just test` (targeted `cargo test -p llm` / `-p aether-core` first, then full).

## Files to Modify/Create

| File | Change | Add/Modify/Remove |
|---|---|---|
| `crates/llm/src/providers/openai_responses/mappers.rs` | Emit `tool_choice.allowed_tools`, forward `defer_loading` + append `tool_search`, insert `configuration_update` via wire patch; capability gating | Modify |
| `crates/llm/src/context.rs` | `allowed_tools`, base-vs-effective effort tracking, accessors; preserve through projections | Modify |
| `crates/llm/src/tools.rs` | `ToolDefinition.defer_loading` (+ builder setter) | Modify |
| `crates/llm/src/chat_message.rs` | Variants for config-update / tool-search history items | Modify |
| `crates/llm/src/catalog/model_spec.rs` + `crates/llm-codegen/src/lib.rs` | `supports_dynamic_reasoning()` / `supports_tool_search()` predicates + codegen/tests | Modify |
| `crates/llm/src/providers/openai_responses/streaming.rs` | Decode/surface `tool_search_call`/`tool_search_output` | Modify |
| `crates/llm/src/llm_response.rs` | New event variant(s) for tool-search progress (if chosen) | Modify |
| `crates/llm/src/providers/codex/provider.rs` | Capture tests for Codex paths (impl inherited) | Modify |
| `crates/llm/src/providers/openai/responses_provider.rs` | Capture tests for OpenAI paths (impl inherited) | Modify |
| `crates/aether-core/src/core/agent.rs` | Route subset-only tool changes via `allowed_tools`; preserve effort tracking across retries | Modify |
| `crates/aether-core/src/core/prompt_cache_key.rs` | No hash change expected; regression tests for stability | Modify |
| `crates/aether-core/tests/agent/prompt_cache_tests.rs` | Key-stability tests (effort change, allowed narrowing, declaration change) | Modify |
| `crates/llm/tests/fixtures/openai_responses/04_tool_search.sse` | New hosted-search SSE fixture | Add |
| `crates/llm/src/docs/providers.md` / `streaming_model_provider.md` | Capability matrix + declare-vs-allow / base-vs-effective docs | Modify |
| `Cargo.toml` / `Cargo.lock` | `async-openai` upgrade **only if** spike shows new types (else skip) | Modify (conditional) |

Out of scope (follow-ups): `namespace`-grouped deferred tools, client-executed (`execution:client`) tool search, `additional_tools` input items, `reasoning.mode:pro` / `reasoning.context:all_turns`, chat-completions-path support.

## Additional Notes

- **Documentation updates needed:** provider docs capability matrix (which models support `configuration_update` vs `tool_search` vs `allowed_tools`); agent-facing notes that reasoning changes and subset tool switching no longer invalidate the prompt cache on supporting models.
- **Key open question for implementer (Step 1):** Codex backend parity. The Codex provider reuses the Responses builder but talks to `https://chatgpt.com/backend-api/codex` with its own defaults (`always_include_reasoning=true`, `default_effort=Medium`, `tool_strict=None`, `disabled→"disabled"`). Verify each mechanism against Codex before enabling: safest is `allowed_tools` + `tool_search` first, `configuration_update` only after proving the Codex backend honors it (its base-pinning interacts with the Medium default).
- **SDK upgrade note:** if `async-openai` gains `ConfigurationUpdate`/`AdditionalTools`/`Reasoning{mode,context}` types in a compatible release, adopt them and delete the wire patch; otherwise keep the patch pattern with a `TODO(async-openai: ...)` pointer. Do not hand-roll a fork.
- **Follow-up tasks:** namespace-grouped deferral for large tool surfaces; client-executed tool search for tenant/project-dependent tools; `reasoning.context: all_turns` + encrypted-reasoning interplay for GPT-5.6+; cost/latency evals (cache-hit rate, input-token savings) to tune which tools are deferred by default.
