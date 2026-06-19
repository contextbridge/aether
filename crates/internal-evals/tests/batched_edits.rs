mod common;
use aether_evals::{Task, TaskRun, Workspace};
use common::{EvalHarnessError, create_aether_agent};

#[tokio::test]
async fn edit_file_multi_point_revision_in_single_call_eval() -> Result<(), EvalHarnessError> {
    let workspace =
        Workspace::from_files([("config.txt", &file_contents(&["host = localhost", "port = 8080", "debug = false"]))])?;
    let agent = create_aether_agent();
    let prompt = lines(&[
        "Use the coding MCP tools to update config.txt.",
        "Read the file first, then call coding__edit_file EXACTLY ONCE, passing all three changes together in the edits array:",
        "- set host to example.com",
        "- set port to 443",
        "- set debug to true",
    ]);

    let run = Task::new(prompt, workspace).run(&agent).await?;

    assert_single_edit_call(&run, "coding__edit_file");
    assert_eq!(
        read_file(&run, "config.txt")?,
        file_contents(&["host = example.com", "port = 443", "debug = true"]),
        "{}",
        run.failure_context()
    );
    Ok(())
}

#[tokio::test]
async fn edit_plan_multi_point_revision_in_single_call_eval() -> Result<(), EvalHarnessError> {
    let agent = create_aether_agent();
    let prompt = lines(&[
        "Use the plan MCP tools.",
        "First call plan__write_plan with planName 'feature' and this exact body:",
        "# Feature",
        "Step one: scaffold",
        "Step two: wire it up",
        "Then revise it by calling plan__edit_plan EXACTLY ONCE, passing both changes together in the edits array:",
        "- change 'Step one: scaffold' to 'Step one: design'",
        "- change 'Step two: wire it up' to 'Step two: implement'",
    ]);

    let run = Task::new(prompt, Workspace::empty()?).run(&agent).await?;

    assert_single_edit_call(&run, "plan__edit_plan");
    assert_eq!(
        read_file(&run, "docs/aether/plans/feature-plan.md")?,
        lines(&["# Feature", "Step one: design", "Step two: implement"]),
        "{}",
        run.failure_context()
    );
    Ok(())
}

#[track_caller]
fn assert_single_edit_call(run: &TaskRun, tool: &str) {
    assert_eq!(run.transcript().tool_call_count(tool), 1, "{}", run.failure_context());
}

fn file_contents(lines: &[&str]) -> String {
    format!("{}\n", lines.join("\n"))
}

fn lines(lines: &[&str]) -> String {
    lines.join("\n")
}

fn read_file(run: &TaskRun, path: &str) -> Result<String, EvalHarnessError> {
    Ok(std::fs::read_to_string(run.workspace().join(path))?)
}
