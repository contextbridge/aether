# Example: declarative eval

A self-contained example of an `aether eval` spec. It defines its own agent
(`demo`) and system prompt so it runs the same way regardless of your global
or repo settings.

## Run it

```sh
# from the repo root (uses the example's own .aether/settings.json via -C)
cargo run -p aether-agent-cli -- eval -C examples/eval-demo

# or, if you have `aether` installed:
aether eval -C examples/eval-demo
```

The `demo` agent uses `zai:glm-5.1`, so set `ZAI_API_KEY` first (or edit
`.aether/settings.json` to point `model` at any provider you have credentials
for, e.g. `anthropic:claude-sonnet-4-5` or `openrouter:...`).

Expected output:

```
[PASS] examples/eval-demo/.aether/evals/greeting.json
  ✓ tool coding__write_file — called
  ✓ file greeting.txt — contains "Hello, Aether!"
  ✓ run `test -f greeting.txt` — exit 0
```

`aether eval` exits non-zero if any check fails, so it drops straight into CI.

## What the spec shows

`.aether/evals/greeting.json` exercises the three **deterministic** assertion
kinds, so it passes reproducibly whenever the agent does the task:

| `expect` entry | Checks |
|---|---|
| `{ "tool": "coding__write_file" }` | the agent **called** that tool (trajectory) |
| `{ "file": "greeting.txt", "contains": "..." }` | resulting **file contents** (outcome) |
| `{ "run": "test -f greeting.txt", "exitCode": 0 }` | a **shell command** passes in the workspace (outcome) |

### LLM-judge checks

The fourth kind, `{ "judge": "..." }`, asks an LLM to rule on a
natural-language criterion (style, architecture, "did it do the right thing").
Judge results are **probabilistic** — the judge sees the agent's transcript and
may be strict about a messy process even when the outcome is correct — so they
are best paired with deterministic checks rather than used alone. A judged
variant lives in `greeting-judged.json` (kept out of `.aether/evals/` so the
default run stays deterministic); try it explicitly:

```sh
cargo run -p aether-agent-cli -- eval -C examples/eval-demo greeting-judged.json
```

## Authoring your own

Drop more `*.json` files in `.aether/evals/` and run `aether eval` (no args) to
run them all. The envelope is `agent`/`model`, `workspace`, `prompt`, `expect[]`:

- **workspace**: omit for empty; `{ "files": { "path": "contents" } }` for
  fixtures; `{ "dir": "..." }` to copy a local directory; or
  `{ "git": { "url", "start", "gold" } }` to clone a repo at a commit
  (`gold` = the human-solution commit, used as the judge's reference diff).
- **prompt**: a string, an array of lines, or `{ "file": "task.md" }`.
- **environment** (optional): `{ "dockerfile": "..." }` or `{ "image": "..." }`
  to run the whole eval in an isolated container (the image must contain the
  `aether` binary).

Tip: give eval agents clean, workspace-relative prompts. Prompts that reference
absolute host paths can steer the agent to write outside the eval workspace.
