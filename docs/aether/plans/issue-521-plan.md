# Issue #521 — Automate TS Package Publishing When Release PR Is Merged

## Overview

### Problem statement

Rust releases are fully automated: `release-plz` opens a `chore: release` PR, and when that PR merges to `main`, the `release-plz-release` job pushes `aether-agent-cli-vX.Y.Z` tags, which trigger `release.yml` (cargo-dist) to build CLI artifacts and publish the `@aether-agent/cli` npm package.

The TypeScript packages are **not** on that train:

- `@aether-agent/sdk` (`packages/aether-sdk/package.json`, currently `0.7.1` with `"@aether-agent/cli": "^0.9.5"`) is published only when someone manually pushes an `aether-sdk-v*` tag (workflow `release-sdk.yml`).
- `@aether-agent/evals` (`packages/aether-evals/package.json`, currently `0.4.1`) is published only when someone manually pushes an `aether-evals-ts-v*` tag (workflow `release-evals.yml`). The `-ts-` infix exists to avoid colliding with the Rust crate tags `aether-evals-v*` (release-plz manages the `aether-evals` Rust crate).
- Before tagging, someone must manually open a version-bump PR that (a) raises the SDK's `@aether-agent/cli` dep to the newly released CLI version, (b) patch-bumps the SDK and evals versions, and (c) refreshes `pnpm-lock.yaml`. Precedents: PR #503 ("update TypeScript packages for Aether 0.9.5": CLI dep `^0.9.2` → `^0.9.5`, sdk `0.7.0` → `0.7.1`, evals `0.4.0` → `0.4.1`) and PR #483 (same shape for CLI 0.9.2).

Issue #521 asks: when a new CLI is released via merging the release PR, auto-bump the sdk and evals versions (with the bumped CLI dep) and release them, so no human has to remember the manual tag dance. The repo is currently at CLI `0.9.7` (`crates/aether-cli/Cargo.toml`) while the SDK still pins `^0.9.5` — i.e. two CLI releases behind — which is the concrete symptom.

### Success criteria / acceptance conditions

1. Merging a `release-plz` release PR that bumps `aether-agent-cli` (or pushing an `aether-agent-cli-v*` tag / publishing its GitHub Release) results in `@aether-agent/sdk` and `@aether-agent/evals` being published to npm **without human intervention**, with the SDK's `@aether-agent/cli` dependency range pointing at the new CLI version.
2. Versioning is deterministic: each CLI release produces exactly one patch bump of sdk and evals (e.g. CLI `0.9.7` → sdk `0.7.2`, evals `0.4.2`), unless the SDK/evals `package.json` versions were already manually bumped higher (then leave them alone and only fix the CLI dep if stale).
3. `pnpm-lock.yaml` is refreshed as part of the automation so `pnpm install --frozen-lockfile` passes on the bump commit.
4. The existing publish workflows (`release-sdk.yml` on `aether-sdk-v*`, `release-evals.yml` on `aether-evals-ts-v*`) keep working unchanged — the automation produces the version-bump commit **and** the tags that trigger them. No duplicate publishes, no infinite workflow loops.
5. Idempotent and safe to re-run: if the SDK already depends on the released CLI version, the workflow exits with no commit and no tags.
6. Failures are visible (workflow failure + summary) and never publish a half-bumped state (e.g. tag pushed but lockfile stale, or evals published against an unpublished SDK version).

## Technical Approach

### High-level architectural decisions

**Decision 1 — Trigger on the CLI release, not on `main` push.**
The most precise "a new CLI was released" signal is the `aether-agent-cli-v*` tag push (or the GitHub Release published by `release.yml`). Triggering on every `main` push and diffing `Cargo.toml` is noisier and races with release-plz's own tag push. Recommended trigger:

```yaml
on:
  release:
    types: [published]
```

filtered to tag pattern `aether-agent-cli-v*`, **or** equivalently `push: tags: aether-agent-cli-v*`.

- `release: published` is preferred over `push: tags:` because `release.yml` creates the GitHub Release **after** building artifacts and publishing the `@aether-agent/cli` npm tarball (`publish-npm` job). Triggering the SDK bump only after that point eliminates the race where the SDK publish job runs before the new `@aether-agent/cli` version exists on the registry (SDK `pnpm install` would fail to resolve the new CLI version).
- Fallback if `release`-event filtering proves fiddly: `workflow_run` on completion of the `Release` workflow (conclusion `success`, for tag `aether-agent-cli-v*`). This is more verbose and is the backup option.

