use aether_evals::{Task, Transcript, Workspace};
use internal_evals::{EvalAgent, EvalHarnessError};
use serde_json::Value;

#[path = "common/mod.rs"]
mod common;
use common::{lines, read_file};

#[tokio::test]
async fn find_bare_readme_pattern_matches_nested_basenames_eval() -> Result<(), EvalHarnessError> {
    let workspace = Workspace::from_files([
        ("README.md", "root docs\n"),
        ("docs/README.adoc", "nested docs\n"),
        ("docs/guide.md", "not a readme\n"),
    ])?;

    let prompt = lines(&[
        "Use the coding MCP tools, not shell commands.",
        "Read every README file in this workspace, including README files in nested directories.",
        "Write find-report.txt summarizing the path and contents of each README you read.",
    ]);
    let (_container, stream) = EvalAgent::new().run(&workspace, Task::new(prompt)).await?;

    let trace = Transcript::from_stream(stream).await?;

    assert!(trace.tool_called("coding__find"));
    assert_eq!(trace.tool_call_count("coding__bash"), 0);
    let report = read_file(&workspace, "find-report.txt")?;
    assert!(report.contains("README.md"), "report:\n{report}");
    assert!(report.contains("root docs"), "report:\n{report}");
    assert!(report.contains("docs/README.adoc"), "report:\n{report}");
    assert!(report.contains("nested docs"), "report:\n{report}");
    assert!(!report.contains("docs/guide.md"), "report:\n{report}");
    Ok(())
}

#[tokio::test]
async fn find_slash_pattern_is_relative_to_workspace_root_eval() -> Result<(), EvalHarnessError> {
    let workspace = Workspace::from_files([
        ("lib.rs", "contents for lib.rs\n"),
        ("crates/service/src/lib.rs", "contents for crates/service/src/lib.rs\n"),
        ("crates/cli/src/main.rs", "contents for crates/cli/src/main.rs\n"),
        ("examples/demo.rs", "contents for examples/demo.rs\n"),
    ])?;
    let prompt = lines(&[
        "Use the coding MCP tools to inventory this Rust workspace.",
        "List only Rust source files that are inside the crates directory; ignore Rust files at the workspace root or under examples.",
        "Write crates-rust-files.txt with the paths you found, one path per line.",
    ]);
    let (_container, stream) = EvalAgent::new().run(&workspace, Task::new(prompt)).await?;

    let trace = Transcript::from_stream(stream).await?;

    assert!(trace.tool_called("coding__find"));
    let report = read_file(&workspace, "crates-rust-files.txt")?;
    assert!(report.contains("crates/service/src/lib.rs"), "report:\n{report}");
    assert!(report.contains("crates/cli/src/main.rs"), "report:\n{report}");
    assert!(!report.contains("examples/demo.rs"), "report:\n{report}");
    assert!(!report.lines().any(|line| line.ends_with("lib.rs") && !line.contains("crates/")), "report:\n{report}");
    Ok(())
}

#[tokio::test]
async fn find_hidden_case_insensitive_limited_search_eval() -> Result<(), EvalHarnessError> {
    let workspace = Workspace::from_files([
        (".aether/settings.json", "contents for .aether/settings.json\n"),
        ("CONFIG/SETTINGS.JSON", "contents for CONFIG/SETTINGS.JSON\n"),
        ("notes/settings.toml", "contents for notes/settings.toml\n"),
    ])?;
    let prompt = lines(&[
        "Use exactly one coding__find call and no shell commands.",
        "Find settings JSON files in this workspace, including files in hidden directories and files whose names use different casing.",
        "Only return one matching path from the search, and report whether the result list was truncated.",
        "Write settings-find-summary.txt with two lines: count=<returned count>, truncated=<true or false>.",
    ]);
    let (_container, stream) = EvalAgent::new().run(&workspace, Task::new(prompt)).await?;

    let trace = Transcript::from_stream(stream).await?;

    assert_eq!(trace.tool_call_count("coding__find"), 1);
    assert_eq!(trace.tool_call_count("coding__bash"), 0);
    assert_find_call_has_bool_arg(&trace, "includeHidden", true);
    assert_find_call_has_bool_arg(&trace, "caseInsensitive", true);
    assert_find_call_has_usize_arg(&trace, "limit", 1);
    let report = read_file(&workspace, "settings-find-summary.txt")?;
    assert!(report.contains("count=1"), "report:\n{report}");
    assert!(report.contains("truncated=true"), "report:\n{report}");
    Ok(())
}

#[track_caller]
fn assert_find_call_has_bool_arg(trace: &Transcript, key: &str, expected: bool) {
    assert!(
        trace.tool_calls("coding__find").any(|call| {
            call.arguments_json()
                .ok()
                .and_then(|args| args.get(key).and_then(Value::as_bool))
                .is_some_and(|actual| actual == expected)
        }),
        "expected coding__find call with {key}={expected}"
    );
}

#[track_caller]
fn assert_find_call_has_usize_arg(trace: &Transcript, key: &str, expected: u64) {
    assert!(
        trace.tool_calls("coding__find").any(|call| {
            call.arguments_json()
                .ok()
                .and_then(|args| args.get(key).and_then(Value::as_u64))
                .is_some_and(|actual| actual == expected)
        }),
        "expected coding__find call with {key}={expected}"
    );
}
