# Aether Evals

Aether Evals is a Rust library that makes writing agent evals easier to express as ordinary `cargo-nextest` tests.

## Table of Contents

<!-- START doctoc generated TOC please keep comment here to allow auto update -->
<!-- DON'T EDIT THIS SECTION, INSTEAD RE-RUN doctoc TO UPDATE -->

- [Quick Start](#quick-start)
- [Test organization](#test-organization)
- [Core API](#core-api)
- [Dockerized evals](#dockerized-evals)
- [Assertions](#assertions)
- [Failure output and debugging](#failure-output-and-debugging)
- [Git-backed evals](#git-backed-evals)
- [Development](#development)

<!-- END doctoc generated TOC -->

## Quick Start

```rust
use aether_evals::{FakeAgent, Task, Workspace};

#[tokio::test]
async fn hello_world_test() -> Result<(), aether_evals::EvalRunError> {
    let run = Task::new("Write 'Hello, World!' to hello.txt", Workspace::empty()?)
        .run(&FakeAgent::writes_file("hello.txt", "Hello, World!"))
        .await?;

    assert!(run.workspace().join("hello.txt").exists(), "{}", run.failure_context());
    Ok(())
}
```

Run evals with nextest:

```bash
cargo nextest run -p aether-evals --all-features
cargo nextest run --profile ci --all-features --workspace
```

The CI profile writes JUnit XML to `target/nextest/ci/junit.xml`.

## Test organization

Aether Evals' fake-agent coverage should use normal test names. Reserve `_eval` suffixes for real provider-backed evals selected by the repository `evals` nextest group.

## Core API

- `Task::new(prompt, workspace)` describes the problem to solve and the starting workspace.
- `Task::run(&agent)` runs an agent on one task and returns a `TaskRun`.
- `DockerAgent::new(image, command)` runs an in-container command whose stdout is newline-delimited `AgentMessage` JSON.
- `default_eval_env_vars()` forwards provider credentials and Aether-related environment into Docker while setting `AETHER_HOME=/root/.aether`.
- `Workspace::empty()` creates an isolated temp directory.
- `Workspace::from_dir(path)` copies fixture directory contents into a temp directory.
- `Workspace::from_git_repo(GitRepoSpec { url, start_commit, gold_commit, subdir })` clones and checks out a git repository.
- `TaskRun` exposes the prompt, final workspace, transcript, and failure context.
- `Transcript` exposes agent messages and tool-call helpers.

## Dockerized evals

Aether Evals mounts the workspace root at `/workspace` and runs the command from the effective eval cwd. The command receives:

- `AETHER_EVAL_WRAPPED_TASK_PROMPT` — the prompt to send to the agent.
- `AETHER_EVAL_WORKSPACE_ROOT=/workspace` — the mounted workspace root.
- `AETHER_EVAL_CWD` — the command's effective cwd inside the workspace.

Stdout is reserved for `AgentMessage` JSON lines and must end with a terminal message such as `{"type":"done"}`. Diagnostic logging should go to stderr.

```rust
use aether_evals::{
    CONTAINER_AETHER_HOME, DockerAgent, DockerImage, GitRepoSpec, Task, Workspace, default_eval_env_vars,
};

let agent = DockerAgent::new(
    DockerImage::new("aether-sandbox", "latest"),
    vec!["/usr/local/bin/aether-eval-agent".to_string()],
)
.with_env_vars(default_eval_env_vars())
.with_ephemeral_mount(CONTAINER_AETHER_HOME);

let run = Task::new(
    "fix the failing test",
    Workspace::from_git_repo(GitRepoSpec {
        url: "https://github.com/example/repo".to_string(),
        start_commit: "start_sha".to_string(),
        gold_commit: "gold_sha".to_string(),
        subdir: None,
    })?,
)
.run(&agent)
.await?;
```

A wrapper script for evaluating Aether itself can delegate to headless JSON output:

```sh
#!/bin/sh
exec aether headless \
  --output json \
  "$AETHER_EVAL_WRAPPED_TASK_PROMPT"
```

A declarative eval:

```json
{
  "agent": {
    "command": ["node", "/app/dist/eval-agent.js"]
  }
}
```

## Assertions

Use normal Rust assertions over the returned `TaskRun`, its `Transcript`, and files on disk:

```rust
let run = Task::new(prompt, Workspace::empty()?).run(&agent).await?;

assert!(run.transcript().tool_called("write_file"), "{}", run.failure_context());
assert_eq!(run.transcript().tool_call_count("bash"), 1, "{}", run.failure_context());
assert_eq!(std::fs::read_to_string(run.workspace().join("output.txt"))?, "expected text\n");
```

Aether Evals also exports small `#[track_caller]` helpers for common tool assertions:

```rust
aether_evals::assert_tool_called(&run, "write_file");
aether_evals::assert_tool_call_count(&run, "bash", 1);
aether_evals::assert_tool_call_with_args(&run, "write_file", &serde_json::json!({ "path": "output.txt" }));
```

LLM judging runs through declarative eval files: add a rubric `judge` block to `expect` in a `*.eval.json` file. Each criterion is scored 0.0–1.0 by the judge model, weighted into an overall score, and blocking criteria below their threshold fail the eval. See the [evals documentation](https://aether-agent.io/aether/running/evals/) for the file format.

Judge faults — transient LLM stream failures, unparseable responses, criterion sets that don't match the rubric — are recorded as failures on the eval's outcome rather than conflated with a "the agent didn't do it" verdict.

## Failure output and debugging

`TaskRun::failure_context()` returns deterministic plain text suitable for assertion messages, nextest output, and JUnit failure bodies. It includes prompt, workspace path, agent messages, and git diff stats when available.

Temp directories are deleted when the task run is dropped.

## Git-backed evals

```rust
use aether_evals::GitRepoSpec;

let run = Task::new(
    "Make the test pass",
    Workspace::from_git_repo(GitRepoSpec {
        url: "https://github.com/example/repo".to_string(),
        start_commit: "start_sha".to_string(),
        gold_commit: "gold_sha".to_string(),
        subdir: Some("packages/api".into()),
    })?,
)
.run(&agent)
.await?;

assert!(run.transcript().tool_called("bash"), "{}", run.failure_context());
```

## Development

```bash
cargo test -p aether-evals
```
