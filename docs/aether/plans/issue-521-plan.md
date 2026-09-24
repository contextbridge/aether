# Issue #521 — Automate TS Package Publishing When Release PR Is Merged

## Overview

### Problem statement

Rust releases are fully automated: `release-plz` opens a `chore: release` PR, and when that PR merges to `main`, the `release-plz-release` job pushes `aether-agent-cli-vX.Y.Z` tags, which trigger `release.yml` (cargo-dist) to build CLI artifacts and publish the `@aether-agent/cli` npm package.

The TypeScript packages are **not** on that train:

- `@aether-agent/sdk` (`packages/aether-sdk/package.json`, currently `0.7.1` with `"@aether-agent/cli": "^0.9.5"`) is published only when someone manually pushes an `aether-sdk-v*` tag (workflow `release-sdk.yml`).
- `@aether-agent/evals` (`packages/aether-evals/package.json`, currently `0.4.1`) is published only when someone manually pushes an `aether-evals-ts-v*` tag (workflow `release-evals.yml`). The `-ts-` infix exists to avoid colliding with the Rust crate tags `aether-evals-v*` (release-plz manages the `aether-evals` Rust crate).
- Before tagging, someone must manually open a version-bump PR that (a) raises the SDK's `@aether-agent/cli` dep to the newly released CLI version, (b) patch-bumps the SDK and evals versions, and (c) refreshes `pnpm-lock.yaml`. Precedents: PR #503 ("update TypeScript packages for Aether 0.9.5": CLI dep `^0.9.2` → `^0.9.5`, sdk `0.7.0` → `0.7.1`, evals `0.4.0` → `0.4.2`) and PR #483 (same shape for CLI 0.9.2).

Issue #521 asks: when a new CLI is released via merging the release PR, auto-bump the sdk and evals versions (with the bumped CLI dep) and release them, so no human has to remember the manual tag dance. The repo is currently at CLI `0.9.7` (`crates/aether-cli/Cargo.toml`) while the SDK still pins `^0.9.5` — i.e. two CLI releases behind — which is the concrete symptom.

### Success criteria / acceptance conditions

1. Publishing a CLI release (`aether-agent-cli-v*` tag / GitHub Release) results in `@aether-agent/sdk` and `@aether-agent/evals` being published to npm **without routine human intervention**, with the SDK's `@aether-agent/cli` dependency range pointing at the new CLI version.
2. A CLI bump that contains **breaking changes requiring TS code updates can never auto-publish a broken SDK**: the version bumps land in a pull request where the full TS validation suite (typecheck, build, tests for both sdk and evals, plus formatting and lockfile checks) runs against the bumped tree, and the npm tags are created only after that PR merges green.
3. Versioning is deterministic: each CLI release produces exactly one patch bump of sdk and evals (e.g. CLI `0.9.7` → sdk `0.7.2`, evals `0.4.2`), unless the SDK/evals `package.json` versions were already manually bumped higher (then leave them alone and only fix the CLI dep if stale).
4. `pnpm-lock.yaml` is refreshed as part of the automation so `pnpm install --frozen-lockfile` passes on the bump PR.
5. The existing publish workflows (`release-sdk.yml` on `aether-sdk-v*`, `release-evals.yml` on `aether-evals-ts-v*`) keep working unchanged — the automation produces the version-bump PR and, after merge, the tags that trigger them. No duplicate publishes, no infinite workflow loops.
6. Idempotent and safe to re-run: if the SDK already depends on the released CLI version, or a bump PR for that CLI version is already open, the workflow exits with no new PR and no tags.
7. Failures are visible (workflow failure + PR checks + summary) and never publish a half-bumped state (e.g. tag pushed but lockfile stale, or evals published against an unpublished SDK version).

## Technical Approach

### High-level architectural decisions

