mod common;

use aether_evals::{Task, TaskRun, Workspace};
use common::{EvalHarnessError, create_aether_agent};

#[tokio::test]
async fn edit_file_replaces_first_match_by_default_eval() -> Result<(), EvalHarnessError> {
    let initial_notes = file_contents(&["alpha", "alpha"]);
    let workspace = Workspace::from_files([("notes.txt", &initial_notes)])?;
    let agent = create_aether_agent();
    let prompt = lines(&[
        "Use the coding MCP tools to update notes.txt.",
        "Read the file first, then call coding__edit_file exactly once to replace only the first 'alpha' with 'beta'.",
        "Do not replace the second 'alpha'.",
    ]);

    let run = Task::new(prompt, workspace).run(&agent).await?;

    assert_read_then_single_edit(&run);
    assert_eq!(read_file(&run, "notes.txt"), file_contents(&["beta", "alpha"]), "{}", run.failure_context());
    Ok(())
}

#[tokio::test]
async fn edit_file_replace_all_updates_every_match_eval() -> Result<(), EvalHarnessError> {
    let initial_tasks = file_contents(&["todo: one", "todo: two", "todo: three"]);
    let workspace = Workspace::from_files([("tasks.md", &initial_tasks)])?;
    let agent = create_aether_agent();
    let prompt = lines(&[
        "Use the coding MCP tools to update tasks.md.",
        "Read the file first, then call coding__edit_file exactly once with replaceAll enabled to change every 'todo' marker to 'done'.",
    ]);

    let run = Task::new(prompt, workspace).run(&agent).await?;

    let contents = read_file(&run, "tasks.md");
    assert_read_then_single_edit(&run);
    assert!(!contents.contains("todo"), "{}", run.failure_context());
    assert_eq!(contents.matches("done").count(), 3, "{}", run.failure_context());
    Ok(())
}

#[tokio::test]
async fn edit_file_handles_multiline_exact_replacement_eval() -> Result<(), EvalHarnessError> {
    let initial_lib = file_contents(&["pub fn greet() {", "    println!(\"hello\");", "}", "", "pub fn keep() {}"]);
    let workspace = Workspace::from_files([("src/lib.rs", &initial_lib)])?;
    let agent = create_aether_agent();
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

    let run = Task::new(prompt, workspace).run(&agent).await?;

    let contents = read_file(&run, "src/lib.rs");
    assert_read_then_single_edit(&run);
    assert!(contents.contains("println!(\"hello from edit_file\");"), "{}", run.failure_context());
    assert!(contents.contains("pub fn keep() {}"), "{}", run.failure_context());
    Ok(())
}

#[tokio::test]
async fn edit_file_pattern_not_found_leaves_file_unchanged_eval() -> Result<(), EvalHarnessError> {
    let initial_config = file_contents(&["mode = \"safe\""]);
    let workspace = Workspace::from_files([("config.toml", &initial_config)])?;
    let agent = create_aether_agent();
    let prompt = lines(&[
        "Use the coding MCP tools on config.toml.",
        "Read the file first, then intentionally call coding__edit_file exactly once with oldString set to 'mode = \"missing\"' and newString set to 'mode = \"unsafe\"'.",
        "This old string is not present; report the tool error and leave the file unchanged.",
    ]);

    let run = Task::new(prompt, workspace).run(&agent).await?;

    let contents = read_file(&run, "config.toml");
    assert_read_then_single_edit(&run);
    assert_eq!(contents, file_contents(&["mode = \"safe\""]), "{}", run.failure_context());
    assert!(!contents.contains("unsafe"), "{}", run.failure_context());
    Ok(())
}

#[track_caller]
fn assert_read_then_single_edit(run: &TaskRun) {
    assert!(run.transcript().tool_called("coding__read_file"), "{}", run.failure_context());
    assert_eq!(run.transcript().tool_call_count("coding__edit_file"), 1, "{}", run.failure_context());
}

fn file_contents(lines: &[&str]) -> String {
    format!("{}\n", lines.join("\n"))
}

fn lines(lines: &[&str]) -> String {
    lines.join("\n")
}

fn read_file(run: &TaskRun, path: &str) -> String {
    std::fs::read_to_string(run.workspace().join(path)).unwrap_or_else(|error| panic!("failed to read {path}: {error}"))
}
