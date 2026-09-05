# Issue #430 — Reusable GitHub Action for aether-agent workflow

## Overview

### Problem statement

The `aether-agent` workflow (`.github/workflows/aether-agent.yml` + 5 bash scripts in
`scripts/aether-agent/`) is hardcoded to this repo: it assumes the ContextBridge GitHub App
(`CB_PR_AUTOMATION_APP_ID`), the Bedrock OIDC role
`arn:aws:iam::212467111133:role/GitHubActionsBedrockInference-aether`, the settings file
`.aether/github.settings.json`, the `assign-agent` / `size:*` labels, the `/aether` comment
prefix, and this repo's own toolchain (mise, rust-cache, pnpm, `libdbus-1-dev`). Other repos
cannot reuse it. The issue asks for a reusable GitHub Action that other repos can install so
that **labels trigger agents** and **PR/issue comments trigger agents**, where each consumer
repo checks in its own agent configuration (different models) and the Action exposes a config
that maps a label to an aether agent defined in that settings file.

### Success criteria / acceptance conditions

1. A consumer repo can add a ~30-line caller workflow plus a checked-in
   `.aether/github.settings.json` (their own models) and get: label-on-issue triggers an
   agent, `/aether` comments on PRs/issues trigger an agent, plan/implement modes, branch
   creation, PR opening, and report-back comments.
2. The Action exposes a **label → agent mapping** (and preserves the current `size:S/M/L`
   behavior as the default mapping) so repos with different models/cost budgets can rewire
   triggers without forking code.
3. All repo-specific secrets are inputs (GitHub token/App credentials, AWS role, LLM API
   keys); nothing ContextBridge-specific is hardcoded in the reusable path.
4. This repo dogfoods the Action: `.github/workflows/aether-agent.yml` becomes a thin caller
   of the new Action with zero behavior change (same labels, same agents, same branch/PR
   conventions).
5. `zizmor` audit and `actionlint` (if adopted) stay green; all third-party actions remain
   SHA-pinned.

## Technical Approach

### High-level architectural decisions

1. **Composite Action, not a reusable workflow, not a JS action.** The existing reusable
   units in this repo (`.github/actions/configure-aws-credentials`,
   `.github/actions/fetch-release-secrets`) are composite actions referenced as
   `uses: $/.github/actions/<name>`, and consumers reference them as
   `contextbridge/aether/.github/actions/<name>@<ref>`. A composite action keeps the
   implementation in bash (the current 5 scripts run unmodified in logic), avoids a
   build/packaging step a JS action would need, and lets the caller own `on:` triggers,
   `permissions:`, `concurrency:`, and `environment:` — which composite actions cannot
   declare. A `workflow_call` reusable workflow is rejected because it forces the caller's
   secrets/environment model into the callee and composes poorly with per-repo `on:` trigger
   customization (different labels/prefixes).
2. **Split responsibilities: caller workflow vs. Action.**
   - Caller (per repo, ~30 lines, documented template): `on: issues/issue_comment/
     pull_request_review_comment`, job-level `permissions:`, `concurrency:` group,
     `environment:` for secret gating.
   - Action (`.github/actions/aether-agent/action.yml`): app-token minting (optional),
     AWS credential config (optional), aether CLI install, resolve-target → build-task →
     run aether → reviewer pass → commit-and-push → open-pr → report-back.
3. **Bundle the scripts inside the Action directory.** Move (copy, then delete originals)
   `scripts/aether-agent/{resolve-target,build-task,commit-and-push,open-pull-request,report-back}`
   to `.github/actions/aether-agent/scripts/` and invoke them via
   `${{ github.action_path }}/scripts/<name>`. This removes the current `RUNNER_TEMP` staging
   step and guarantees consumers get version-consistent scripts for the Action ref they pin.
   Rationale over alternatives: git submodules or separate repo would add release overhead;
   keeping scripts at repo root forces consumers to check out aether repo files manually.
