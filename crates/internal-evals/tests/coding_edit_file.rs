mod common;

use std::fs::read_to_string;

use aether_evals::{Agent, Task, Transcript, Workspace};
use common::{EvalHarnessError, create_aether_agent};

#[tokio::test]
async fn edit_file_replaces_first_match_by_default_eval() -> Result<(), EvalHarnessError> {
    let initial_notes = file_contents(&["alpha", "alpha"]);
    let workspace = Workspace::from_files([("notes.txt", &initial_notes)])?;
    let (_container, agent) = create_aether_agent(&workspace).await?;
    let prompt = lines(&[
        "Use the coding MCP tools to update notes.txt.",
        "Read the file first, then call coding__edit_file exactly once to replace only the first 'alpha' with 'beta'.",
        "Do not replace the second 'alpha'.",
    ]);

    let trace = Transcript::from_stream(agent.run(Task::new(prompt.clone()))).await?;

    assert_read_then_single_edit(&trace);
    assert_eq!(read_file(&workspace, "notes.txt")?, file_contents(&["beta", "alpha"]));
    Ok(())
}

#[tokio::test]
async fn edit_file_replace_all_updates_every_match_eval() -> Result<(), EvalHarnessError> {
    let initial_tasks = file_contents(&["todo: one", "todo: two", "todo: three"]);
    let workspace = Workspace::from_files([("tasks.md", &initial_tasks)])?;
    let (_container, agent) = create_aether_agent(&workspace).await?;
    let prompt = lines(&[
        "Use the coding MCP tools to update tasks.md.",
        "Read the file first, then call coding__edit_file exactly once, using a single replace edit with replaceAll enabled, to change every 'todo' marker to 'done'.",
    ]);

    let trace = Transcript::from_stream(agent.run(Task::new(prompt.clone()))).await?;

    let contents = read_file(&workspace, "tasks.md")?;
    assert_read_then_single_edit(&trace);
    assert!(!contents.contains("todo"));
    assert_eq!(contents.matches("done").count(), 3);
    Ok(())
}

#[tokio::test]
async fn edit_file_handles_multiline_exact_replacement_eval() -> Result<(), EvalHarnessError> {
    let initial_lib = file_contents(&["pub fn greet() {", "    println!(\"hello\");", "}", "", "pub fn keep() {}"]);
    let workspace = Workspace::from_files([("src/lib.rs", &initial_lib)])?;
    let (_container, agent) = create_aether_agent(&workspace).await?;
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

    let trace = Transcript::from_stream(agent.run(Task::new(prompt.clone()))).await?;

    let contents = read_file(&workspace, "src/lib.rs")?;
    assert_read_then_single_edit(&trace);
    assert!(contents.contains("println!(\"hello from edit_file\");"));
    assert!(contents.contains("pub fn keep() {}"));
    Ok(())
}

#[tokio::test]
async fn edit_file_pattern_not_found_leaves_file_unchanged_eval() -> Result<(), EvalHarnessError> {
    let initial_config = file_contents(&["mode = \"safe\""]);
    let workspace = Workspace::from_files([("config.toml", &initial_config)])?;
    let (_container, agent) = create_aether_agent(&workspace).await?;
    let prompt = lines(&[
        "Use the coding MCP tools on config.toml.",
        "Read the file first, then intentionally call coding__edit_file exactly once with a single replace edit whose oldString is 'mode = \"missing\"' and newString is 'mode = \"unsafe\"'.",
        "This old string is not present; report the tool error and leave the file unchanged.",
    ]);

    let trace = Transcript::from_stream(agent.run(Task::new(prompt.clone()))).await?;

    let contents = read_file(&workspace, "config.toml")?;
    assert_read_then_single_edit(&trace);
    assert_eq!(contents, file_contents(&["mode = \"safe\""]));
    assert!(!contents.contains("unsafe"));
    Ok(())
}

#[track_caller]
fn assert_read_then_single_edit(trace: &Transcript) {
    assert!(trace.tool_called("coding__read_file"));
    assert_eq!(trace.tool_call_count("coding__edit_file"), 1);
}

fn file_contents(lines: &[&str]) -> String {
    format!("{}\n", lines.join("\n"))
}

fn lines(lines: &[&str]) -> String {
    lines.join("\n")
}

fn read_file(workspace: &Workspace, path: &str) -> Result<String, EvalHarnessError> {
    Ok(read_to_string(workspace.join(path))?)
}
