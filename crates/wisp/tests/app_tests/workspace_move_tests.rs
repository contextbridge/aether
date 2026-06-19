use acp_utils::client::PromptCommand;
use acp_utils::notifications::{WorkspaceEntry, WorkspaceMoveTarget};
use agent_client_protocol::schema as acp;
use std::{io, path::PathBuf};
use tokio::sync::mpsc::UnboundedReceiver;
use tui::testing::TestTerminal;
use tui::{KeyCode, KeyModifiers};

use super::common::*;

#[tokio::test]
async fn move_command_is_listed_only_when_capability_advertised() {
    let (mut renderer, _commands) = workspace_renderer();
    renderer.initial_render().unwrap();
    type_string(&mut renderer, "/").await;
    let names = command_picker_visible_names(renderer.writer());
    assert!(names.iter().any(|n| n == "move"), "expected /move in {names:?}");

    let terminal = TestTerminal::new(TEST_WIDTH, 40);
    let (mut renderer, _commands) = Renderer::new_recording(terminal, TEST_AGENT.to_string(), &[], (TEST_WIDTH, 40));
    renderer.initial_render().unwrap();
    type_string(&mut renderer, "/").await;
    let names = command_picker_visible_names(renderer.writer());
    assert!(!names.iter().any(|n| n == "move"), "did not expect /move in {names:?}");
}

#[tokio::test]
async fn move_command_requests_workspace_list() {
    let (mut renderer, mut commands) = workspace_renderer();
    renderer.initial_render().unwrap();

    type_string(&mut renderer, "/move").await;
    press_enter(&mut renderer).await;

    match commands.try_recv().expect("expected list workspaces command") {
        PromptCommand::ListWorkspaces(params) => assert_eq!(params.session_id, "test"),
        other => panic!("expected ListWorkspaces command, got {other:?}"),
    }
}

#[tokio::test]
async fn workspaces_listed_opens_picker_hiding_current_workspace() {
    let (mut renderer, _commands) = workspace_renderer();
    renderer.initial_render().unwrap();

    type_string(&mut renderer, "/move").await;
    press_enter(&mut renderer).await;
    renderer
        .on_workspaces_listed(vec![
            entry("/repo/aether", true),
            entry("/repo/aether-fix", false),
            entry("/repo/aether-experiment", false),
        ])
        .unwrap();

    assert_buffer_contains(renderer.writer(), "Move to workspace");
    assert_buffer_contains(renderer.writer(), "aether-fix");
    assert_buffer_contains(renderer.writer(), "aether-experiment");
    assert_buffer_contains(renderer.writer(), "Create new workspace");
    let lines = renderer.writer().get_lines();
    assert!(!lines.iter().any(|l| l.trim() == "/repo/aether"), "current workspace should be hidden");
}

#[tokio::test]
async fn selecting_existing_workspace_sends_move_command() {
    let (mut renderer, mut commands) = workspace_renderer();
    renderer.initial_render().unwrap();

    type_string(&mut renderer, "/move").await;
    press_enter(&mut renderer).await;
    drain(&mut commands);
    renderer.on_workspaces_listed(vec![entry("/repo/aether", true), entry("/repo/aether-fix", false)]).unwrap();

    press_enter(&mut renderer).await;

    match commands.try_recv().expect("expected move workspace command") {
        PromptCommand::MoveWorkspace(params) => {
            assert_eq!(params.session_id, "test");
            assert_eq!(params.target, WorkspaceMoveTarget::Existing { path: PathBuf::from("/repo/aether-fix") });
        }
        other => panic!("expected MoveWorkspace command, got {other:?}"),
    }
    assert_buffer_not_contains(renderer.writer(), "Move to workspace");
    assert_buffer_contains(renderer.writer(), "Moving workspace...");
    assert_buffer_not_contains(renderer.writer(), "esc to interrupt");
}