4. **Parameterize everything repo-specific via Action inputs.** Hardcoded values in
   `aether-agent.yml` / scripts become inputs with defaults that reproduce today's behavior
   (backwards compatible for this repo):
   - `settings-file` (default `.aether/github.settings.json`), `plans-dir` (default
     `docs/aether/plans`), `issue-label` (default `assign-agent`), `comment-prefix`
     (default `/aether`), `implement-plan-command` (default `/aether implement-plan`).
   - Auth: `github-token` (fallback to `secrets.GITHUB_TOKEN`-passed value) XOR
     `app-id` + `app-private-key` + `app-owner` for App-token mode; `aws-role-to-assume`
     (empty = skip Bedrock OIDC); `zai-api-key`, `deepseek-api-key`, `openrouter-api-key`
     (empty = unset).
   - `aether-version` (default `latest`, passed to the installer URL; allows `vX.Y.Z` pin).
   - `label-agent-map`: JSON mapping label → `{ agent, mode }`, default reproduces
     `size:L → {Planner, plan}`, `size:M → {Complex Builder, implement}`,
     `size:S/"" → {Simple Builder, implement}`.
5. **Do NOT install repo toolchains in the Action.** Today's workflow installs
   `libdbus-1-dev`, mise, rust-cache, pnpm — all specific to building *this* repo. The
   reusable Action installs only the aether CLI (cargo-dist installer URL). Consumers add
   their own toolchain steps before/after via `pre-run`/`post-run` composition (plain extra
   steps in the caller workflow), documented in the template.
6. **Settings schema needs no change.** Both `.aether/settings.json` and
   `.aether/github.settings.json` already share one schema (`AetherSettings` /
   `AgentConfig` in `crates/aether-project`, camelCase, `deny_unknown_fields`). The
   label→agent map references agent `name`s resolved by the existing
   `aether headless --agent <name> --settings-file <file>` path. Document a minimal consumer
   settings file (2 agents: cheap builder + planner) rather than requiring the full 6-agent
   file.

### Design patterns to employ

- Follow the existing composite-action pattern (`action.yml` with `inputs:`, `runs.using:
  composite`, `shell: bash` steps), SHA-pin every third-party `uses:` with a `# vX.Y.Z`
  comment (satisfies `zizmor.yml`).
- Scripts keep the current contract: env-var inputs, `GITHUB_OUTPUT` outputs
  (`mode/task_kind/agent/number/branch/base/plan_file`, `path`, `pushed/implemented/subject`).
  Only the *source* of env values changes (Action inputs → `env:` blocks instead of workflow
  `env:` blocks).
- Trusted-scripts invariant is preserved automatically: scripts now ship inside the pinned
  Action ref instead of being staged from the default branch to `RUNNER_TEMP`.

### Key technical considerations and trade-offs

- **Action location: in-repo path vs. repo root vs. standalone repo.** GitHub Marketplace
  prefers `action.yml` at repo root, but cross-repo reference to
  `contextbridge/aether/.github/actions/aether-agent@<tag>` works today with zero release
  plumbing and matches existing precedent. Recommend v1 in-repo; note a future move to a
  standalone `aether-action` repo (or root `action.yml` shim that re-exports) if Marketplace
  listing is wanted. Call this out explicitly in the plan doc so the junior does not
  restructure repos.
- **Fork-PR safety:** `commit-and-push` already refuses cross-repository PRs
  (`isCrossRepository` check in `resolve-target`); keep it, and document that label triggers
  on forked issues are safe while `/aether` comments require OWNER/MEMBER/COLLABORATOR
  association (keep the `if:` association check in the caller template).
- **Credential hygiene:** keep `env -u GH_TOKEN -u GITHUB_TOKEN` on the `aether headless`
  steps, `persist-credentials: false` on checkouts, single authenticated `git push` via
  token URL. The Action must accept the token as an input and never `echo` it.
- **`$` vs `${{ github.repository }}`:** today's workflow mixes `$/.github/...` self-refs
  with hardcoded `owner: contextbridge, repositories: aether` in the app-token step.
  `app-owner` defaults to `${{ github.repository_owner }}` and repositories default to the
  current repo name (parse from `github.repository`), so forks/other orgs work.
- **Reviewer pass:** keep as an Action boolean input `enable-reviewer` (default `true`) with
  the same skip condition (`mode == plan` or `task_kind == pull_request_feedback`).

## Implementation Steps

1. **Create `.github/actions/aether-agent/action.yml` skeleton.**
   Define `name: Aether agent`, `description`, all `inputs:` (see Technical Approach §4:
   `github-token`, `app-id`, `app-private-key`, `app-owner`, `aws-role-to-assume`,
   `aws-region` default `us-west-2`, `role-duration-seconds` default `7200`,
   `settings-file`, `plans-dir`, `issue-label`, `comment-prefix`,
   `implement-plan-command`, `label-agent-map` JSON string, `aether-version`,
   `zai-api-key`, `deepseek-api-key`, `openrouter-api-key`, `enable-reviewer`), and
   `outputs:` (`branch`, `pr-number`, `pushed`, `mode`). No logic yet; `runs.using:
   composite`.