**Decision 1 — Trigger the bump on the CLI release, not on `main` push.**
The most precise "a new CLI was released" signal is the published GitHub Release for an `aether-agent-cli-v*` tag (created by `release.yml` after building artifacts and publishing the `@aether-agent/cli` npm tarball in its `publish-npm` job). Recommended trigger for the new workflow:

```yaml
on:
  release:
    types: [published]
```

with a first-step guard that proceeds only when `github.event.release.tag_name` starts with `aether-agent-cli-v`. Triggering on every `main` push and diffing `Cargo.toml` is noisier and races with release-plz's own tag push. Triggering on `release.published` (rather than `push.tags`) eliminates the race where the SDK bump runs before the new `@aether-agent/cli` version exists on the registry (SDK `pnpm install` would fail to resolve the new CLI version). Fallback if `release`-event filtering proves fiddly: `workflow_run` on completion of the `Release` workflow (conclusion `success`, for tag `aether-agent-cli-v*`).

**Decision 2 — Bump via a CI-gated release PR with auto-merge; tag only after merge.**
This is the guard against CLI bumps that break TS code. The workflow does **not** push commits or tags to `main` directly. Instead, for each new CLI version it opens a release PR (branch `release-ts/<cli-version>`, title `chore(deps): update TypeScript packages for Aether <cli_version>`), enables GitHub auto-merge on it, and stops. The PR runs the exact same TS validation CI runs on every PR (formatting, `sdk:typecheck`, `sdk:build`, `sdk:test`, `evals:typecheck`, `evals:build`, `evals:test`, frozen-lockfile install). Tags are created by a second trigger in the same workflow file that fires when the bump PR merges to `main` (a `push` to `main` whose head commit matches the bump-commit convention), and only then pushes `aether-sdk-v<new>` and `aether-evals-ts-v<new>`, which in turn fire the existing publish workflows. Consequences:

- Happy path (no breaking changes): release publishes → bump PR opens → checks go green → auto-merge merges → tag step pushes both tags → sdk/evals publish. Zero human action.
- Breaking-change path: release publishes → bump PR opens → TS checks fail → auto-merge cannot merge (branch protection blocks it) → a human is notified by the failing PR, pushes TS fix commits onto the same PR branch (bumping nothing by hand — the version numbers are already right), checks go green → auto-merge proceeds → tags publish. The broken SDK is never published because tags are never cut from a red tree.
- This mirrors the release-plz model contributors already understand (bot opens PR, CI gates it, merge triggers the release), applied to the TS packages. It satisfies the "middle ground" requested in review: all checks run with the version bumps applied, auto-merge carries the routine case through, and tags are the release act.
- Repo-settings prerequisite (one-time): enable auto-merge for the repository and require the TS check suite on `main` via branch protection, so a red bump PR cannot merge and a green one merges without a human clicking. Document the exact settings in the workflow header and the release docs.

**Decision 3 — Reuse the existing tag-gated publish workflows; don't merge publishing into the bump workflow.**
`release-sdk.yml` and `release-evals.yml` already implement trusted publishing (`environment: ci`, `id-token: write`, `--provenance`). The new workflow only creates the bump PR and, post-merge, pushes the two tags. This keeps the provenance/attestation path unchanged, minimizes blast radius, and preserves the ability to manually push a tag for an out-of-band release. Publishing order is guaranteed by npm semantics: the sdk tag is pushed first and the evals tag second (as two explicit ordered steps in the tag job, so an sdk-tag failure blocks the evals-tag step); evals depends on `workspace:^`, which at publish time resolves to the just-bumped local SDK version, and both tags point at the same merged commit so the pair is consistent.

**Decision 4 — Patch-bump policy with caret CLI dep.**
Mirror the precedent set by #483/#503:

- SDK `@aether-agent/cli` dep: `^<new-cli-version>` (exact caret bump, e.g. `^0.9.5` → `^0.9.7`).
- SDK version: `patch+1` (e.g. `0.7.1` → `0.7.2`).
- Evals version: `patch+1` (e.g. `0.4.1` → `0.4.2`). Evals' `@aether-agent/sdk` dep stays `workspace:^` (no change needed — pnpm resolves it, and on publish it becomes `^<sdk-version>`).
- Guard: if a `package.json` version is already higher than the computed bump (human shipped a minor/major), keep the higher version and only fix the CLI dep.

**Decision 5 — A small checked-in script owns the version math, the workflow owns git/tag I/O.**
Put the parsing/bumping logic in a testable Node script (no new dependencies — plain `node`, `fs`, `child_process`) rather than inline bash/`jq`. The workflow calls it in both the open-PR job and as a convergence check in the tag job, then runs the standard validation, commits to the PR branch, and (post-merge) tags.

### Design patterns to employ

- **Idempotent reconciler**: the script reads desired state (CLI version from the release tag, cross-checked against `crates/aether-cli/Cargo.toml`) and current state (both `package.json` files), and no-ops when already converged. Exit code plus a `$GITHUB_OUTPUT` flag (`bumped=true/false`) gates all downstream steps; the open-PR job also no-ops when a bump PR for the same CLI version is already open (checked via `gh pr list --head release-ts/<cli_version>`).
- **PR as the validation gate**: the bump commit exists only on the PR branch until CI is green; nothing is tagged from the branch. The merge commit to `main` is the single atomic unit that carries both `package.json` edits plus the refreshed `pnpm-lock.yaml`.
- **Tag-after-merge**: tags are created only after the bump commit has merged with green checks. The tag job re-verifies the merged tree is converged (script in `--check` mode) and that the head commit message matches the bump convention before pushing any tag.
- **Concurrency guard**: `concurrency: group: ts-release` with `cancel-in-progress: false` so two CLI releases racing can't interleave PRs/tags; the second run sees the first run's merged commit and bumps from there.
- **Allowlisted commit**: the bump commit must contain only `packages/aether-sdk/package.json`, `packages/aether-evals/package.json`, and `pnpm-lock.yaml`. The workflow asserts this with `git status --porcelain` before committing; otherwise it fails the job. This also guarantees the bot commit never touches `crates/**`, so release-plz never opens a spurious Rust release PR in response.
- **Builder pattern for tests** (per repo testing guidelines): test the script through its public CLI entry-point asserting on resulting `package.json` contents using in-memory temp-dir fixtures, not mocks.

### Key technical considerations and trade-offs

