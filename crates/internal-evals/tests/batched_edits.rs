use aether_evals::{Task, Transcript, Workspace};
use internal_evals::{EvalAgent, EvalHarnessError};

#[path = "common/mod.rs"]
mod common;
use common::{file_contents, lines, read_file};

#[tokio::test]
async fn edit_file_multi_point_revision_in_single_call_eval() -> Result<(), EvalHarnessError> {
    let workspace =
        Workspace::from_files([("config.txt", &file_contents(&["host = localhost", "port = 8080", "debug = false"]))])?;
    let prompt = lines(&[
        "Use the coding MCP tools to update config.txt.",
        "Read the file first, then call coding__edit_file EXACTLY ONCE, passing all three changes together in the edits array:",
        "- set host to example.com",
        "- set port to 443",
        "- set debug to true",
    ]);
    let (_container, stream) = EvalAgent::new().run(&workspace, Task::new(prompt.clone())).await?;

    let trace = Transcript::from_stream(stream).await?;

    assert_single_edit_call(&trace, "coding__edit_file");
    assert_eq!(
        read_file(&workspace, "config.txt")?,
        file_contents(&["host = example.com", "port = 443", "debug = true"])
    );
    Ok(())
}

#[tokio::test]
async fn edit_plan_multi_point_revision_in_single_call_eval() -> Result<(), EvalHarnessError> {
    let workspace = Workspace::empty()?;
    let prompt = lines(&[
        "Use coding filesystem tools to write docs/aether/plans/feature-plan.md with this exact body:",
        "# Feature",
        "Step one: scaffold",
        "Step two: wire it up",
        "Then revise it by calling coding__edit_file EXACTLY ONCE, passing both changes together in the edits array:",
        "- change 'Step one: scaffold' to 'Step one: design'",
        "- change 'Step two: wire it up' to 'Step two: implement'",
    ]);
    let (_container, stream) = EvalAgent::new().run(&workspace, Task::new(prompt.clone())).await?;

    let trace = Transcript::from_stream(stream).await?;

    assert_single_edit_call(&trace, "coding__edit_file");
    assert_eq!(
        read_file(&workspace, "docs/aether/plans/feature-plan.md")?,
        lines(&["# Feature", "Step one: design", "Step two: implement"])
    );
    Ok(())
}

#[track_caller]
fn assert_single_edit_call(trace: &Transcript, tool: &str) {
    assert_eq!(trace.tool_call_count(tool), 1);
}