#[tokio::test]
async fn workspace_move_spinner_transitions_to_session_load_and_clears_when_loaded()
-> Result<(), Box<dyn std::error::Error>> {
    let (mut renderer, mut commands) = workspace_renderer();
    renderer.initial_render()?;

    type_string(&mut renderer, "/move").await;
    press_enter(&mut renderer).await;
    drain(&mut commands);
    renderer.on_workspaces_listed(vec![entry("/repo/aether", true), entry("/repo/aether-fix", false)])?;
    press_enter(&mut renderer).await;
    drain(&mut commands);

    assert_buffer_contains(renderer.writer(), "Moving workspace...");

    renderer.on_workspace_moved(PathBuf::from("/elsewhere/aether-2"))?;

    let command = commands.try_recv().map_err(|_| io::Error::other("expected load session command"))?;
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
async fn create_new_workspace_flow_submits_typed_name() {
    let (mut renderer, mut commands) = workspace_renderer();
    renderer.initial_render().unwrap();

    type_string(&mut renderer, "/move").await;
    press_enter(&mut renderer).await;
    drain(&mut commands);
    renderer.on_workspaces_listed(vec![entry("/repo/aether", true), entry("/repo/aether-fix", false)]).unwrap();

    send_key(&mut renderer, KeyCode::Down, KeyModifiers::NONE).await;
    press_enter(&mut renderer).await;
    assert_buffer_contains(renderer.writer(), "New workspace name");
    assert_buffer_contains(renderer.writer(), "will be created in");

    type_string(&mut renderer, "aether-2").await;
    press_enter(&mut renderer).await;

    match commands.try_recv().expect("expected move workspace command") {
        PromptCommand::MoveWorkspace(params) => {
            assert_eq!(params.target, WorkspaceMoveTarget::New { name: "aether-2".to_string() });
        }
        other => panic!("expected MoveWorkspace command, got {other:?}"),
    }
}

#[tokio::test]
async fn escape_in_name_input_returns_to_workspace_list() {
    let (mut renderer, mut commands) = workspace_renderer();
    renderer.initial_render().unwrap();

    type_string(&mut renderer, "/move").await;
    press_enter(&mut renderer).await;
    drain(&mut commands);
    renderer.on_workspaces_listed(vec![entry("/repo/aether", true), entry("/repo/aether-fix", false)]).unwrap();

    send_key(&mut renderer, KeyCode::Down, KeyModifiers::NONE).await;
    press_enter(&mut renderer).await;
    assert_buffer_contains(renderer.writer(), "New workspace name");

    send_key(&mut renderer, KeyCode::Esc, KeyModifiers::NONE).await;

    assert_buffer_contains(renderer.writer(), "Move to workspace");
    assert_buffer_contains(renderer.writer(), "aether-fix");
    assert!(drain(&mut commands).is_empty(), "escape should not send commands");
}

#[tokio::test]
async fn workspace_moved_updates_status_line_and_reloads_session() {
    let (mut renderer, mut commands) = workspace_renderer();
    renderer.initial_render().unwrap();

    renderer.on_workspace_moved(PathBuf::from("/elsewhere/aether-2")).unwrap();

    match commands.try_recv().expect("expected load session command") {
        PromptCommand::LoadSession { session_id, cwd } => {
            assert_eq!(session_id.0.as_ref(), "test");
            assert_eq!(cwd, PathBuf::from("/elsewhere/aether-2"));
        }
        other => panic!("expected LoadSession command, got {other:?}"),
    }
    assert_buffer_contains(renderer.writer(), "/elsewhere/aether-2");
}

#[tokio::test]
async fn workspace_move_failure_is_reported_in_conversation() {
    let (mut renderer, _commands) = workspace_renderer();
    renderer.initial_render().unwrap();

    renderer.on_workspace_move_failed("target workspace has uncommitted changes: /repo/aether-fix").unwrap();

    assert_buffer_not_contains(renderer.writer(), "Moving workspace...");
    assert_buffer_contains(renderer.writer(), "[wisp] Workspace move failed:");
    assert_buffer_contains(renderer.writer(), "uncommitted changes");
}

#[tokio::test]
async fn move_command_is_rejected_while_waiting_for_response() {
    let (mut renderer, mut commands) = workspace_renderer();
    renderer.initial_render().unwrap();

    type_string(&mut renderer, "hello").await;
    press_enter(&mut renderer).await;
    drain(&mut commands);

    type_string(&mut renderer, "/move").await;
    press_enter(&mut renderer).await;

    assert_buffer_contains(renderer.writer(), "[wisp] Cannot move workspaces while a prompt is running.");
    assert!(drain(&mut commands).is_empty(), "no command should be sent while waiting");
}

#[tokio::test]
async fn closing_workspace_picker_allows_move_to_be_requested_again() {
    let (mut renderer, mut commands) = workspace_renderer();
    renderer.initial_render().unwrap();

    type_string(&mut renderer, "/move").await;
    press_enter(&mut renderer).await;
    drain(&mut commands);
    renderer.on_workspaces_listed(vec![entry("/repo/aether", true), entry("/repo/aether-fix", false)]).unwrap();

    send_key(&mut renderer, KeyCode::Esc, KeyModifiers::NONE).await;
    type_string(&mut renderer, "/move").await;
    press_enter(&mut renderer).await;

    match commands.try_recv().expect("expected list workspaces command") {
        PromptCommand::ListWorkspaces(params) => assert_eq!(params.session_id, "test"),
        other => panic!("expected ListWorkspaces command, got {other:?}"),
    }
}

#[tokio::test]
async fn mouse_selecting_existing_workspace_sends_move_command() {
    let (mut renderer, mut commands) = workspace_renderer();
    renderer.initial_render().unwrap();

    type_string(&mut renderer, "/move").await;
    press_enter(&mut renderer).await;
    drain(&mut commands);
    renderer.on_workspaces_listed(vec![entry("/repo/aether", true), entry("/repo/aether-fix", false)]).unwrap();

    renderer.on_mouse_down(4, 4).await.unwrap();

    match commands.try_recv().expect("expected move workspace command") {
        PromptCommand::MoveWorkspace(params) => {
            assert_eq!(params.target, WorkspaceMoveTarget::Existing { path: PathBuf::from("/repo/aether-fix") });
        }
        other => panic!("expected MoveWorkspace command, got {other:?}"),
    }
}

#[tokio::test]
async fn rejected_move_while_waiting_preserves_draft() {
    let (mut renderer, mut commands) = workspace_renderer();
    renderer.initial_render().unwrap();

    type_string(&mut renderer, "hello").await;
    press_enter(&mut renderer).await;
    drain(&mut commands);

    type_string(&mut renderer, "/move").await;
    press_enter(&mut renderer).await;

    assert_buffer_contains(renderer.writer(), "[wisp] Cannot move workspaces while a prompt is running.");
    assert_buffer_contains(renderer.writer(), "> /move");
    assert!(drain(&mut commands).is_empty(), "no command should be sent while waiting");
}

#[tokio::test]
async fn move_command_is_rejected_while_workspace_list_is_pending() {
    let (mut renderer, mut commands) = workspace_renderer();
    renderer.initial_render().unwrap();

    type_string(&mut renderer, "/move").await;
    press_enter(&mut renderer).await;
    drain(&mut commands);

    type_string(&mut renderer, "/move").await;
    press_enter(&mut renderer).await;

    assert_buffer_contains(
        renderer.writer(),
        "[wisp] Cannot move workspaces while another workspace move is in progress.",
    );
    assert_buffer_contains(renderer.writer(), "> /move");
    assert!(drain(&mut commands).is_empty(), "no second list command should be sent");
}

fn workspace_renderer() -> (Renderer, UnboundedReceiver<PromptCommand>) {
    let terminal = TestTerminal::new(TEST_WIDTH, 40);
    Renderer::new_recording_with_session_capabilities(
        terminal,
        TEST_AGENT.to_string(),
        &[],
        workspace_session_capabilities(),
        (TEST_WIDTH, 40),
    )
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
