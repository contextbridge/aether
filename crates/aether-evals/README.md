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
- [Debugging failed evals](#debugging-failed-evals)
- [Git-backed evals](#git-backed-evals)
- [Development](#development)

<!-- END doctoc generated TOC -->

## Quick Start

```rust
use aether_evals::{Agent, FakeAgent, Task, Transcript, Workspace};

#[tokio::test]
async fn hello_world_test() -> Result<(), aether_evals::WorkspaceError> {
    let workspace = Workspace::empty()?;
    let prompt = "Write 'Hello, World!' to hello.txt";
    let agent = FakeAgent::writes_file("hello.txt", "Hello, World!").with_workspace(workspace.path());
    let transcript = Transcript::from_stream(agent.run(Task::new(prompt))).await.unwrap();

    assert!(!transcript.messages().is_empty());
    assert!(workspace.join("hello.txt").exists());
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

- `Task::new(prompt)` describes the problem to solve.
- `Agent::run(task)` streams `AgentMessage`s from an agent's configured execution environment.
- `Transcript::from_stream(agent.run(task))` collects the stream into a `Transcript`.
- `Transcript::default()` plus `transcript.add(message)` supports observing each streamed message while building the transcript manually.
- `Container::builder(image).start(&workspace)` starts a caller-owned Docker container with the workspace mounted at `/workspace`.
- `DockerAgent::new(container, command)` runs an in-container command whose stdout is newline-delimited `AgentMessage` JSON.
- `Container::exec_shell(script)` runs follow-up assertions or test commands in the same caller-owned container.
- `default_eval_env_vars()` forwards provider credentials and Aether-related environment into Docker while setting `AETHER_HOME=/root/.aether`.
- `Workspace::empty()` creates an isolated temp directory.
- `Workspace::from_dir(path)` copies fixture directory contents into a temp directory.
- `Workspace::from_git_repo(GitRepoSpec { url, start_commit, gold_commit, subdir })` clones and checks out a git repository.
- `Transcript` exposes collected agent messages, token usage, and tool-call helpers.

## Dockerized evals

Aether Evals mounts the workspace root at `/workspace` and runs the command from the effective eval cwd. It does not wrap or alter prompts; the command receives the caller-provided prompt verbatim:

- `AETHER_EVAL_TASK_PROMPT` — the caller-provided prompt to send to the agent unchanged.
- `AETHER_EVAL_WORKSPACE_ROOT=/workspace` — the mounted workspace root.
- `AETHER_EVAL_CWD` — the command's effective cwd inside the workspace.

Stdout is reserved for `AgentMessage` JSON lines and must end with a terminal message such as `{"type":"done"}`. Diagnostic logging should go to stderr.

```rust
use aether_evals::{
    Agent, CONTAINER_AETHER_HOME, Container, DockerAgent, GitRepoSpec, Image, Task, Transcript, Workspace,
    default_eval_env_vars,
};

let workspace = Workspace::from_git_repo(GitRepoSpec {
    url: "https://github.com/example/repo".to_string(),
    start_commit: "start_sha".to_string(),
    gold_commit: "gold_sha".to_string(),
    subdir: None,
})?;
let container = Container::builder(Image::new("aether-sandbox", "latest"))
    .with_env_vars(default_eval_env_vars())
    .with_ephemeral_mount(CONTAINER_AETHER_HOME)
    .start(&workspace)
    .await?;
let agent = DockerAgent::new(container.clone(), vec!["/usr/local/bin/aether-eval-agent".to_string()]);

let prompt = "fix the failing test";
let transcript = Transcript::from_stream(agent.run(Task::new(prompt))).await?;
let tests = container.exec_shell("cargo test").await?;
assert_eq!(tests.exit_code, 0, "stdout:\n{}\nstderr:\n{}", tests.stdout, tests.stderr);
assert!(!transcript.messages().is_empty());
```

Run follow-up `container.exec_shell(...)` assertions before dropping the caller-owned container and workspace.

A wrapper script for evaluating Aether itself can delegate to headless JSON output:

```sh
#!/bin/sh
exec aether headless \
  --output json \
  "$AETHER_EVAL_TASK_PROMPT"
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

Use normal Rust assertions over the returned `Transcript` and files on disk:

```rust
let workspace = Workspace::empty()?;
let transcript = Transcript::from_stream(agent.run(Task::new(prompt))).await?;

assert!(transcript.tool_called("write_file"));
assert_eq!(transcript.tool_call_count("bash"), 1);
assert_eq!(std::fs::read_to_string(workspace.join("output.txt"))?, "expected text\n");
```

Aether Evals also exports small `#[track_caller]` helpers for common tool assertions:

```rust
aether_evals::assert_tool_called(&transcript, "write_file");
aether_evals::assert_tool_call_count(&transcript, "bash", 1);
aether_evals::assert_tool_call_with_args(&transcript, "write_file", &serde_json::json!({ "path": "output.txt" }));
```

LLM judging runs through declarative eval files: add a rubric `judge` block to `expect` in a `*.eval.json` file. Each criterion is scored 0.0–1.0 by the judge model, weighted into an overall score, and blocking criteria below their threshold fail the eval. See the [evals documentation](https://aether-agent.io/aether/running/evals/) for the file format.

Judge faults — transient LLM stream failures, unparseable responses, criterion sets that don't match the rubric — are recorded as failures on the eval's outcome rather than conflated with a "the agent didn't do it" verdict.

## Debugging failed evals

`TranscriptError` preserves the partial transcript if an agent stream returns an error. Use `error.transcript().messages()` or `error.into_parts()` to inspect messages that were emitted before the failure.

`Workspace` removes its temporary directory when dropped. To retain files for debugging, call `workspace.persist()` and remove the returned path manually when finished.

For git-backed workspaces, `workspace.capture_git_diffs()` returns the agent-produced diff and the reference diff when available.

## Git-backed evals

```rust
use aether_evals::{Agent, GitRepoSpec, Task, Transcript, Workspace};

let workspace = Workspace::from_git_repo(GitRepoSpec {
    url: "https://github.com/example/repo".to_string(),
    start_commit: "start_sha".to_string(),
    gold_commit: "gold_sha".to_string(),
    subdir: Some("packages/api".into()),
})?;
let prompt = "Make the test pass";
let transcript = Transcript::from_stream(agent.run(Task::new(prompt))).await?;

assert!(transcript.tool_called("bash"));
```

## Development

```bash
cargo test -p aether-evals
```
