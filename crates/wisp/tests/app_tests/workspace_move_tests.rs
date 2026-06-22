use acp_utils::client::PromptCommand;
use acp_utils::notifications::{WorkspaceEntry, WorkspaceMoveTarget};
use agent_client_protocol::schema as acp;
use std::{io, path::PathBuf};
use tokio::sync::mpsc::UnboundedReceiver;

use super::common::*;

#[tokio::test]
async fn move_command_is_listed_only_when_capability_advertised() -> TestResult {
    let mut renderer = workspace_renderer()?;
    type_string(&mut renderer, "/").await?;
    let names = command_picker_visible_names(renderer.writer());
    assert!(names.iter().any(|n| n == "move"), "expected /move in {names:?}");

    let mut renderer = RendererTest::new().build()?;
    type_string(&mut renderer, "/").await?;
    let names = command_picker_visible_names(renderer.writer());
    assert!(!names.iter().any(|n| n == "move"), "did not expect /move in {names:?}");
    Ok(())
}

#[tokio::test]
async fn move_command_requests_workspace_list() -> TestResult {
    let mut renderer = workspace_renderer()?;

    type_string(&mut renderer, "/move").await?;
    press(&mut renderer, Enter).await?;

    match renderer.commands().try_recv().expect("expected list workspaces command") {
        PromptCommand::ListWorkspaces(params) => assert_eq!(params.session_id, "test"),
        other => panic!("expected ListWorkspaces command, got {other:?}"),
    }
    Ok(())
}

#[tokio::test]
async fn workspaces_listed_opens_picker_hiding_current_workspace() -> TestResult {
    let mut renderer = workspace_renderer()?;

    type_string(&mut renderer, "/move").await?;
    press(&mut renderer, Enter).await?;
    renderer.on_workspaces_listed(vec![
        entry("/repo/aether", true),
        entry("/repo/aether-fix", false),
        entry("/repo/aether-experiment", false),
    ])?;

    assert_buffer_contains(renderer.writer(), "Move to workspace");
    assert_buffer_contains(renderer.writer(), "aether-fix");
    assert_buffer_contains(renderer.writer(), "aether-experiment");
    assert_buffer_contains(renderer.writer(), "Create new workspace");
    let lines = renderer.writer().get_lines();
    assert!(!lines.iter().any(|l| l.trim() == "/repo/aether"), "current workspace should be hidden");
    Ok(())
}

#[tokio::test]
async fn selecting_existing_workspace_sends_move_command() -> TestResult {
    let mut renderer = workspace_renderer()?;

    type_string(&mut renderer, "/move").await?;
    press(&mut renderer, Enter).await?;
    drain(renderer.commands());
    renderer.on_workspaces_listed(vec![entry("/repo/aether", true), entry("/repo/aether-fix", false)])?;

    press(&mut renderer, Enter).await?;

    match renderer.commands().try_recv().expect("expected move workspace command") {
        PromptCommand::MoveWorkspace(params) => {
            assert_eq!(params.session_id, "test");
            assert_eq!(params.target, WorkspaceMoveTarget::Existing { path: PathBuf::from("/repo/aether-fix") });
        }
        other => panic!("expected MoveWorkspace command, got {other:?}"),
    }
    assert_buffer_not_contains(renderer.writer(), "Move to workspace");
    assert_buffer_contains(renderer.writer(), "Moving workspace...");
    assert_buffer_not_contains(renderer.writer(), "esc to interrupt");
    Ok(())
}

