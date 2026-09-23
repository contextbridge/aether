# Issue #514 Plan — Dynamic reasoning effort without cache rewrites (OpenAI + Codex)

## Overview

### Problem statement

On the OpenAI Responses API, `reasoning.effort` is part of the rendered prefix that prompt caching hashes. Changing it at the request level rewrites model-side reasoning instructions in the hidden system content, so any mid-session effort change today discards the cached prefix and pays full input-token cost again. The [prompt-caching guide](https://developers.openai.com/api/docs/guides/prompt-caching#how-to-optimize-prompt-caching) calls this out explicitly: keep request-level `reasoning.effort` unchanged and change effort via a `configuration_update` input item instead, preserving the earlier prefix for cache reuse.

The supported mechanism is a `configuration_update` input item placed before the next user message:

```json
{"type": "configuration_update", "reasoning": {"effort": "high"}}
```

The latest update controls the effective effort for that and subsequent responses until overridden. The response's `reasoning.effort` still reports the request-level value. Support is documented for the GPT-6 family in standard, single-agent mode, effort-only. Sources: [Reasoning guide — change reasoning mid-conversation](https://developers.openai.com/api/docs/guides/reasoning?api-mode=responses), [Prompt caching — how to optimize](https://developers.openai.com/api/docs/guides/prompt-caching#how-to-optimize-prompt-caching).

Today, Aether does not use this mechanism for the OpenAI Responses and Codex providers (which share one builder in `crates/llm/src/providers/openai_responses/mappers.rs`):

- `build_wire_request` patches `reasoning.effort` at the request level on every call, so any effort change rewrites the cached prefix.
- `Context` holds a single `reasoning_effort` with no notion of a pinned request-level (base) value versus the current effective value, so the mapper cannot keep the prefix stable while changing effort.
- `AgentCommand::SetReasoningEffort` just mutates `Context.reasoning_effort`, which flows straight into the request-level patch.

The issue asks us to support switching reasoning levels mid-session without thrashing the (expensive) prompt cache, for both `openai` and `codex` providers.

### Success criteria / acceptance conditions

1. Changing reasoning effort mid-conversation on a supporting model sends a `configuration_update` input item and keeps request-level `reasoning.effort` and `prompt_cache_key` stable (cache hit preserved); the effective effort applies to the next and subsequent responses until overridden.
2. Behavior is capability-gated: models that do not support `configuration_update` (non-GPT-6) keep today's behavior exactly (request-level effort patch).
3. The API's `configuration_update` constraints are all honored: item placed before the next user message; never two adjacent updates (coalesce rapid successive changes); incompatible with automatic compaction/truncation and `/responses/compact`; after explicit compaction, a fresh update is emitted when effective effort differs from base; updates are replayed in position on manual history round-trips (`filter_encrypted_reasoning`, `with_compacted_summary`, retry clones in `agent.rs`).
4. Codex provider gets the same mechanism where its backend honors it, otherwise explicitly gated off with tests proving which path it takes (Codex defaults `always_include_reasoning=true` / `default_effort=Medium` and `disabled` vs `none` need their own base-pinning tests).
5. `just fmt`, `just lint`, `just check`, `just test` pass; new wire tests assert the exact JSON bodies.

## Technical Approach

### High-level architectural decisions

1. **Keep the shared builder.** OpenAI Responses (`ResponsesRequestPolicy::openai()`) and Codex (`::codex()`) share `build_typed_request`/`build_wire_request` in `openai_responses/mappers.rs`. Add the new behavior there behind a catalog capability check, not in per-provider forks. Bedrock Mantle (`::mantle()`) must be unaffected — gate strictly on model capability.
2. **Separate "base (request-level) effort" from "effective effort" in `Context`.** The request-level `reasoning.effort` is pinned once a conversation starts (it is part of the cached prefix); per-turn changes travel as `configuration_update` items in `input`. Track both, plus enough state to coalesce rapid changes and to avoid emitting two adjacent updates.
3. **Synthesize the update item at request-build time; do not store it as chat history.** The mapper derives whether an update is needed from `Context` state (`effective != base`) and injects exactly one item immediately before the last user message (or at a single defined fallback position when there is no new user message). This keeps `ChatMessage` unchanged, makes explicit compaction automatically correct (the update is re-synthesized after history is dropped), and avoids persisting API wire details in the conversation model.
4. **Wire-patch what `async-openai` cannot express (same pattern as today), but prefer a dependency upgrade if it lands the missing type.** Today's code already serializes `CreateResponse` then patches `body["reasoning"]["effort"]` because the pinned `ReasoningEffort` enum cannot express every wire value. The pinned SDK has no `configuration_update` input-item type, so patch the serialized `input` array in `build_wire_request`, exactly like the existing effort patch. First spike: check for a newer `async-openai` release with this type; if it exists as a drop-in upgrade, use it instead of patching. Decision point documented in Step 1.
5. **Keep `derive_prompt_cache_key` semantics: key = cacheable prefix.** Reasoning effort is already excluded from the key; `configuration_update` items must not enter the hash either, so the key stays stable across effort changes by construction. Add regression tests proving it.
6. **No backwards-compat shims.** Per repo convention, gate by capability and change behavior outright for supporting models; no feature flags or fallback env vars.

### Design patterns to employ

- **Catalog-driven capability** (`LlmModel` / `ModelSpec`): add a `supports_dynamic_reasoning()` predicate (GPT-6 family) with catalog tests; the mapper consults it at build time.
- **Policy object** (`ResponsesRequestPolicy`): no new per-provider flags beyond what the capability check needs; Codex-specific base-pinning edge cases are covered by tests, not forks.
- **Wire-patch in `build_wire_request`** for the SDK gap (established pattern, documented with a comment + unit test on the JSON shape).
- **Test-builder + Fake-based integration tests**; public-API-only assertions on wire JSON (capture fixtures) and on `prompt_cache_key` stability.

### Key technical considerations and trade-offs

- **Capability is model-dependent, not provider-dependent.** Dynamic reasoning is documented for the GPT-6 family (standard, single-agent mode, effort-only). Codex subscription models (gpt-5.5/5.6/6-astra per `llm-codegen` `CODEX_SUBSCRIPTION_MODELS`) may support a subset — verify against the Codex backend and gate; do not assume parity.
- **`configuration_update` constraints** (all must be honored):
  - Place the item before the next user message in `input`; leave request-level `reasoning.effort` unchanged.
  - Effective until overridden; the response's `reasoning.effort` still reports the request-level value.
  - Never emit two adjacent updates (the API rejects them); coalesce rapid successive changes into one.
  - Incompatible with automatic compaction/truncation and `/responses/compact`; after explicit compaction (which drops history including updates), emit a fresh update when effective differs from base — synthesis-at-build gives this for free, but it needs a test.
  - Replay/preserve updates in original positions when manually round-tripping history (`filter_encrypted_reasoning`, `with_compacted_summary`, retry clones in `agent.rs`) — synthesis-at-build gives this for free as long as the base/effective tracking fields survive those projections (they do via `..self.clone()`; add regression tests).
- **Base-pinning rule.** The first request in a conversation sends today's request-level `reasoning.effort` (the base). Later requests keep `body["reasoning"]["effort"]` at base and carry changes as input items. The base is pinned per `Context` lifetime; `clear_conversation`/`replace_conversation` semantics for the base (keep vs. re-pin) must be defined and tested — recommended: `clear_conversation` re-pins on the next request, `replace_conversation` keeps the existing base.
- **Codex interaction.** Codex defaults to `always_include_reasoning=true` with `default_effort=Medium` and serializes disabled effort as `"disabled"` (OpenAI uses `"none"`). The base-pinning logic must account for the provider default: an explicit effort equal to the provider default still counts as the base, not an update. Test the `Default` + Codex-Medium combination explicitly.
- **Chat-completions path** (`openai/provider.rs`, `openai_compatible`) does not get this feature — Responses-only.

## Implementation Steps

### Step 1 — Capability inventory + SDK spike (no behavior change)

1. Confirm the pinned `async-openai` has no `configuration_update` input-item type. Check crates.io for a newer release with the type; if a drop-in minor upgrade provides it, prefer the upgrade over JSON patching (update the workspace pin + `Cargo.lock`, re-run `just check`).
2. Add the model capability source: extend `llm-codegen` (`crates/llm-codegen/src/lib.rs`) + `ModelSpec` (`crates/llm/src/catalog/model_spec.rs`) with a `supports_dynamic_reasoning()` predicate (GPT-6 family), plus catalog tests.
3. Verify Codex backend support for `configuration_update` (docs + a live/stubbed capture test); record the decision (supported vs. gated off) in the plan's Additional Notes before implementing Steps 3–4 for Codex.

### Step 2 — `Context` state: base vs. effective effort

File: `crates/llm/src/context.rs`.

1. Add `#[serde(skip)]` runtime fields preserving today's `reasoning_effort` getter/setter as the effective effort, plus:
   - `base_reasoning_effort: Option<ReasoningEffort>` — pinned request-level value once the first request is built (`None` = not yet pinned).
   - Accessors to read the base, pin it, and compute `needs_configuration_update()` (`base.is_some() && effective != base`).
2. Define projection semantics with tests: `filter_encrypted_reasoning` / `with_compacted_summary` / struct-update clones preserve both fields; `clear_conversation` resets the base to `None` (re-pin on next build); `replace_conversation` keeps the base.
3. Keep `set_reasoning_effort` semantics (sets the effective effort). Document that the mapper, not the caller, decides the wire shape.

Pseudo-shape:

```rust
pub struct Context {
    messages: Vec<ChatMessage>,
    tools: Vec<ToolDefinition>,
    #[serde(skip)] reasoning_effort: ReasoningEffort,      // effective effort
    #[serde(skip)] base_reasoning_effort: Option<ReasoningEffort>, // pinned request-level value
    ...
}
```

### Step 3 — Mapper: `configuration_update` (dynamic reasoning without cache thrash)

File: `crates/llm/src/providers/openai_responses/mappers.rs` (+ `build_wire_request` patch).

1. Pinning rule: on the first `build_wire_request` for a `Context` (base is `None`), pin the base to the resolved effort (`policy.effort(context)`) and send today's request-level `reasoning.effort` with no update item. On subsequent requests where the effective effort differs from base **and** the model supports dynamic reasoning, keep `body["reasoning"]["effort"]` at base and insert `{"type":"configuration_update","reasoning":{"effort":<new>}}` into `input` immediately before the last user message. When there is no user message (e.g. a trailing tool-output flush), append the update at the end of `input` — define and test this single fallback rule.
2. Coalescing/adjacency: synthesis-at-build emits at most one update per request, so rapid successive changes naturally coalesce into a single item reflecting the latest effective effort. After explicit compaction, the update is re-synthesized whenever effective still differs from base.
3. Unsupported models: keep today's behavior (patch request-level effort; no `configuration_update`).
4. Since `async-openai` lacks the type, patch the serialized body in `build_wire_request` (same function as the existing effort patch), with a comment citing the docs + constraint list.
5. Unit tests (wire JSON): effort change on a supported model yields base effort in `reasoning.effort` plus exactly one `configuration_update` before the user message; a second build without further change yields the same single item (no duplication growth); two rapid changes yield one item with the latest effort; unsupported model yields a request-level patch with no update item; `store:false` / `prompt_cache_key` unchanged.

### Step 4 — Agent loop + cache-key regression tests

Files: `crates/aether-core/src/core/agent.rs`, `crates/aether-core/src/core/prompt_cache_key.rs`, `crates/aether-core/tests/agent/prompt_cache_tests.rs`.

1. `SetReasoningEffort`: keep mutating `Context.reasoning_effort` (the effective effort); no agent-side branching — Step 3 emits it as `configuration_update` on supporting models. Ensure retry clones (`agent.rs` retry path) and `refresh_prompt_cache_key` do not reset the base/effective tracking.
2. `derive_prompt_cache_key`: no hash-input change (effort already excluded; synthesized `configuration_update` items must not enter the hash). Add regression tests: key stable across effort changes; session-affinity behavior unchanged.

### Step 5 — Codex + OpenAI provider wiring + docs

Files: `crates/llm/src/providers/codex/provider.rs`, `crates/llm/src/providers/openai/responses_provider.rs`, `crates/llm/src/docs/*`.

1. No per-provider mapper forks: both inherit Step 3 via the shared builder. Add provider-level capture tests asserting: OpenAI GPT-6 model with an effort change takes the `configuration_update` path (base pinned, update before the user message); non-GPT-6 model keeps the request-level patch; Codex default model takes whichever path Step 1 verified, including the `Default`-effort-with-Medium-default base-pinning case and the `disabled` vs `none` serialization.
2. Update provider docs (`streaming_model_provider.md` / `providers.md`) with the capability note (which models support `configuration_update`) and the "base vs. effective effort" mental model, plus the constraint list (no adjacent updates, no auto-compaction, re-emit after explicit compaction).

## Testing Plan

- **Unit (mapper, `openai_responses/mappers.rs` tests):**
  - Supported model + effort change: `reasoning.effort` equals base, exactly one `configuration_update` before the user message, `prompt_cache_key`/`store` unchanged.
  - Second build without further change: still exactly one update (no duplicate growth).
  - Two rapid changes: single coalesced update with the latest effort.
  - Unsupported model: request-level patch only, no update item.
  - No-user-message fallback: update appended at end of `input`.
  - First-request pinning: base pinned, no update item on the first build.
  - Codex `Default` effort resolves to the Medium default as base; `Disabled` serializes `"disabled"` on Codex vs `"none"` on OpenAI.
- **Unit (`context.rs`, `catalog/model_spec.rs`):** base pinning, `needs_configuration_update`, preservation through `filter_encrypted_reasoning` / `with_compacted_summary`, `clear_conversation` re-pin vs. `replace_conversation` keep, capability predicate per model.
- **Provider capture tests** (`codex/provider.rs`, `openai/responses_provider.rs` inline tests + `tests/providers/openai/capture_fixtures.rs`): exact wire bodies for reasoning changes on supported vs. unsupported models, for both providers.
- **Agent integration (`crates/aether-core/tests/agent/prompt_cache_tests.rs`):** key stable across `SetReasoningEffort`; session-affinity behavior unchanged.
- **Edge cases:** adjacent-update rejection avoidance; explicit-compaction re-emit; retry-path clone preservation; Bedrock Mantle parity check (no behavior change).
- **Gates:** `just fmt && just lint && just check && just test` (targeted `cargo test -p llm` / `-p aether-core` first, then full).

## Files to Modify/Create

| File | Change | Add/Modify/Remove |
|---|---|---|
| `crates/llm/src/providers/openai_responses/mappers.rs` | Pin base effort; insert `configuration_update` via wire patch; capability gating | Modify |
| `crates/llm/src/context.rs` | `base_reasoning_effort` tracking, accessors, projection semantics | Modify |
| `crates/llm/src/catalog/model_spec.rs` + `crates/llm-codegen/src/lib.rs` | `supports_dynamic_reasoning()` predicate + codegen/tests | Modify |
| `crates/llm/src/providers/codex/provider.rs` | Capture tests for the Codex path (impl inherited) | Modify |
| `crates/llm/src/providers/openai/responses_provider.rs` | Capture tests for the OpenAI path (impl inherited) | Modify |
| `crates/aether-core/src/core/agent.rs` | Preserve effort tracking across retries; no new branching | Modify |
| `crates/aether-core/src/core/prompt_cache_key.rs` | No hash change; regression tests for stability | Modify |
| `crates/aether-core/tests/agent/prompt_cache_tests.rs` | Key-stability tests across effort changes | Modify |
| `crates/llm/src/docs/providers.md` / `streaming_model_provider.md` | Capability note + base-vs-effective docs | Modify |
| `Cargo.toml` / `Cargo.lock` | `async-openai` upgrade **only if** spike shows the new type (else skip) | Modify (conditional) |

Out of scope (follow-ups): `reasoning.mode:pro` / `reasoning.context:all_turns`, chat-completions-path support. Tool-definition caching behavior is unchanged: Aether keeps its stable tool lists and existing tool deferral, so no `allowed_tools`, `defer_loading`, `tool_search`, or `additional_tools` work is included.

## Additional Notes

- **Documentation updates needed:** provider docs capability note (which models support `configuration_update`); agent-facing notes that mid-session reasoning changes no longer invalidate the prompt cache on supporting models.
- **Key open question for implementer (Step 1):** Codex backend parity. The Codex provider reuses the Responses builder but talks to its own backend with its own defaults (`always_include_reasoning=true`, `default_effort=Medium`, `disabled`→`"disabled"`). Verify the Codex backend honors `configuration_update` before enabling it there; the base-pinning interaction with the Medium default needs its own test.
- **SDK upgrade note:** if `async-openai` gains a `ConfigurationUpdate` input-item type in a compatible release, adopt it and delete the wire patch; otherwise keep the patch pattern with a `TODO(async-openai: ...)` pointer. Do not hand-roll a fork.
