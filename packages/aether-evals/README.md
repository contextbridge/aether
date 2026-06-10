# Aether Evals

Aether Evals is a Rust library that makes writing agent evals easier to express as ordinary `cargo-nextest` tests.

## Table of Contents

<!-- START doctoc generated TOC please keep comment here to allow auto update -->
<!-- DON'T EDIT THIS SECTION, INSTEAD RE-RUN doctoc TO UPDATE -->

- [Quick Start](#quick-start)
- [Test organization](#test-organization)
- [Core API](#core-api)
- [Dockerized Aether evals](#dockerized-aether-evals)
- [Assertions](#assertions)
- [Failure output and debugging](#failure-output-and-debugging)
- [Git-backed evals](#git-backed-evals)
- [Development](#development)

<!-- END doctoc generated TOC -->

## Quick Start

```rust
use aether_evals::{FakeAgent, Workspace, run_eval};

#[tokio::test]
async fn hello_world_test() -> Result<(), aether_evals::EvalRunError> {
    let report = run_eval(
        &FakeAgent::writes_file("hello.txt", "Hello, World!"),
        "Write 'Hello, World!' to hello.txt",
        Workspace::empty()?,
    )
    .await?;

    assert!(report.path("hello.txt").exists(), "{}", report.failure_context());
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

- `run_eval(&agent, prompt, workspace)` runs one eval and returns an `EvalReport`.
- `DockerAetherAgent::new(image)` runs `aether headless --output json` in a fresh Testcontainers container for each eval run.
- `Workspace::empty()` creates an isolated temp directory.
- `Workspace::from_dir(path)` copies fixture directory contents into a temp directory.
- `Workspace::from_git_repo(GitRepoSpec { url, start_commit, gold_commit, subdir })` clones and checks out a git repository.
- `EvalReport` exposes the prompt, workspace, agent messages, tool-call helpers, and git diff summaries.

## Dockerized Aether evals

Use `DockerAetherAgent` when an eval should exercise the real Aether CLI in an isolated container. Aether Evals mounts the workspace root at `/workspace`; git-backed workspaces with `GitRepoSpec::subdir` mount the repository root so `.git` remains available while headless runs from the matching relative cwd under `/workspace`.

```rust
use aether_evals::{DockerAetherAgent, DockerImage, GitRepoSpec, Workspace, run_eval};
use aether_project::AetherSettings;
use std::path::Path;

let settings = AetherSettings::load_file_for_export(Path::new(".aether/settings.json"))?;
let agent = DockerAetherAgent::new(DockerImage::new("aether-sandbox", "latest"))
    .with_settings(settings)
    .with_agent("Fast");

let report = run_eval(
    &agent,
    "fix the failing test",
    Workspace::from_git_repo(GitRepoSpec {
        url: "https://github.com/example/repo".to_string(),
        start_commit: "start_sha".to_string(),
        gold_commit: "gold_sha".to_string(),
        subdir: None,
    })?,
)
.await?;
```

By default the Docker agent forwards provider API key env vars, `OLLAMA_HOST`, and `AETHER_*` except host `AETHER_HOME`. Each run gets a fresh temporary container `AETHER_HOME`; configured settings are passed with `--settings-json`, and `.with_agent(...)` passes `--agent`.

## Assertions

Use normal Rust assertions over the returned `EvalReport` and files on disk:

```rust
let report = run_eval(&agent, prompt, Workspace::empty()?).await?;

assert!(report.tool_called("write_file"), "{}", report.failure_context());
assert_eq!(report.tool_call_count("bash"), 1, "{}", report.failure_context());
assert_eq!(std::fs::read_to_string(report.path("output.txt"))?, "expected text\n");
```

Aether Evals also exports small `#[track_caller]` helpers for common tool assertions:

```rust
aether_evals::assert_tool_called(&report, "write_file");
aether_evals::assert_tool_call_count(&report, "bash", 1);
aether_evals::assert_tool_call_with_args(&report, "write_file", &serde_json::json!({ "path": "output.txt" }));
```

LLM judging runs through declarative eval files: add a rubric `judge` block to `expect` in a `*.eval.json` file. Each criterion is scored 0.0–1.0 by the judge model, weighted into an overall score, and blocking criteria below their threshold fail the eval. See the [evals documentation](https://aether-agent.io/aether/running/evals/) for the file format.

Judge faults — transient LLM stream failures, unparseable responses, criterion sets that don't match the rubric — are recorded as failures on the eval's outcome rather than conflated with a "the agent didn't do it" verdict.

## Failure output and debugging

`EvalReport::failure_context()` returns deterministic plain text suitable for assertion messages, nextest output, and JUnit failure bodies. It includes prompt, workspace path, agent messages, and git diff stats when available.

Temp directories are deleted when the report is dropped.

## Git-backed evals

```rust
use aether_evals::GitRepoSpec;

let report = run_eval(
    &agent,
    "Make the test pass",
    Workspace::from_git_repo(GitRepoSpec {
        url: "https://github.com/example/repo".to_string(),
        start_commit: "start_sha".to_string(),
        gold_commit: "gold_sha".to_string(),
        subdir: Some("packages/api".into()),
    })?,
)
.await?;

assert!(report.tool_called("bash"), "{}", report.failure_context());
```

For git-backed evals, `EvalReport` exposes the agent diff from `HEAD` and the reference diff from `start_commit..gold_commit` when git commands succeed.

## Development

```bash
just test -p aether-evals
just test-ci -p aether-evals
just lint -p aether-evals
just doc-check -p aether-evals
```
