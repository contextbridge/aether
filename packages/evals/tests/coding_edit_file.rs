use aether_evals::{EvalHarnessError, create_aether_agent, write_fixture_files};
use crucible::{EvalReport, run_eval};

#[tokio::test]
async fn edit_file_replaces_first_match_by_default_eval() -> Result<(), EvalHarnessError> {
    let initial_notes = file_contents(&["alpha", "alpha"]);
    let workspace = write_fixture_files(&[("notes.txt", &initial_notes)])?;
    let agent = create_aether_agent(workspace.path()).await?;
    let prompt = lines(&[
        "Use the coding MCP tools to update notes.txt.",
        "Read the file first, then call coding__edit_file exactly once to replace only the first 'alpha' with 'beta'.",
        "Do not replace the second 'alpha'.",
    ]);

    let report = run_eval(&agent, prompt, workspace).await?;

    assert_read_then_single_edit(&report);
    assert_eq!(read_file(&report, "notes.txt"), file_contents(&["beta", "alpha"]), "{}", report.failure_context());
    Ok(())
}

#[tokio::test]
async fn edit_file_replace_all_updates_every_match_eval() -> Result<(), EvalHarnessError> {
    let initial_tasks = file_contents(&["todo: one", "todo: two", "todo: three"]);
    let workspace = write_fixture_files(&[("tasks.md", &initial_tasks)])?;
    let agent = create_aether_agent(workspace.path()).await?;
    let prompt = lines(&[
        "Use the coding MCP tools to update tasks.md.",
        "Read the file first, then call coding__edit_file exactly once with replaceAll enabled to change every 'todo' marker to 'done'.",
    ]);

    let report = run_eval(&agent, prompt, workspace).await?;

    let contents = read_file(&report, "tasks.md");
    assert_read_then_single_edit(&report);
    assert!(!contents.contains("todo"), "{}", report.failure_context());
    assert_eq!(contents.matches("done").count(), 3, "{}", report.failure_context());
    Ok(())
}

#[tokio::test]
async fn edit_file_handles_multiline_exact_replacement_eval() -> Result<(), EvalHarnessError> {
    let initial_lib = file_contents(&["pub fn greet() {", "    println!(\"hello\");", "}", "", "pub fn keep() {}"]);
    let workspace = write_fixture_files(&[("src/lib.rs", &initial_lib)])?;
    let agent = create_aether_agent(workspace.path()).await?;
    let prompt = lines(&[
        "Use the coding MCP tools to update src/lib.rs.",
        "Read the file first, then call coding__edit_file exactly once to replace the entire greet function with:",
        "",
        "pub fn greet() {",
        "    println!(\"hello from edit_file\");",
        "}",
        "",
        "Preserve pub fn keep() unchanged.",
    ]);

    let report = run_eval(&agent, prompt, workspace).await?;

    let contents = read_file(&report, "src/lib.rs");
    assert_read_then_single_edit(&report);
    assert!(contents.contains("println!(\"hello from edit_file\");"), "{}", report.failure_context());
    assert!(contents.contains("pub fn keep() {}"), "{}", report.failure_context());
    Ok(())
}

#[tokio::test]
async fn edit_file_pattern_not_found_leaves_file_unchanged_eval() -> Result<(), EvalHarnessError> {
    let initial_config = file_contents(&["mode = \"safe\""]);
    let workspace = write_fixture_files(&[("config.toml", &initial_config)])?;
    let agent = create_aether_agent(workspace.path()).await?;
    let prompt = lines(&[
        "Use the coding MCP tools on config.toml.",
        "Read the file first, then intentionally call coding__edit_file exactly once with oldString set to 'mode = \"missing\"' and newString set to 'mode = \"unsafe\"'.",
        "This old string is not present; report the tool error and leave the file unchanged.",
    ]);

    let report = run_eval(&agent, prompt, workspace).await?;

    let contents = read_file(&report, "config.toml");
    assert_read_then_single_edit(&report);
    assert_eq!(contents, file_contents(&["mode = \"safe\""]), "{}", report.failure_context());
    assert!(!contents.contains("unsafe"), "{}", report.failure_context());
    Ok(())
}

#[track_caller]
fn assert_read_then_single_edit(report: &EvalReport) {
    assert!(report.tool_called("coding__read_file"), "{}", report.failure_context());
    assert_eq!(report.tool_call_count("coding__edit_file"), 1, "{}", report.failure_context());
}

fn file_contents(lines: &[&str]) -> String {
    format!("{}\n", lines.join("\n"))
}

fn lines(lines: &[&str]) -> String {
    lines.join("\n")
}

fn read_file(report: &EvalReport, path: &str) -> String {
    std::fs::read_to_string(report.path(path)).unwrap_or_else(|error| panic!("failed to read {path}: {error}"))
}