2. **Vendor the scripts into the Action.** Copy `scripts/aether-agent/*` (5 files) to
   `.github/actions/aether-agent/scripts/` verbatim. Update the one self-reference that
   assumes repo-root layout if any (verify: scripts use only env vars + `gh`/`git`/`jq`, no
   repo paths except `PLANS_DIR`/`GITHUB_WORKSPACE` — no edit expected).
3. **Parameterize `resolve-target`.** Replace hardcoded `assign-agent` check (currently in
   workflow `if:`), `size:L/M/S` case statement, `/aether implement-plan` string, and
   `aether/issue-N` branch prefix with env vars `ISSUE_LABEL`, `COMMENT_PREFIX`,
   `IMPLEMENT_PLAN_COMMAND`, `LABEL_AGENT_MAP` (JSON, parsed with `jq`; default JSON
   reproduces size:L/M/S behavior), `BRANCH_PREFIX` (default `aether/issue-`). Emit identical
   `GITHUB_OUTPUT`s. Pseudo-code for the size→agent block:
   ```bash
   agent="$(jq -r --arg s "$size" '.[$s] // .[""] | .agent' <<< "$LABEL_AGENT_MAP")"
   mode="$(jq -r --arg s "$size" '.[$s] // .[""] | .mode' <<< "$LABEL_AGENT_MAP")"
   ```
4. **Parameterize remaining scripts.** `build-task`: replace `.git/aether-task.json` const
   with `TASK_FILE` env (default `.git/aether-task.json`). `commit-and-push`,
   `open-pull-request`, `report-back`: replace `PLANS_DIR` const with env (already env in
   `commit-and-push`; thread through Action `env:`). No logic changes.
5. **Write the composite steps** in `action.yml`, mirroring `aether-agent.yml` steps in
   order, with `${{ github.action_path }}/scripts/<name>` invocations:
   app-token (skip if `app-id` empty → fall back to `github-token`), acknowledge-reactions,
   checkout (`actions/checkout`, `fetch-depth: 0`, `persist-credentials: false`,
   `token: <resolved>`), AWS OIDC via `../configure-aws-credentials` (skip if
   `aws-role-to-assume` empty), install aether CLI honoring `aether-version` (URL:
   `.../releases/<version>/download/aether-agent-cli-installer.sh`, `latest` for default),
   resolve → branch → task → run → reviewer (gated on `enable-reviewer`) → push → PR →
   report-back. Deliberately OMIT mise/rust-cache/pnpm/apt steps.
6. **Add caller template** `.github/actions/aether-agent/caller-workflow.example.yml`
   (or `docs/aether/github-action-template.yml`): `on:` block with `issues.labeled`,
   `issue_comment.created`, `pull_request_review_comment.created`; job `permissions:
   contents: read, id-token: write, pull-requests: write, issues: write`; `concurrency:`
   group; `environment: ci`; association/`startsWith` `if:` guard using
   `inputs`-independent expressions; single `uses: ./ .github/actions/aether-agent` step
   (self) with comment showing the cross-repo pin form. Also document minimal consumer
   `.aether/github.settings.json` (2 agents) inline as comments.
7. **Dogfood: rewrite `.github/workflows/aether-agent.yml`** as a thin caller of the new
   Action, passing today's values explicitly (app id/key secrets, Bedrock role ARN,
   `settings-file: .aether/github.settings.json`, default label map). Keep the workflow's
   `on:`/`permissions:`/`concurrency:`/`environment:` identical. Keep the toolchain steps?
   Decision: keep mise/rust-cache/pnpm/apt steps in the *caller* (this repo needs them to
   build itself) — document that these are caller-owned, not Action-owned.
8. **Delete `scripts/aether-agent/`** (or leave thin deprecated shims that error with a
   pointer to the Action path — prefer deletion; backwards compat is not a concern per repo
   style). Update any references (grep for `aether-agent/` across workflows/docs).
9. **Docs:** add `packages/website/src/content/docs/aether/running/github-action.mdx`
   (install snippet, inputs table, label→agent map example, settings-file example, auth
   options, fork-safety notes) and link it from the appropriate docs sidebar/index. Update
   root `README.md` only if it references the old workflow (check; keep the diff minimal).