1. **Breaking CLI changes are handled by CI, not by version sniffing.** Trying to detect "breaking" from the CLI semver (major bump) is unreliable — a minor/patch CLI release can also rename a flag the SDK shells out to, and a major bump can be TS-inert. The PR gate handles all of these uniformly: any breakage fails checks on the bump PR and blocks auto-merge. No semver-based skip logic is needed; the only special case is pre-releases (see edge table).
2. **Tag-triggered infinite loops.** Pushing `aether-sdk-v*` / `aether-evals-ts-v*` tags triggers the publish workflows, which do not push commits/tags, so no cycle. The new workflow's open-PR job triggers **only** on `aether-agent-cli-v*` releases (never on sdk/evals tags), and its tag job triggers only on `main` pushes whose head commit matches the bump-commit message convention — the tag pushes themselves are tag events, not `main` pushes, so they cannot retrigger it.
3. **npm availability race.** The PR-branch `pnpm install` must resolve `@aether-agent/cli@^<new>` from the registry. `release.published` ordering (after `publish-npm`) plus a short retry loop on `pnpm install` (3 attempts, 60s apart) covers propagation delay. Do **not** publish the SDK from a local tarball of the CLI — the SDK declares a registry dependency.
4. **The `-ts-` infix is load-bearing.** Rust crate `aether-evals` owns the `aether-evals-v*` tag namespace via release-plz (`release-plz.toml` git tag format `{{ package }}-v{{ version }}`). The TS evals package must keep using `aether-evals-ts-v*` (what `release-evals.yml` listens on). Do not "normalize" it.
5. **`workspace:^` semantics.** `packages/aether-evals/package.json` depends on `"@aether-agent/sdk": "workspace:^"`. At `pnpm publish` time this is rewritten to the current SDK version range. Since both bumps land in one merged commit, publishing sdk first then evals yields a consistent pair.
6. **Permissions.** The open-PR job needs `contents: write` (push the PR branch) and `pull-requests: write` (open the PR, enable auto-merge) via the automation app token (`CB_PR_AUTOMATION_APP_ID` / `CB_PR_AUTOMATION_APP_PRIVATE_KEY`, same as `release-plz.yml`), not the default `GITHUB_TOKEN`. The tag job needs `contents: write` (push tags). `id-token: write` is **not** needed in either job (only the existing publish workflows need it). Top-level `permissions: {}` with per-job grants, matching `release-plz.yml` style.
7. **Auto-merge mechanics.** Enable with `gh pr merge --auto --squash` (or `--merge`, matching repo convention — decide during implementation and document it) right after opening the PR. Auto-merge only takes effect once required checks pass; if branch protection is misconfigured the PR simply stays open, which is the safe failure mode. The workflow must not use admin bypass flags.
8. **Backfill.** The repo is currently skewed (CLI `0.9.7` vs SDK dep `^0.9.5`). The first run of the automation (manual `workflow_dispatch` with input `cli_version`, defaulting to current `Cargo.toml` version) should close this gap through the same PR path: bump SDK dep to `^0.9.7`, sdk → `0.7.2`, evals → `0.4.2`, open the PR with auto-merge, and let the merge trigger the tags. Include this as an explicit step-0 validation (run workflow manually once, verify the PR, the merge, the tags, and npm).
9. **Alternative considered and rejected — direct commit + tag push to `main` with no PR.** Fully automatic, but with no place for TS validation to block a bad CLI bump: checks could only run after the tags are already pushed, at which point a broken SDK may already be on npm. The PR + auto-merge design costs one extra merge but buys the breaking-change guard for exactly the mechanical bump case.
10. **Alternative considered and rejected — extending release-plz to own TS versions.** release-plz is Cargo-centric; it cannot version pnpm workspaces, update `pnpm-lock.yaml`, or push npm tags for non-Cargo packages. Teaching it to do so (via `pre-release-hook` hacks) couples two release systems and breaks `release-plz.toml`'s clean Cargo-only contract. A dedicated workflow + script is simpler and independently testable.
11. **Alternative considered and rejected — Changesets.** Adopting Changesets would give full JS-side versioning/changelogs but requires migrating the TS release process, adding a bot, and retraining contributors, for a use case that is currently a pure function of the CLI version. Overkill; revisit only if the SDK needs independent (non-CLI-driven) releases with changelogs.

## Implementation Steps

### Step 1 — Add the bump script `scripts/bump-ts-packages.mjs`

Create a dependency-free Node script (repo runs Node 24 via mise; `.tool-versions` pins `nodejs 24.15.0`):

