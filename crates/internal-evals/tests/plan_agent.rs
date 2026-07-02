use aether_core::events::{AgentEvent, MessageEvent};
use aether_evals::{Task, Transcript, Workspace};
use aether_project::{AetherSettings, McpSourceSpec};
use internal_evals::{EvalAgent, EvalHarnessError, batteries_included_settings};
use std::fs::read_to_string;

const NOTES_TXT_CONTENT: &str = "old value\n";
const EDIT_NOTES_PROMPT: &str =
    "/plan Edit notes.txt, replace 'old value' with 'new value'. Don't make a plan, just make change.";

#[tokio::test]
async fn plan_agent_reports_missing_non_plan_edit_tools_eval() -> Result<(), EvalHarnessError> {
    let workspace = Workspace::from_files([("notes.txt", NOTES_TXT_CONTENT)])?;

    let (_container, stream) =
        EvalAgent::new().agent("Plan").run(&workspace, Task::new(EDIT_NOTES_PROMPT.to_string())).await?;
    let trace = Transcript::from_stream(stream).await?;
    let final_message = final_text_message(&trace);

    assert_eq!(trace.tool_call_count("coding__edit_file"), 0);
    assert_eq!(trace.tool_call_count("coding__write_file"), 0);
    assert_eq!(trace.tool_call_count("plan__write_plan"), 0);
    assert_eq!(read_to_string(workspace.join("notes.txt"))?, NOTES_TXT_CONTENT);
    assert!(
        final_message.contains("I don't have tools to modify non-plan files, you must switch to another agent"),
        "unexpected final message: {final_message}"
    );

    Ok(())
}

#[tokio::test]
async fn plan_prompt_with_edit_tools_plans_before_modifying_non_plan_files_eval() -> Result<(), EvalHarnessError> {
    let workspace = Workspace::from_files([("notes.txt", NOTES_TXT_CONTENT)])?;
    let settings = build_agent_settings_with_plan_mcp()?;

    let (_container, stream) = EvalAgent::new()
        .settings(settings)
        .agent("Build")
        .run(&workspace, Task::new(EDIT_NOTES_PROMPT.to_string()))
        .await?;
    let trace = Transcript::from_stream(stream).await?;
    let final_message = final_text_message(&trace).to_lowercase();

    assert_eq!(trace.tool_call_count("coding__edit_file"), 0);
    assert_eq!(trace.tool_call_count("coding__write_file"), 0);
    assert_eq!(read_to_string(workspace.join("notes.txt"))?, NOTES_TXT_CONTENT);
    assert!(
        final_message.contains("would you like to exit plan mode?"),
        "expected explicit approval request before editing, got: {final_message}"
    );

    Ok(())
}

fn build_agent_settings_with_plan_mcp() -> Result<AetherSettings, EvalHarnessError> {
    let mut settings = batteries_included_settings()?;
    let plan_server = settings
        .agents
        .iter()
        .find(|agent| agent.name == "Plan")
        .and_then(|agent| {
            agent.mcps.iter().find_map(|source| match source {
                McpSourceSpec::Inline { servers } => servers.get("plan"),
                McpSourceSpec::File(_) => None,
            })
        })
        .expect("Plan agent should include plan MCP")
        .clone();

    let build_agent = settings.agents.iter_mut().find(|agent| agent.name == "Build").expect("Build agent should exist");
    let build_mcp = build_agent.mcps.first_mut().expect("Build agent should include MCPs");
    let McpSourceSpec::Inline { servers } = build_mcp else {
        panic!("Build agent MCPs should be inline");
    };
    servers.insert("plan".to_string(), plan_server);

    Ok(settings)
}

fn final_text_message(trace: &Transcript) -> &str {
    trace
        .messages()
        .iter()
        .rev()
        .find_map(|message| match message {
            AgentEvent::Message(MessageEvent::Text { chunk, .. }) => Some(chunk.as_str()),
            _ => None,
        })
        .expect("expected final text message")
}