#[tokio::test]
async fn workspace_move_spinner_transitions_to_session_load_and_clears_when_loaded() -> TestResult {
    let mut renderer = workspace_renderer()?;

    type_string(&mut renderer, "/move").await?;
    press(&mut renderer, Enter).await?;
    drain(renderer.commands());
    renderer.on_workspaces_listed(vec![entry("/repo/aether", true), entry("/repo/aether-fix", false)])?;
    press(&mut renderer, Enter).await?;
    drain(renderer.commands());

    assert_buffer_contains(renderer.writer(), "Moving workspace...");

    renderer.on_workspace_moved(PathBuf::from("/elsewhere/aether-2"))?;

    let command = renderer.commands().try_recv().map_err(|_| io::Error::other("expected load session command"))?;
    let PromptCommand::LoadSession { session_id, cwd } = command else {
        return Err(io::Error::other(format!("expected LoadSession command, got {command:?}")).into());
    };
    assert_eq!(session_id.0.as_ref(), "test");
    assert_eq!(cwd, PathBuf::from("/elsewhere/aether-2"));

    assert_buffer_not_contains(renderer.writer(), "Moving workspace...");
    assert_buffer_contains(renderer.writer(), "Loading session in new workspace...");

    renderer.on_session_loaded(acp::SessionId::new("test"), Vec::new())?;

    assert_buffer_not_contains(renderer.writer(), "Loading session in new workspace...");
    Ok(())
}

#[tokio::test]
async fn create_new_workspace_flow_submits_typed_name() -> TestResult {
    let mut renderer = workspace_renderer()?;

    type_string(&mut renderer, "/move").await?;
    press(&mut renderer, Enter).await?;
    drain(renderer.commands());
    renderer.on_workspaces_listed(vec![entry("/repo/aether", true), entry("/repo/aether-fix", false)])?;

    press(&mut renderer, Down).await?;
    press(&mut renderer, Enter).await?;
    assert_buffer_contains(renderer.writer(), "New workspace name");
    assert_buffer_contains(renderer.writer(), "will be created in");

    type_string(&mut renderer, "aether-2").await?;
    press(&mut renderer, Enter).await?;

    match renderer.commands().try_recv().expect("expected move workspace command") {
        PromptCommand::MoveWorkspace(params) => {
            assert_eq!(params.target, WorkspaceMoveTarget::New { name: "aether-2".to_string() });
        }
        other => panic!("expected MoveWorkspace command, got {other:?}"),
    }
    Ok(())
}

#[tokio::test]
async fn escape_in_name_input_returns_to_workspace_list() -> TestResult {
    let mut renderer = workspace_renderer()?;

    type_string(&mut renderer, "/move").await?;
    press(&mut renderer, Enter).await?;
    drain(renderer.commands());
    renderer.on_workspaces_listed(vec![entry("/repo/aether", true), entry("/repo/aether-fix", false)])?;

    press(&mut renderer, Down).await?;
    press(&mut renderer, Enter).await?;
    assert_buffer_contains(renderer.writer(), "New workspace name");

    press(&mut renderer, Esc).await?;

    assert_buffer_contains(renderer.writer(), "Move to workspace");
    assert_buffer_contains(renderer.writer(), "aether-fix");
    assert!(drain(renderer.commands()).is_empty(), "escape should not send commands");
    Ok(())
}

#[tokio::test]
async fn workspace_moved_updates_status_line_and_reloads_session() -> TestResult {
    let mut renderer = workspace_renderer()?;

    renderer.on_workspace_moved(PathBuf::from("/elsewhere/aether-2"))?;

    match renderer.commands().try_recv().expect("expected load session command") {
        PromptCommand::LoadSession { session_id, cwd } => {
            assert_eq!(session_id.0.as_ref(), "test");
            assert_eq!(cwd, PathBuf::from("/elsewhere/aether-2"));
        }
        other => panic!("expected LoadSession command, got {other:?}"),
    }
    assert_buffer_contains(renderer.writer(), "/elsewhere/aether-2");
    Ok(())
}

#[tokio::test]
async fn workspace_move_failure_is_reported_in_conversation() -> TestResult {
    let mut renderer = workspace_renderer()?;

    renderer.on_workspace_move_failed("target workspace has uncommitted changes: /repo/aether-fix")?;

    assert_buffer_not_contains(renderer.writer(), "Moving workspace...");
    assert_buffer_contains(renderer.writer(), "[wisp] Workspace move failed:");
    assert_buffer_contains(renderer.writer(), "uncommitted changes");
    Ok(())
}