## Testing Plan

- **Unit tests (bash):** add `bats` or plain `bash` test harness? Repo has no bash test
  infra today — use `shellcheck` + `bash -n` syntax checks in CI (`zizmor.yml` adjacent or
  `ci.yml` step) rather than introducing a framework. At minimum: `shellcheck -S warning`
  on all 5 vendored scripts + `action.yml` YAML parse check.
- **Logic tests for `resolve-target` (manual matrix, documented):** fake `GITHUB_EVENT_PATH`
  payloads for (a) issue labeled `size:L` → Planner/plan, (b) `size:M` → Complex
  Builder/implement, (c) no size → Simple Builder/implement, (d) conflicting sizes → error,
  (e) `/aether implement-plan` on PR → implement_plan, (f) other `/aether` comment →
  pull_request_feedback, (g) custom `LABEL_AGENT_MAP` mapping a custom label
  (e.g. `aether-docs`) to a custom agent → verifies the new mapping path. Run each with
  stubbed `gh` on PATH asserting `GITHUB_OUTPUT` lines. Record results in the PR body.
- **Integration tests:** (1) `actionlint` on the caller template + rewritten
  `aether-agent.yml` if the tool is available, else `yamllint`-level eyeball; (2) `zizmor`
  must pass (it already scans `.github/`); (3) dogfood run: label a test issue
  `assign-agent` + `size:S` on this repo after merge and confirm branch/PR/report-back end
  to end; (4) consumer simulation: temporary test repo (or local `act` run) using the
  template with `github-token` auth + custom settings file, verifying a label trigger
  produces a branch.
- **Edge cases to verify:** fork PR comment (must refuse with clear error, no push);
  existing open PR for branch (open-pr no-op); re-label collision (`branch-N` suffix path);
  empty agent output (report-back "made no changes"); `aws-role-to-assume` empty (OIDC step
  skipped, Bedrock models fail with actionable error); `app-id` empty (falls back to
  `github-token`); `aether-version` pinned tag (installer URL resolves).

## Files to Modify/Create

| File | Change | Add/Mod/Del |
|---|---|---|
| `.github/actions/aether-agent/action.yml` | New composite Action: inputs/outputs + all agent steps | Add |
| `.github/actions/aether-agent/scripts/resolve-target` | Vendored + parameterized (label map, prefix, commands) | Add |
| `.github/actions/aether-agent/scripts/build-task` | Vendored + `TASK_FILE` env | Add |
| `.github/actions/aether-agent/scripts/commit-and-push` | Vendored (env passthrough) | Add |
| `.github/actions/aether-agent/scripts/open-pull-request` | Vendored (env passthrough) | Add |
| `.github/actions/aether-agent/scripts/report-back` | Vendored (env passthrough) | Add |
| `.github/actions/aether-agent/caller-workflow.example.yml` | Thin caller template + consumer settings example | Add |
| `.github/workflows/aether-agent.yml` | Rewrite as thin caller of the new Action (same triggers/behavior) | Mod |
| `scripts/aether-agent/*` (5 files) | Delete after vendoring (update all references) | Del |
| `packages/website/src/content/docs/aether/running/github-action.mdx` | New docs page: install, inputs, label map, auth | Add |
| `README.md` | Mention reusable Action only if it already mentions the workflow | Mod |

## Additional Notes

- **Documentation updates needed:** the new `github-action.mdx` page is the primary doc;
  keep the caller template's inline comments in sync with the inputs table (single source
  of truth = `action.yml` `inputs:` descriptions).
- **Follow-up tasks that may be spawned:** (1) Marketplace/standalone-repo decision if
  discoverability demands it; (2) `aether-version` pinning against CLI release tags
  (`aether-agent-cli-v*` vs `latest` — verify installer URL layout for pinned versions);
  (3) `update-models.yml`-style automation is out of scope; (4) consider JSON-schema for
  `LABEL_AGENT_MAP` + a `--validate` dry-run mode in `resolve-target`; (5) `fetch-release-secrets`
  is unreferenced — do not wire it into the Action unless a consumer need appears.
- **Explicit non-goals for v1:** building consumer repos' toolchains (mise/cargo/pnpm stay
  caller-owned); supporting multiple simultaneous agents per label; self-hosted runner
  support (ubuntu-latest only, matching today).