**Decision 2 — Fully automatic commit + tag push to `main`, no intermediate human-reviewed PR.**
The issue explicitly wants to remove the manual step. A bot-opened PR that waits for human merge (release-plz style) does not satisfy "auto … release them". The bump is mechanical (3 files + lockfile) and is validated by CI before tagging, so direct push is justified. Use the existing `CB_PR_AUTOMATION_APP_ID` / `CB_PR_AUTOMATION_APP_PRIVATE_KEY` app token (same as `release-plz.yml` and `release.yml` homebrew job) for push permission, not the default `GITHUB_TOKEN`.

**Decision 3 — Reuse the existing tag-gated publish workflows; don't merge publishing into the bump workflow.**
`release-sdk.yml` and `release-evals.yml` already implement trusted publishing (`environment: ci`, `id-token: write`, `--provenance`). The new workflow only creates the bump commit and pushes the two tags (`aether-sdk-v<new>` and `aether-evals-ts-v<new>`). This keeps the provenance/attestation path unchanged, minimizes blast radius, and preserves the ability to manually push a tag for an out-of-band release. Publishing order is guaranteed by npm semantics: sdk tag workflow publishes first; evals depends on `workspace:^` which resolves to the just-bumped local version at build time, but at install time on the registry it resolves to `^<sdk-version>` — so the evals tag must point at a commit where the SDK version is already the newly published one (same commit satisfies this since both bumps land atomically).

**Decision 4 — Patch-bump policy with caret CLI dep.**
Mirror the precedent set by #483/#503:

- SDK `@aether-agent/cli` dep: `^<new-cli-version>` (exact caret bump, e.g. `^0.9.5` → `^0.9.7`).
- SDK version: `patch+1` (e.g. `0.7.1` → `0.7.2`).
- Evals version: `patch+1` (e.g. `0.4.1` → `0.4.2`). Evals' `@aether-agent/sdk` dep stays `workspace:^` (no change needed — pnpm resolves it, and on publish it becomes `^<sdk-version>`).
- Guard: if `package.json` version is already higher than computed bump (human shipped a minor/major), keep the higher version and only fix the CLI dep.

**Decision 5 — A small checked-in script owns the version math, the workflow owns git/tag I/O.**
Put the parsing/bumping logic in a testable Node script (no new dependencies — plain `node`, `fs`, `child_process`) rather than inline bash/`jq`. The workflow calls it, then runs the standard validation (`pnpm install`, typecheck, build, tests), commits, and tags.

### Design patterns to employ

- **Idempotent reconciler**: the script reads desired state (CLI version from `crates/aether-cli/Cargo.toml`) and current state (both `package.json` files), and no-ops when already converged. Exit code or `$GITHUB_OUTPUT` flag (`bumped=true/false`) gates all downstream steps.
- **Atomic commit**: one commit contains both `package.json` edits + refreshed `pnpm-lock.yaml`, so neither tag can point at a half-bumped tree.
- **Tag-after-validation**: tags are created only after `pnpm install --frozen-lockfile`, `sdk:typecheck`, `sdk:build`, `sdk:test`, `evals:typecheck`, `evals:build`, `evals:test` all pass on the bump commit. Tag push is the last step.
- **Concurrency guard**: `concurrency: group: ts-release-<ref>` (or on the CLI tag) with `cancel-in-progress: false` so two CLI releases racing can't interleave commits/tags.
- **Builder pattern for tests** (per repo testing guidelines): if the script exposes helpers, test through the public CLI entry-point asserting on resulting `package.json` contents using an in-memory temp-dir fixture, not mocks.

### Key technical considerations and trade-offs