#[tokio::test]
async fn move_command_is_rejected_while_waiting_for_response() -> TestResult {
    let mut renderer = workspace_renderer()?;

    type_string(&mut renderer, "hello").await?;
    press(&mut renderer, Enter).await?;
    drain(renderer.commands());

    type_string(&mut renderer, "/move").await?;
    press(&mut renderer, Enter).await?;

    assert_buffer_contains(renderer.writer(), "[wisp] Cannot move workspaces while a prompt is running.");
    assert!(drain(renderer.commands()).is_empty(), "no command should be sent while waiting");
    Ok(())
}

#[tokio::test]
async fn closing_workspace_picker_allows_move_to_be_requested_again() -> TestResult {
    let mut renderer = workspace_renderer()?;

    type_string(&mut renderer, "/move").await?;
    press(&mut renderer, Enter).await?;
    drain(renderer.commands());
    renderer.on_workspaces_listed(vec![entry("/repo/aether", true), entry("/repo/aether-fix", false)])?;

    press(&mut renderer, Esc).await?;
    type_string(&mut renderer, "/move").await?;
    press(&mut renderer, Enter).await?;

    match renderer.commands().try_recv().expect("expected list workspaces command") {
        PromptCommand::ListWorkspaces(params) => assert_eq!(params.session_id, "test"),
        other => panic!("expected ListWorkspaces command, got {other:?}"),
    }
    Ok(())
}

#[tokio::test]
async fn mouse_selecting_existing_workspace_sends_move_command() -> TestResult {
    let mut renderer = workspace_renderer()?;

    type_string(&mut renderer, "/move").await?;
    press(&mut renderer, Enter).await?;
    drain(renderer.commands());
    renderer.on_workspaces_listed(vec![entry("/repo/aether", true), entry("/repo/aether-fix", false)])?;

    renderer.on_mouse_down(4, 4).await?;

    match renderer.commands().try_recv().expect("expected move workspace command") {
        PromptCommand::MoveWorkspace(params) => {
            assert_eq!(params.target, WorkspaceMoveTarget::Existing { path: PathBuf::from("/repo/aether-fix") });
        }
        other => panic!("expected MoveWorkspace command, got {other:?}"),
    }
    Ok(())
}

#[tokio::test]
async fn rejected_move_while_waiting_preserves_draft() -> TestResult {
    let mut renderer = workspace_renderer()?;

    type_string(&mut renderer, "hello").await?;
    press(&mut renderer, Enter).await?;
    drain(renderer.commands());

    type_string(&mut renderer, "/move").await?;
    press(&mut renderer, Enter).await?;

    assert_buffer_contains(renderer.writer(), "[wisp] Cannot move workspaces while a prompt is running.");
    assert_buffer_contains(renderer.writer(), "> /move");
    assert!(drain(renderer.commands()).is_empty(), "no command should be sent while waiting");
    Ok(())
}

#[tokio::test]
async fn move_command_is_rejected_while_workspace_list_is_pending() -> TestResult {
    let mut renderer = workspace_renderer()?;

    type_string(&mut renderer, "/move").await?;
    press(&mut renderer, Enter).await?;
    drain(renderer.commands());

    type_string(&mut renderer, "/move").await?;
    press(&mut renderer, Enter).await?;

    assert_buffer_contains(
        renderer.writer(),
        "[wisp] Cannot move workspaces while another workspace move is in progress.",
    );
    assert_buffer_contains(renderer.writer(), "> /move");
    assert!(drain(renderer.commands()).is_empty(), "no second list command should be sent");
    Ok(())
}

fn workspace_renderer() -> TestResult<Renderer> {
    RendererTest::new().session_capabilities(workspace_session_capabilities()).build()
}

fn entry(path: &str, is_current: bool) -> WorkspaceEntry {
    WorkspaceEntry { path: PathBuf::from(path), is_current }
}

fn drain(commands: &mut UnboundedReceiver<PromptCommand>) -> Vec<PromptCommand> {
    let mut drained = Vec::new();
    while let Ok(cmd) = commands.try_recv() {
        drained.push(cmd);
    }
    drained
}
