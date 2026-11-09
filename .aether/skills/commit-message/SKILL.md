---
name: commit-message
description: Generate conventional commit messages for this repository. Use when the user asks to commit changes, create a commit, or generate a commit message.
allowed-tools:
  - Bash(git status:*)
  - Bash(git diff:*)
  - Bash(git log:*)
  - Bash(git add:*)
  - Bash(git commit:*)
  - Read
  - AskUserQuestion
---

# Commit Message Generator

Generate commit messages following this repository's conventional commit standards.

## Conventions

- **Prefixes**: `feat`, `fix`, `chore`, `refactor`, `docs`, `test`
- **Scopes** (optional): `(gateway)`, `(frontend)`, `(deps)`, `(deps-dev)`, `(infra)`
- **Format**: `<type>(<scope>): <subject>` or `<type>: <subject>`
- **Subject**: lowercase after prefix, imperative mood, no trailing period, max 72 chars
- **Body**: optional, separated by blank line, explains "why" not just "what"
- **Linear issues**: include `CON-XXX` in subject when relevant

## Examples

```
feat: add user authentication endpoint
fix(gateway): handle connection timeout gracefully
chore(deps): bump sentry-rust from 0.31 to 0.32
refactor: extract validation logic into separate module
feat(frontend): expose client connection details on dashboard (CON-244)
```

## Workflow

### Step 1: Analyze Changes

Start with the overview; full diffs can be very large:

```bash
git status --short
git diff --cached --stat
git diff --stat
git log --oneline -5
```

Then view diffs per file or area as needed (e.g. `git diff -- crates/foo/`, `git diff --cached -- path/to/file.rs`) instead of the whole diff at once.

### Step 2: Determine Commit Type

Based on the changes:

| Change Type | Prefix |
|-------------|--------|
| New feature or capability | `feat` |
| Bug fix | `fix` |
| Dependencies, configs, build | `chore` |
| Code restructuring (no behavior change) | `refactor` |
| Documentation only | `docs` |
| Tests only | `test` |

### Step 3: Determine Scope (Optional)

Infer from changed file paths:

| Path Pattern | Scope |
|--------------|-------|
| `gateway/` | `(gateway)` |
| `frontend/` | `(frontend)` |
| `infra/`, `terraform/` | `(infra)` |
| `Cargo.toml`, `package.json` (deps only) | `(deps)` |
| Dev dependencies | `(deps-dev)` |

Omit scope if changes span multiple areas or scope isn't clear.

### Step 4: Write Subject Line

- Start with lowercase after the prefix
- Use imperative mood: "add" not "added", "fix" not "fixes"
- Be specific but concise
- No trailing period
- Keep under 72 characters

### Step 5: Add Body (If Needed)

Add a body when:
- The "why" isn't obvious from the subject
- There's important context reviewers need
- The change is complex or has side effects

Body format:
- Blank line after subject
- Wrap at 72 characters
- Explain motivation and contrast with previous behavior

### Step 6: Execute Commit

Stage and commit the changes:

```bash
git add -A && git commit -m "$(cat <<'EOF'
<type>(<scope>): <subject>

<body if needed>
EOF
)"
```
