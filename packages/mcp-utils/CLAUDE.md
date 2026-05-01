# CLAUDE.md - mcp-utils

## Writing Evals

MCP utility evals are ordinary Rust tests that use Crucible helpers. Nextest discovers, filters, schedules, and reports them.

### Eval Directory Structure

Fixtures and prompts can still live on disk:

```
tests/evals/<eval_name>/
├── prompt.md
└── src/
    └── fixture files
```

Load those files from a `#[tokio::test]`, call `run_eval`, then assert on the returned report and filesystem state.

### Creating a New Eval

```rust
use crucible::{Workspace, run_eval};

#[tokio::test]
async fn edit_single_file_eval() -> Result<(), crucible::EvalRunError> {
    let tests_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests");
    let prompt = std::fs::read_to_string(tests_dir.join("evals/edit_single_file/prompt.md"))
        .expect("eval prompt should be readable");
    let workspace = Workspace::from_dir(tests_dir.join("evals/edit_single_file/src"))?;

    let report = run_eval(&runner(), prompt, workspace).await?;

    let contents = std::fs::read_to_string(report.path("src/main.rs")).expect("output file should be readable");
    assert!(contents.contains("Hello, World!"), "{}", report.failure_context());
    assert!(report.tool_called("edit_file"), "{}", report.failure_context());
    Ok(())
}
```

Use test names ending in `_eval` only for real provider-backed evals selected by the repository `evals` nextest group. Fake-agent coverage is normal unit or integration testing and should use normal test names.

### Assertion Style

Prefer normal Rust assertions:

- `assert!(report.path("out.txt").exists(), "{}", report.failure_context())` verifies a file or directory exists.
- `assert!(contents.contains("expected text"), "{}", report.failure_context())` checks file contents.
- `assert!(report.tool_called("edit_file"), "{}", report.failure_context())` checks that a tool was called.
- `assert_eq!(report.tool_call_count("edit_file"), 1, "{}", report.failure_context())` checks exact call count.
- `report.tool_calls("edit_file")` returns calls with `arguments` and `arguments_json()` for detailed inspection.
- Use `std::process` or `tokio::process` directly when a shell command is clearer than a state assertion.

### Workspace Options

- `Workspace::empty()` creates a fresh empty temp directory.
- `Workspace::from_dir(path)` copies fixture contents into a fresh temp directory.
- `Workspace::from_git_repo(url, start_sha, gold_sha, subdir)` clones a repository and checks out the start commit.


### LLM Judge Helper

```rust
use crucible::{BinaryMetric, LlmJudgeContext};
use schemars::schema_for;

fn yes_no_prompt(question: &str, _ctx: &LlmJudgeContext) -> String {
    format!(
        "{question}\n\nRespond with JSON matching this schema:\n{}\n\nOnly return JSON.",
        serde_json::to_string_pretty(&schema_for!(BinaryMetric)).unwrap()
    )
}
```

Run LLM judges explicitly on the report:

```rust
let judgment = report.judge(&judge_llm, |ctx| yes_no_prompt("Did this pass?", ctx)).await;
assert!(judgment.passed(), "{}\n\n{}", judgment.reason(), report.failure_context());
```

### Best Practices

- Keep each eval focused on one primary MCP behavior.
- Prefer file and tool-call assertions before adding an LLM judge.
- Use `BinaryMetric` or `NumericMetric` schemas for judge responses.
- Keep eval prompts clear and user-like.
- Assert on `EvalReport` directly with normal Rust assertions.
- Run evals with `cargo nextest run` or `just test`.
