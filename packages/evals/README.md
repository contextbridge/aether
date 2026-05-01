# Aether evals

Contains evals for Aether agents.

## Running evals

Normal nextest runs compile these evals but do not execute them because `.config/nextest.toml` excludes `aether-evals` tests ending in `_eval` by default:

```bash
just test -p aether-evals
```

Run the eval group explicitly with:

```bash
just evals
```

List evals without running them:

```bash
just evals-list
```

## Model configuration

The eval harness requires `AETHER_EVAL_MODEL`; it fails before starting an agent when the variable is unset.

Examples:

```bash
AETHER_EVAL_MODEL="anthropic:claude-sonnet-4-5-20250929" just evals
AETHER_EVAL_MODEL="openai:gpt-4.1" just evals edit_file
```

Set the provider credentials required by the selected model, such as `ANTHROPIC_API_KEY` or `OPENAI_API_KEY`.

## Adding evals

- Put reusable setup code in `packages/evals/src`.
- Put eval scenarios in `packages/evals/tests`.
- Suffix real LLM test names with `_eval`; nextest excludes those tests from normal runs and assigns them to the `evals` group for serial execution.
- Prefer `crucible::AetherAgent` and product MCP wiring over fake agents.
- Run the agent with `crucible::run_eval(&agent, prompt, workspace).await?`.
- Assert Aether namespaced MCP tool names, such as `coding__read_file` and `coding__edit_file`, with `EvalReport` helpers.
- Prefer direct filesystem assertions over shell commands for file outcomes.
