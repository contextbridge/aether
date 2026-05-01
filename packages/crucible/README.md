# Crucible

Crucible is a Rust library that makes writing agent evals easier to express as ordinary `cargo-nextest` tests.

## Table of Contents

<!-- START doctoc generated TOC please keep comment here to allow auto update -->
<!-- DON'T EDIT THIS SECTION, INSTEAD RE-RUN doctoc TO UPDATE -->

- [Quick Start](#quick-start)
- [Test organization](#test-organization)
- [Core API](#core-api)
- [Assertions](#assertions)
- [Failure output and debugging](#failure-output-and-debugging)
- [Git-backed evals](#git-backed-evals)
- [Development](#development)

<!-- END doctoc generated TOC -->

## Quick Start

```rust
use crucible::{FakeAgent, Workspace, run_eval};

#[tokio::test]
async fn hello_world_test() -> Result<(), crucible::EvalRunError> {
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
cargo nextest run -p crucible --all-features
cargo nextest run --profile ci --all-features --workspace
```

The CI profile writes JUnit XML to `target/nextest/ci/junit.xml`.

## Test organization

Crucible's fake-agent coverage should use normal test names. Reserve `_eval` suffixes for real provider-backed evals selected by the repository `evals` nextest group.

## Core API

- `run_eval(&agent, prompt, workspace)` runs one eval and returns an `EvalReport`.
- `AetherAgent::with_system_prompt(prompt)` configures the agent's system prompt; accepts any `crucible::Prompt`, including file/glob prompts that get `!`cmd`` shell interpolation.
- `Workspace::empty()` creates an isolated temp directory.
- `Workspace::from_dir(path)` copies fixture directory contents into a temp directory.
- `Workspace::from_git_repo(GitRepoSpec { url, start_commit, gold_commit, subdir })` clones and checks out a git repository.
- `EvalReport` exposes the prompt, workspace, agent messages, tool-call helpers, and git diff summaries.

## Assertions

Use normal Rust assertions over the returned `EvalReport` and files on disk:

```rust
let report = run_eval(&agent, prompt, Workspace::empty()?).await?;

assert!(report.tool_called("write_file"), "{}", report.failure_context());
assert_eq!(report.tool_call_count("bash"), 1, "{}", report.failure_context());
assert_eq!(std::fs::read_to_string(report.path("output.txt"))?, "expected text\n");
```

Crucible also exports small `#[track_caller]` helpers for common tool assertions:

```rust
crucible::assert_tool_called(&report, "write_file");
crucible::assert_tool_call_count(&report, "bash", 1);
crucible::assert_tool_call_with_args(&report, "write_file", &serde_json::json!({ "path": "output.txt" }));
```

LLM judging is explicit on the report:

```rust
use crucible::BinaryMetric;
use schemars::schema_for;

let judgment = report
    .judge(&judge_llm, |ctx| {
        format!(
            "Did the agent satisfy the task?\n\nTask: {}\n\nRespond as JSON matching this schema:\n{}",
            ctx.original_prompt,
            serde_json::to_string_pretty(&schema_for!(BinaryMetric)).unwrap()
        )
    })
    .await
    .expect("judge failed to produce a result");

assert!(judgment.passed(), "{}\n\n{}", judgment.reason(), report.failure_context());
```

`judge` returns `Err(JudgeError::Stream)` for transient LLM stream failures and `Err(JudgeError::InvalidJson)` when the judge response is not parseable — these are system errors that should fail the test loudly rather than be conflated with a "the agent didn't do it" verdict.

## Failure output and debugging

`EvalReport::failure_context()` returns deterministic plain text suitable for assertion messages, nextest output, and JUnit failure bodies. It includes prompt, workspace path, agent messages, and git diff stats when available.

Temp directories are deleted when the report is dropped.

## Git-backed evals

```rust
use crucible::GitRepoSpec;

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
just test -p crucible
just test-ci -p crucible
just lint -p crucible
just doc-check -p crucible
```
