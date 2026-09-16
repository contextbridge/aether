use aether_evals::{Task, Transcript, Workspace};
use internal_evals::{EvalAgent, EvalHarnessError};

#[path = "common/mod.rs"]
mod common;
use common::read_file;

const NOTES_TXT_CONTENT: &str = "old value\n";
const PLAN_PATH: &str = "docs/aether/plans/notes-plan.md";
const PLAN_PROMPT: &str = "/plan Plan replacing 'old value' with 'new value' in notes.txt. Write the plan to docs/aether/plans/notes-plan.md and request review. This is a planning-only task; do not change notes.txt. If review is cancelled, declined, or unavailable, stop without implementing or reopening review.";

#[tokio::test]
async fn plan_agent_writes_artifact_without_modifying_source_eval() -> Result<(), EvalHarnessError> {
    assert_planning_preserves_source("Plan").await
}

#[tokio::test]
async fn build_agent_plan_skill_writes_artifact_without_modifying_source_eval() -> Result<(), EvalHarnessError> {
    assert_planning_preserves_source("Build").await
}

async fn assert_planning_preserves_source(agent: &str) -> Result<(), EvalHarnessError> {
    let workspace = Workspace::from_files([
        ("notes.txt", NOTES_TXT_CONTENT),
        (".agents/skills/plan.md", include_str!("../../aether-cli/src/init/templates/skills/plan.md")),
    ])?;
    let (_container, stream) =
        EvalAgent::new().agent(agent).run(&workspace, Task::new(PLAN_PROMPT.to_string())).await?;
    let trace = Transcript::from_stream(stream).await?;

    assert_eq!(read_file(&workspace, "notes.txt")?, NOTES_TXT_CONTENT);
    let plan = read_file(&workspace, PLAN_PATH)?;
    assert!(plan.contains("notes.txt"), "plan should identify the file to change: {plan}");
    assert!(plan.contains("new value"), "plan should describe the requested change: {plan}");
    assert!(trace.tool_called("review__review_artifact"), "plan must be presented for review");
    Ok(())
}
