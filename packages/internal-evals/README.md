<!-- START doctoc generated TOC please keep comment here to allow auto update -->
<!-- DON'T EDIT THIS SECTION, INSTEAD RE-RUN doctoc TO UPDATE -->

- [Aether evals](#aether-evals)
  - [Running evals](#running-evals)
  - [Docker and agent configuration](#docker-and-agent-configuration)
  - [Adding evals](#adding-evals)

<!-- END doctoc generated TOC please keep comment here to allow auto update -->

# Aether evals

Contains Dockerized evals for Aether agents.

There are two ways to write evals:

- **Declarative JSON, no Rust** — author one `*.eval.json` file per scenario and run them with `aether eval`. Best for users evaluating the agent against their own repo. See the [evals documentation](https://aether-agent.io/aether/running/evals/) for the eval file format and CLI usage; a complete example is in [`examples/edit-notes.eval.json`](examples/edit-notes.eval.json) with a sandbox image in [`examples/Dockerfile`](examples/Dockerfile).
- **Rust tests** — `#[tokio::test]`s in `packages/internal-evals/tests` using the `aether-evals` harness directly. Best for evals maintained in this repo. See [Running evals](#running-evals).

## Running evals

Normal nextest runs compile these evals but do not execute them because `.config/nextest.toml` excludes the entire `internal-evals` package by default:

```bash
just test -p internal-evals
```

Run the eval group explicitly:

```bash
just evals
```

List evals without running them:

```bash
just evals-list
```

## Docker and agent configuration

All evals in this repo run in the `aether-sandbox:latest` image, built from [`examples/Dockerfile`](examples/Dockerfile). `just evals` builds it (via `just build-sandbox`) before running anything, so a fresh checkout works without manual setup. The example eval also points `docker.file` at the same Dockerfile, so running it standalone with `aether eval` builds the identical image. The evals crate loads repo `.aether/settings.json` and passes it to Dockerized Aether ACP with `--settings-json`.

Set `AETHER_EVAL_AGENT` to pass `--agent` through to `aether acp`:

```bash
AETHER_EVAL_AGENT="Fast" just evals
```

## Adding evals

- Put real LLM scenarios in `packages/internal-evals/tests` or add declarative `*.eval.json` files under `packages/internal-evals/examples`; shared Rust setup code lives in `tests/common`.
- `just evals` runs tests ending in `_eval` with nextest's default filter disabled and the `evals` group selected; keep that suffix for real provider-backed eval tests.
- Prefer `aether_evals::DockerAetherAgent` with settings-driven MCP wiring over fake agents.
- Run the agent with `aether_evals::run_eval(&agent, prompt, workspace).await?`.
- Assert Aether namespaced MCP tool names, such as `coding__read_file` and `coding__edit_file`, with `EvalReport` helpers.
- Prefer direct filesystem assertions over shell commands for file outcomes.
