# Aether evals

Contains Dockerized evals for Aether agents.

## Running evals

Normal nextest runs compile these evals but do not execute them because `.config/nextest.toml` excludes `aether-evals` tests ending in `_eval` by default:

```bash
just test -p aether-evals
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

The eval harness runs `aether headless --output json` inside one fresh Docker container per scenario. It uses `AETHER_EVAL_DOCKER_IMAGE` when set and defaults to `aether-sandbox:latest`; `just evals` builds the default image only when it is missing. Set `AETHER_EVAL_REBUILD_DOCKER=1` or run `just build-sandbox` to refresh the local image after changing sandboxed Aether code. The evals crate loads the repo `.aether/settings.json` and passes it to Dockerized headless with `--settings-json`.

```bash
AETHER_EVAL_DOCKER_IMAGE="ghcr.io/org/aether:sha" just evals
```

Set `AETHER_EVAL_AGENT` to pass `--agent` through to `aether headless`:

```bash
AETHER_EVAL_AGENT="Fast" just evals
```

## Adding evals

- Put reusable setup code in `packages/evals/src`.
- Put eval scenarios in `packages/evals/tests`.
- Suffix real LLM test names with `_eval`; nextest excludes those tests from normal runs and assigns them to the `evals` group for serial execution.
- Prefer `crucible::DockerAetherAgent` and settings-driven MCP wiring over fake agents.
- Run the agent with `crucible::run_eval(&agent, prompt, workspace).await?`.
- Assert Aether namespaced MCP tool names, such as `coding__read_file` and `coding__edit_file`, with `EvalReport` helpers.
- Prefer direct filesystem assertions over shell commands for file outcomes.