1. **Tag-triggered infinite loops.** Pushing `aether-sdk-v*` / `aether-evals-ts-v*` tags triggers the publish workflows, which do not push commits/tags, so no cycle. The new workflow must trigger **only** on `aether-agent-cli-v*` (never on sdk/evals tags or on `main` pushes it creates itself). If implemented with `release: published`, filter `github.event.release.tag_name` startswith `aether-agent-cli-v` in a first-step guard.
2. **npm availability race.** The SDK bump commit runs `pnpm install`, which must resolve `@aether-agent/cli@^<new>` from the registry. `release: published` ordering (after `publish-npm`) plus a short retry loop on `pnpm install` (3 attempts, 60s apart) covers propagation delay. Do **not** publish the SDK from a local tarball of the CLI — the SDK declares a registry dependency.
3. **The `-ts-` infix is load-bearing.** Rust crate `aether-evals` owns the `aether-evals-v*` tag namespace via release-plz (`release-plz.toml` git tag format `{{ package }}-v{{ version }}`). The TS evals package must keep using `aether-evals-ts-v*` (what `release-evals.yml` listens on). Do not "normalize" it.
4. **`workspace:^` semantics.** `packages/aether-evals/package.json` depends on `"@aether-agent/sdk": "workspace:^"`. At `pnpm publish` time this is rewritten to the current SDK version range. Since both bumps land in one commit, publishing sdk first then evals yields a consistent pair. Document in the workflow that if the sdk publish fails, the evals tag must not be pushed (push sdk tag, wait/verify, then push evals tag — or push both and rely on evals workflow's `pnpm sdk:build` step failing visibly; preferred: push sdk tag first, then evals tag, as two explicit steps so a sdk-tag failure blocks the evals-tag step via `needs`/step ordering in the same job).
5. **Permissions.** Needs `contents: write` (push commit + tags to `main`) via the automation app token, plus `id-token: write` is **not** needed in the bump workflow (only the existing publish workflows need it). Top-level `permissions: {}` with per-job grants, matching `release-plz.yml` style.
6. **Backfill.** The repo is currently skewed (CLI `0.9.7` vs SDK dep `^0.9.5`). The first run of the automation (manual `workflow_dispatch` with input `cli_version`, defaulting to current `Cargo.toml` version) should close this gap: bump SDK dep to `^0.9.7`, sdk → `0.7.2`, evals → `0.4.2`, push tags. Include this as an explicit step-0 validation in the plan (run workflow manually once, verify npm shows the new versions, then rely on automation going forward).
7. **Alternative considered and rejected — extending release-plz to own TS versions.** release-plz is Cargo-centric; it cannot version pnpm workspaces, update `pnpm-lock.yaml`, or push npm tags for non-Cargo packages. Teaching it to do so (via `pre-release-hook` hacks) couples two release systems and breaks `release-plz.toml`'s clean Cargo-only contract. A dedicated workflow + script is simpler and independently testable.
8. **Alternative considered and rejected — Changesets.** Adopting Changesets would give full JS-side versioning/changelogs but requires migrating the TS release process, adding a bot, and retraining contributors, for a use case that is currently a pure function of the CLI version. Overkill; revisit only if the SDK needs independent (non-CLI-driven) releases with changelogs.

## Implementation Steps

### Step 1 — Add the bump script `scripts/bump-ts-packages.mjs`

Create a dependency-free Node script (repo already runs Node 24 via mise; `.tool-versions` pins `nodejs 24.15.0`):

- Inputs: `--cli-version <semver>` (defaults to parsing `crates/aether-cli/Cargo.toml` `version = "x.y.z"`), `--repo-root` (defaults to script's grandparent), `--check` (dry-run flag for CI/tests).
- Behavior:
  1. Read CLI version (strip pre-release suffix handling: if CLI version contains `-`, carry it into the caret range verbatim, e.g. `^1.0.0-beta.1`; document that pre-release CLI versions still bump TS packages — dist honors `announcement_is_prerelease`, but npm publish of sdk/evals for a pre-release CLI should still proceed since the dep range is satisfiable).
  2. Read `packages/aether-sdk/package.json` and `packages/aether-evals/package.json`.
  3. Compute: `sdkCliDep = ^<cliVersion>`; `newSdkVersion = max(currentSdk + patch, currentSdk if already greater)`; `newEvalsVersion = max(currentEvals + patch, ...)`. Patch increment = `(major, minor, patch+1)` on valid `x.y.z`; if current version is pre-release, just drop the pre-release suffix comparison and bump patch of the base (keep it simple; log a warning).
  4. Converged check: if SDK dep already equals `^<cliVersion>` **and** neither version needs a bump (because a human already bumped them for this CLI), print `already in sync` and exit `0` with output `bumped=false` (write to `$GITHUB_OUTPUT` when env var `GITHUB_OUTPUT` is set).
  5. Otherwise rewrite the two `package.json` files preserving 2-space formatting + trailing newline (matching existing files), print a summary (`cli 0.9.5 → 0.9.7, sdk 0.7.1 → 0.7.2, evals 0.4.1 → 0.4.2`), exit `0` with `bumped=true`.
- Pseudo-code:

```js
// scripts/bump-ts-packages.mjs
import { readFileSync, writeFileSync } from "node:fs";
const cliVersion = getCliVersion(argv);           // from --cli-version or Cargo.toml
const sdk = readJson("packages/aether-sdk/package.json");
const evals = readJson("packages/aether-evals/package.json");
const wantDep = `^${cliVersion}`;
if (sdk.dependencies["@aether-agent/cli"] === wantDep && !needsBump(sdk, evals)) {
  emitOutput("bumped", "false"); return;
}
sdk.dependencies["@aether-agent/cli"] = wantDep;
sdk.version = bumpPatch(sdk.version);
evals.version = bumpPatch(evals.version);
writeJson("packages/aether-sdk/package.json", sdk);
writeJson("packages/aether-evals/package.json", evals);
emitOutput("bumped", "true");
emitOutput("sdk_version", sdk.version);
emitOutput("evals_version", evals.version);
emitOutput("cli_version", cliVersion);
```

### Step 2 — Add unit coverage for the script

Add `scripts/bump-ts-packages.test.mjs` (or a `vitest` suite if the team prefers; the script must stay runnable with plain `node --test` so CI needs no new harness — check what the repo uses for `scripts/`; `packages/aether-sdk/scripts/e2e.mjs` has no tests today, so plain `node --test` is the lightest consistent choice):

- Cases: (a) stale dep + normal versions → dep updated, both patch-bumped; (b) already converged → no file changes, `bumped=false`; (c) SDK already at a higher minor (e.g. `0.8.0` vs computed `0.7.2`) → version preserved, dep still updated; (d) pre-release CLI version → caret range carries suffix; (e) `--check` mode exits non-zero when changes would be made (for a CI drift check, optional).
- Fixtures: copy the two real `package.json` shapes into a temp dir per test (builder-style helper `makePackageDir({sdkVersion, evalsVersion, cliDep})`), run the script as a subprocess against the temp dir, assert on resulting JSON contents (state-based, not call-count-based, per repo testing guidelines). Tests assert only the script's public CLI contract.

### Step 3 — Create `.github/workflows/release-ts.yml` (the automation workflow)

New workflow file:

```yaml
name: Release TS packages
on:
  release:
    types: [published]
  workflow_dispatch:
    inputs:
      cli_version:
        description: "CLI version to sync SDK/evals to (default: from crates/aether-cli/Cargo.toml)"
        required: false
        type: string

permissions: {}

concurrency:
  group: ts-release
  cancel-in-progress: false

jobs:
  bump-and-tag:
    runs-on: ubuntu-latest
    environment: ci
    permissions:
      contents: write
    steps:
      # 1. Guard: only proceed for aether-agent-cli releases (skip for other releases when triggered by `release`).
      #    `if:` on tag prefix, or an early exit step that sets `proceed=false`.
      # 2. Generate app token (CB_PR_AUTOMATION_APP_ID / _PRIVATE_KEY, owner contextbridge, repos aether, permission-contents write) — same pattern as release-plz.yml.
      # 3. Checkout with fetch-depth 0 + token, mise setup, pnpm setup (node-version-file .tool-versions).
      # 4. Run node scripts/bump-ts-packages.mjs [--cli-version <input or tag-derived>] → $GITHUB_OUTPUT (bumped, sdk_version, evals_version, cli_version). Tag-derived = strip `aether-agent-cli-v` prefix from github.event.release.tag_name.
      # 5. If bumped=false → success exit with summary "already in sync".
      # 6. pnpm install (with retry loop for registry propagation) → regenerates pnpm-lock.yaml against the new CLI version.
      # 7. Validate: pnpm sdk:typecheck, pnpm sdk:build, pnpm sdk:test, pnpm evals:typecheck, pnpm evals:build, pnpm evals:test (mirror the `sdk` job in ci.yml; reuse the same apt/mise steps incl. libdbus-1-dev).
      # 8. Commit: git config bot identity, git add packages/aether-sdk/package.json packages/aether-evals/package.json pnpm-lock.yaml, git commit -m "chore(deps): update TypeScript packages for Aether <cli_version>".
      #    Include the #503-style body (CLI dep, sdk/evals versions) for traceability. Push to main.
      # 9. Tag sdk: git tag aether-sdk-v<sdk_version> && git push origin aether-sdk-v<sdk_version>  (fires release-sdk.yml).
      # 10. Tag evals: git tag aether-evals-ts-v<evals_version> && git push origin aether-evals-ts-v<evals_version> (fires release-evals.yml). Runs only if step 9 succeeded.
```

Key details:

- Tag→version derivation: `aether-agent-cli-v0.9.7` → `0.9.7`. Cross-check against `crates/aether-cli/Cargo.toml` on the release commit; if they differ, prefer the tag (the thing actually released) and log a warning.
- Commit message convention: `chore(deps): update TypeScript packages for Aether <cli_version>` — matches #503/#483 so history stays greppable and release-plz ignores it (no `feat:`/`fix:` prefix → no spurious Rust bumps).
- The commit must **not** touch any `crates/**` files, otherwise release-plz would see a Rust change and open another release PR (loop risk). Guard with `git status --porcelain` asserting only the 3 allowed files changed.
- Step 6 retry: `for i in 1 2 3; do pnpm install --frozen-lockfile=false && break || sleep 60; done` — first run updates the lockfile; subsequent CI runs use `--frozen-lockfile`. (Initial `pnpm install` without `--frozen-lockfile` is what rewrites the `@aether-agent/cli` resolution entry, exactly as #503 did by hand.)

### Step 4 — Backfill the current skew via `workflow_dispatch`

Run the new workflow manually once with `cli_version: 0.9.7` (current `Cargo.toml`):

1. Verify the bump commit updates SDK dep `^0.9.5` → `^0.9.7`, sdk `0.7.1` → `0.7.2`, evals `0.4.1` → `0.4.2`, plus lockfile.
2. Verify tags `aether-sdk-v0.7.2` and `aether-evals-ts-v0.4.2` appear and both publish workflows go green.
3. Verify on npm: `@aether-agent/sdk@0.7.2` depends on `@aether-agent/cli@^0.9.7`; `@aether-agent/evals@0.4.2` exists.

### Step 5 — Verify end-to-end on the next real CLI release

On the next `chore: release` PR that bumps `aether-agent-cli` (e.g. `0.9.7` → `0.9.8`):

1. Confirm `release-ts.yml` fires on the published `aether-agent-cli-v0.9.8` release.
2. Confirm the bump commit + both tags land with no human action.
3. Confirm `release-sdk.yml` / `release-evals.yml` publish the new versions.
4. Confirm no extra release-plz PR was spawned by the bot commit.

### Step 6 — Document the release train

Update the release documentation (repo `README.md` release section if present, or the website docs page covering releases — locate during implementation; at minimum add a short comment header in `release-ts.yml` and a paragraph in the plan's follow-up):

- Diagram: `release-plz PR merge → aether-agent-cli-v* tag → cargo-dist Release (CLI npm) → release-ts (bump commit + sdk/evals tags) → sdk/evals npm publishes`.
- Note the manual escape hatch: pushing `aether-sdk-v*` / `aether-evals-ts-v*` by hand still works for out-of-band fixes; the automation no-ops when already converged.

## Testing Plan

### Unit tests required

- `scripts/bump-ts-packages.test.mjs` (new, plain `node --test`):
  - Stale dep bumps dep + patch-bumps both packages.
  - Converged tree → no writes, `bumped=false`.
  - Higher-than-computed SDK version preserved (only dep fixed).
  - Pre-release CLI version carried into caret range.
  - Malformed `Cargo.toml` (no version) → non-zero exit with a clear error naming the file.
- All tests assert on file contents (state-based) using temp-dir fixtures; no mocks of `fs`.

### Integration tests needed

- Dry-run the script against the real repo in `--check` mode on the current tree: with CLI at `0.9.7` and SDK dep at `^0.9.5`, `--check` must report drift (proving it would have caught the current skew).
- Manual `workflow_dispatch` backfill run (Step 4) is the integration test for the workflow itself: lockfile refresh, full `ci.yml`-parity TS validation (typecheck/build/test for sdk + evals), commit, and both tag-triggered publishes.
- Next-release observation (Step 5) is the acceptance test for the `release: published` trigger and ordering (no npm-resolution race).

### Edge cases to verify

| # | Edge case | Expected behavior |
|---|-----------|-------------------|
| 1 | SDK/evals already in sync (re-run, duplicate delivery of `release` event) | No commit, no tags, green success with "already in sync" summary |
| 2 | CLI pre-release (e.g. `1.0.0-beta.1`) published | SDK dep becomes `^1.0.0-beta.1`; sdk/evals patch-bump and publish (dist marks GitHub release prerelease; npm publish still proceeds — confirm this is desired, else add a guard to skip pre-releases) |
| 3 | Human already bumped SDK minor for the same CLI (e.g. sdk `0.8.0` with stale CLI dep) | Dep updated, SDK version kept at `0.8.0`, evals patch-bumped normally |
| 4 | `@aether-agent/cli` not yet visible on npm when bump job runs | Retry loop on `pnpm install`; job fails visibly after 3 attempts without pushing tags |
| 5 | Two CLI releases in quick succession | `concurrency.group: ts-release`, `cancel-in-progress: false` serializes runs; second run sees first run's commit and bumps from there |
| 6 | Evals publish runs before SDK publish finishes | Evals tag pushed only after SDK tag push step succeeds; evals workflow builds against the same commit so its `workspace:^` resolves to the new SDK version |
| 7 | Bot commit accidentally touches `crates/**` | Guard step fails the job before commit if any file outside the 3-file allowlist is modified |
| 8 | Tag already exists (e.g. manual tag pushed for same version) | `git push origin <tag>` fails; job surfaces the failure instead of silently skipping — operator deletes or bumps; document in workflow summary |

## Files to Modify/Create

| File | Change | Add / Modify / Remove |
|------|--------|----------------------|
| `scripts/bump-ts-packages.mjs` | New dependency-free Node script: derives CLI version, updates SDK CLI dep to `^<cli>`, patch-bumps sdk + evals, idempotent with `bumped` output flag | Add |
| `scripts/bump-ts-packages.test.mjs` | Unit tests for the script via plain `node --test` with temp-dir fixtures, state-based assertions | Add |
| `.github/workflows/release-ts.yml` | New workflow: triggers on `release.published` for `aether-agent-cli-v*` (+ `workflow_dispatch` backfill input); app-token auth; bump → `pnpm install` (retry) → full TS validation → atomic commit (3-file allowlist guard) → push sdk tag then evals tag | Add |
| `packages/aether-sdk/package.json` | Automated at runtime by the workflow (CLI dep `^<new>`, patch version bump); **not** hand-edited in the implementing PR | Modify (runtime) |
| `packages/aether-evals/package.json` | Automated at runtime by the workflow (patch version bump); **not** hand-edited in the implementing PR | Modify (runtime) |
| `pnpm-lock.yaml` | Automated at runtime by the workflow (`pnpm install` refresh of the `@aether-agent/cli` resolution); **not** hand-edited in the implementing PR | Modify (runtime) |
| `.github/workflows/release-sdk.yml` | No change (kept as the publisher; documents the contract the new workflow relies on) | Modify (none — reference only) |
| `.github/workflows/release-evals.yml` | No change (same as above) | Modify (none — reference only) |
| Release docs (`README.md` or website releases page — locate during implementation) | Short paragraph + diagram of the automated train and the manual-tag escape hatch | Modify |

## Additional Notes

### Documentation updates needed

- In-code: header comment in `release-ts.yml` explaining trigger choice (`release.published` after CLI npm publish), the `-ts-` infix rationale, and the 3-file commit allowlist.
- User-facing: release-process paragraph (location TBD during implementation) so contributors stop hand-cutting the #503-style bump PRs and instead use `workflow_dispatch` only for backfills.

### Follow-up tasks that may be spawned

1. **Pre-release policy decision**: confirm whether a pre-release CLI (e.g. `1.0.0-beta.1`) should auto-publish sdk/evals to npm (possibly under a `beta` dist-tag) or skip. The plan defaults to "publish like normal"; if maintainers want skips, add a tag-suffix guard in step 1 of the workflow. This is the one point where a maintainer clarifying answer before implementation would help.
2. **Provenance check on evals ordering**: if npm shows evals resolving a stale SDK range after the first automated run, switch the evals tag step to wait for the SDK npm version to be visible (poll `npm view @aether-agent/sdk versions`) before pushing the evals tag.
3. **Consider `workflow_run` fallback**: if `release.published` proves unreliable for dist-created releases, re-trigger on `workflow_run: Release / completed / success` with tag filtering.
4. **Long-term**: if the SDK ever needs releases decoupled from the CLI (features without a CLI change), adopt Changesets; until then the CLI-driven patch policy in this plan is sufficient and simpler.
5. **Drift check in CI (optional)**: add a lightweight `ci.yml` step running `node scripts/bump-ts-packages.mjs --check` to fail PRs that bump the CLI crate version without the corresponding TS bump — redundant once automation lands, but useful as a safety net during rollout.