- Inputs: `--cli-version <semver>` (defaults to parsing `crates/aether-cli/Cargo.toml` `version = "x.y.z"`), `--repo-root` (defaults to script's grandparent), `--check` (dry-run flag for CI/tests: exits non-zero when changes would be made, without writing).
- Behavior:
  1. Read CLI version (strip pre-release suffix handling: if CLI version contains `-`, carry it into the caret range verbatim, e.g. `^1.0.0-beta.1`; document that pre-release CLI versions still open a bump PR — dist honors `announcement_is_prerelease`, and whether the resulting TS packages should publish is governed by the pre-release policy in the edge table).
  2. Read `packages/aether-sdk/package.json` and `packages/aether-evals/package.json`.
  3. Compute: `sdkCliDep = ^<cliVersion>`; `newSdkVersion = max(currentSdk + patch, currentSdk if already greater)`; `newEvalsVersion = max(currentEvals + patch, ...)`. Patch increment = `(major, minor, patch+1)` on valid `x.y.z`; if current version is pre-release, bump patch of the base and drop the suffix comparison (keep it simple; log a warning).
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

Add `scripts/bump-ts-packages.test.mjs` runnable with plain `node --test` (no new harness — `packages/aether-sdk/scripts/e2e.mjs` has no tests today, so plain `node --test` is the lightest consistent choice):

- Cases: (a) stale dep + normal versions → dep updated, both patch-bumped; (b) already converged → no file changes, `bumped=false`; (c) SDK already at a higher minor (e.g. `0.8.0` vs computed `0.7.2`) → version preserved, dep still updated; (d) pre-release CLI version → caret range carries suffix; (e) `--check` mode exits non-zero when changes would be made (used by the tag-job convergence re-check and optionally a CI drift check).
- Fixtures: copy the two real `package.json` shapes into a temp dir per test (builder-style helper `makePackageDir({sdkVersion, evalsVersion, cliDep})`), run the script as a subprocess against the temp dir, assert on resulting JSON contents (state-based, not call-count-based, per repo testing guidelines). Tests assert only the script's public CLI contract.

### Step 3 — Create `.github/workflows/release-ts.yml` (open-PR job + tag-after-merge job)

New workflow file with two event sources and two corresponding jobs:

```yaml
name: Release TS packages
on:
  release:
    types: [published]
  push:
    branches: [main]
    paths:
      - "packages/aether-sdk/package.json"
      - "packages/aether-evals/package.json"
      - "pnpm-lock.yaml"
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
  open-bump-pr:
    if: github.event_name == 'release' || github.event_name == 'workflow_dispatch'
    runs-on: ubuntu-latest
    environment: ci
    permissions:
      contents: write
      pull-requests: write
    steps:
      # 1. Guard: only proceed for aether-agent-cli releases (skip for other
      #    releases when triggered by `release`). Tag-derived version = strip
      #    `aether-agent-cli-v` prefix from github.event.release.tag_name.
      #    Cross-check against crates/aether-cli/Cargo.toml; if they differ,
      #    prefer the tag (the thing actually released) and log a warning.
      #    For workflow_dispatch, use the cli_version input when given.
      # 2. Generate app token (CB_PR_AUTOMATION_APP_ID / _PRIVATE_KEY, owner
      #    contextbridge, repos aether, permission-contents write,
      #    permission-pull-requests write) — same pattern as release-plz.yml.
      # 3. Checkout with fetch-depth 0 + token, mise setup, pnpm setup
      #    (node-version-file .tool-versions).
      # 4. If `gh pr list --head release-ts/<cli_version> --state open` is
      #    non-empty → success exit with summary "bump PR already open".
      # 5. Run node scripts/bump-ts-packages.mjs --cli-version <version> →
      #    $GITHUB_OUTPUT (bumped, sdk_version, evals_version, cli_version).
      #    If bumped=false → success exit with summary "already in sync".
      # 6. pnpm install (with retry loop for registry propagation: 3 attempts,
      #    60s apart) → regenerates pnpm-lock.yaml against the new CLI version.
      # 7. Assert only the 3 allowlisted files changed
      #    (packages/aether-sdk/package.json, packages/aether-evals/package.json,
      #    pnpm-lock.yaml) via git status --porcelain; fail otherwise.
      # 8. Create branch release-ts/<cli_version>, commit with message
      #    "chore(deps): update TypeScript packages for Aether <cli_version>"
      #    (plus the #503-style body: CLI dep, sdk/evals versions), push branch.
      # 9. Open the PR (gh pr create, base main) and enable auto-merge
      #    (gh pr merge --auto --squash). CI on the PR runs the full TS suite,
      #    which is the breaking-change guard: a red PR cannot merge.

  tag-after-merge:
    if: github.event_name == 'push'
    runs-on: ubuntu-latest
    environment: ci
    permissions:
      contents: write
    steps:
      # 1. Guard: proceed only if the head commit message matches
      #    "chore(deps): update TypeScript packages for Aether <semver>".
      #    (Manual edits to the TS package.json files with any other message
      #    never trigger tags.) Extract <semver> from the message.
      # 2. Generate app token (contents write) and checkout with fetch-depth 0.
      # 3. Re-run node scripts/bump-ts-packages.mjs --cli-version <semver>
      #    --check to confirm the merged tree is converged; fail visibly if not.
      # 4. Verify the merged commit contains only the 3 allowlisted files
      #    (git show --name-only); fail otherwise, pushing no tags.
      # 5. Tag sdk: git tag aether-sdk-v<sdk_version from merged package.json>
      #    && git push origin aether-sdk-v<sdk_version> (fires release-sdk.yml).
      # 6. Tag evals: git tag aether-evals-ts-v<evals_version> &&
      #    git push origin aether-evals-ts-v<evals_version>
      #    (fires release-evals.yml). Runs only if step 5 succeeded.
```

Key details:

- Commit message convention: `chore(deps): update TypeScript packages for Aether <cli_version>` — matches #503/#483 so history stays greppable and release-plz ignores it (no `feat:`/`fix:` prefix → no spurious Rust bumps). The tag job keys off this exact convention, so document it as contract, not just style.
- Breaking-change flow: if CI fails on the bump PR, auto-merge stays pending and the workflow posts (or the PR already shows) a summary explaining a human must push TS fixes to the `release-ts/<cli_version>` branch. Once fixed and green, auto-merge merges and the tag job fires normally. No workflow change is needed for this path.
- Step 6 retry: `for i in 1 2 3; do pnpm install && break || sleep 60; done` without `--frozen-lockfile` (this is what rewrites the `@aether-agent/cli` resolution entry, exactly as #503 did by hand). The PR's own CI then runs `pnpm install --frozen-lockfile`, proving the committed lockfile is fresh.

### Step 4 — Backfill the current skew via `workflow_dispatch`

Run the new workflow manually once with `cli_version: 0.9.7` (current `Cargo.toml`):

1. Verify the bump PR opens with SDK dep `^0.9.5` → `^0.9.7`, sdk `0.7.1` → `0.7.2`, evals `0.4.1` → `0.4.2`, plus lockfile, and auto-merge is enabled.
2. Verify the PR merges on green checks with no human action, and the tag job pushes tags `aether-sdk-v0.7.2` and `aether-evals-ts-v0.4.2` with both publish workflows green.
3. Verify on npm: `@aether-agent/sdk@0.7.2` depends on `@aether-agent/cli@^0.9.7`; `@aether-agent/evals@0.4.2` exists.

### Step 5 — Verify end-to-end on the next real CLI release

On the next `chore: release` PR that bumps `aether-agent-cli` (e.g. `0.9.7` → `0.9.8`):

1. Confirm `release-ts.yml` fires on the published `aether-agent-cli-v0.9.8` release and opens the bump PR.
2. Confirm the PR merges via auto-merge with no human action (happy path), or blocks on red checks pending a human TS fix (breaking-change path — the guard working as designed).
3. Confirm both tags land and `release-sdk.yml` / `release-evals.yml` publish the new versions.
4. Confirm no extra release-plz PR was spawned by the bot commit.

### Step 6 — Document the release train

Update the release documentation (repo `README.md` release section if present, or the website docs page covering releases — locate during implementation; at minimum add a header comment in `release-ts.yml` and a paragraph in the follow-up docs):

- Diagram: `release-plz PR merge → aether-agent-cli-v* tag → cargo-dist Release (CLI npm) → release-ts opens bump PR → CI gate (breaking-change guard) → auto-merge → sdk/evals tags → sdk/evals npm publishes`.
- Note the breaking-change path: a red bump PR means the CLI release needs companion TS fixes; push them to the `release-ts/*` branch and auto-merge carries it from there.
- Note the manual escape hatch: pushing `aether-sdk-v*` / `aether-evals-ts-v*` by hand still works for out-of-band fixes; the automation no-ops when already converged (or when a bump PR for that CLI version is already open).
- Note the one-time repo setting: auto-merge enabled + required TS checks on `main` branch protection.

## Testing Plan

### Unit tests required

- `scripts/bump-ts-packages.test.mjs` (new, plain `node --test`):
  - Stale dep bumps dep + patch-bumps both packages.
  - Converged tree → no writes, `bumped=false`.
  - Higher-than-computed SDK version preserved (only dep fixed).
  - Pre-release CLI version carried into caret range.
  - `--check` mode exits non-zero when changes would be made.
  - Malformed `Cargo.toml` (no version) → non-zero exit with a clear error naming the file.
- All tests assert on file contents (state-based) using temp-dir fixtures; no mocks of `fs`.

### Integration tests needed

- Dry-run the script against the real repo in `--check` mode on the current tree: with CLI at `0.9.7` and SDK dep at `^0.9.5`, `--check` must report drift (proving it would have caught the current skew).
- Manual `workflow_dispatch` backfill run (Step 4) is the integration test for the workflow itself: PR creation, auto-merge enablement, full `ci.yml`-parity TS validation on the PR (typecheck/build/test for sdk + evals), merge, and both tag-triggered publishes.
- Deliberately break the guard once in a safe way (e.g. dispatch a backfill against a scratch CLI version whose dep cannot resolve, or temporarily assert a red check blocks merge) to prove tags are not pushed from a red tree.
- Next-release observation (Step 5) is the acceptance test for the `release: published` trigger and ordering (no npm-resolution race).

### Edge cases to verify

| # | Edge case | Expected behavior |
|---|-----------|-------------------|
| 1 | SDK/evals already in sync (re-run, duplicate delivery of `release` event) | No new PR, no tags, green success with "already in sync" summary |
| 2 | Bump PR for the same CLI version already open (release event redelivered, or two CLI releases in quick succession) | Second run detects the open PR and exits green without opening a duplicate |
| 3 | CLI pre-release (e.g. `1.0.0-beta.1`) published | SDK dep becomes `^1.0.0-beta.1`; bump PR opens and publishes on merge like normal — confirm this is desired, else add a guard to skip pre-releases (open question in follow-ups) |
| 4 | Human already bumped SDK minor for the same CLI (e.g. sdk `0.8.0` with stale CLI dep) | Dep updated, SDK version kept at `0.8.0`, evals patch-bumped normally |
| 5 | `@aether-agent/cli` not yet visible on npm when bump job runs | Retry loop on `pnpm install`; job fails visibly after 3 attempts with no PR opened |
| 6 | CLI bump breaks TS code (the reviewer's core concern) | Bump PR checks go red, auto-merge cannot merge, no tags pushed; human pushes TS fixes to the PR branch, checks go green, auto-merge merges, tags publish |
| 7 | Two CLI releases in quick succession | `concurrency.group: ts-release`, `cancel-in-progress: false` serializes runs; second run bumps from the first run's merged commit |
| 8 | Evals publish runs before SDK publish finishes | Evals tag pushed only after SDK tag push step succeeds; evals workflow builds against the same merged commit so its `workspace:^` resolves to the new SDK version |
| 9 | Bot commit accidentally touches `crates/**` | Allowlist guard fails the job before PR creation (open-PR job) and before tagging (tag job) |
| 10 | Tag already exists (e.g. manual tag pushed for same version) | `git push origin <tag>` fails; job surfaces the failure instead of silently skipping — operator deletes or bumps; document in workflow summary |
| 11 | Branch protection / auto-merge misconfigured | PR stays open unmerged (safe failure); workflow summary flags that auto-merge is pending and links the repo-settings prerequisite |

## Files to Modify/Create

| File | Change | Add / Modify / Remove |
|------|--------|----------------------|
| `scripts/bump-ts-packages.mjs` | New dependency-free Node script: derives CLI version, updates SDK CLI dep to `^<cli>`, patch-bumps sdk + evals, idempotent with `bumped` output flag, `--check` dry-run mode | Add |
| `scripts/bump-ts-packages.test.mjs` | Unit tests for the script via plain `node --test` with temp-dir fixtures, state-based assertions | Add |
| `.github/workflows/release-ts.yml` | New workflow with `open-bump-pr` job (fires on `release.published` for `aether-agent-cli-v*` + `workflow_dispatch`; app-token auth; bump → `pnpm install` (retry) → 3-file allowlist guard → push PR branch → open PR → enable auto-merge) and `tag-after-merge` job (fires on `push` to `main` for the bump-commit message convention; convergence re-check + allowlist guard → push sdk tag then evals tag) | Add |
| `packages/aether-sdk/package.json` | Automated at runtime by the workflow (CLI dep `^<new>`, patch version bump); **not** hand-edited in the implementing PR | Modify (runtime) |
| `packages/aether-evals/package.json` | Automated at runtime by the workflow (patch version bump); **not** hand-edited in the implementing PR | Modify (runtime) |
| `pnpm-lock.yaml` | Automated at runtime by the workflow (`pnpm install` refresh of the `@aether-agent/cli` resolution); **not** hand-edited in the implementing PR | Modify (runtime) |
| `.github/workflows/release-sdk.yml` | No change (kept as the publisher; documents the contract the new workflow relies on) | Modify (none — reference only) |
| `.github/workflows/release-evals.yml` | No change (same as above) | Modify (none — reference only) |
| Release docs (`README.md` or website releases page — locate during implementation) | Short paragraph + diagram of the automated train (including the PR gate / breaking-change path), the auto-merge + branch-protection prerequisite, and the manual-tag escape hatch | Modify |

## Additional Notes

### Documentation updates needed

- In-code: header comment in `release-ts.yml` explaining trigger choice (`release.published` after CLI npm publish), the PR-gate rationale (breaking-change guard), auto-merge + branch-protection prerequisite, the `-ts-` infix rationale, and the 3-file commit allowlist.
- User-facing: release-process paragraph (location TBD during implementation) so contributors stop hand-cutting the #503-style bump PRs, know to watch the auto-opened `release-ts/*` PR when they ship a CLI release, and know to push TS fixes onto that branch (rather than opening a competing PR) when it goes red.

### Follow-up tasks that may be spawned

1. **Pre-release policy decision**: confirm whether a pre-release CLI (e.g. `1.0.0-beta.1`) should auto-publish sdk/evals to npm (possibly under a `beta` dist-tag) or skip opening the bump PR. The plan defaults to "same PR path as normal"; if maintainers want skips, add a tag-suffix guard in the open-PR job. This is the one point where a maintainer clarifying answer before implementation would help.
2. **Provenance check on evals ordering**: if npm shows evals resolving a stale SDK range after the first automated run, switch the evals tag step to wait for the SDK npm version to be visible (poll `npm view @aether-agent/sdk versions`) before pushing the evals tag.
3. **Consider `workflow_run` fallback**: if `release.published` proves unreliable for dist-created releases, re-trigger the open-PR job on `workflow_run: Release / completed / success` with tag filtering.
4. **Long-term**: if the SDK ever needs releases decoupled from the CLI (features without a CLI change), adopt Changesets; until then the CLI-driven patch policy in this plan is sufficient and simpler.
5. **Drift check in CI (optional)**: add a lightweight `ci.yml` step running `node scripts/bump-ts-packages.mjs --check` to fail PRs that bump the CLI crate version without the corresponding TS bump — redundant once automation lands, but useful as a safety net during rollout.
