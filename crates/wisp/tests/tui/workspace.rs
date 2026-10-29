use super::support::*;

#[test]
fn remote_workspace_move_keeps_paths_server_side() {
    let mut ui =
        TestUiBuilder::new().session(|session| session.remote_workspace().workspace_move()).dimensions(120, 24).build();
    open_workspace_picker(&mut ui, vec![workspace_entry("/server/next", false)]);
    ui.assert_viewport_contains("/server/next");
    ui.key(key(KeyCode::Enter));
    workspace_moved(&mut ui, "/server/next");
    session_loaded(&mut ui, Vec::new());
    assert_eq!(
        ui.app().core().session().workspace_status().display_dir,
        "/server/next",
        "a workspace move shows the agent's status of the workspace it moved to"
    );
    ui.assert_viewport_contains("/server/next");
}

#[test]
fn workspace_move_command_hidden_without_capability() {
    let mut ui = TestUi::new();

    ui.key(key(KeyCode::Char('/')));
    assert!(ui.app().composer().has_completion());

    ui.assert_viewport_contains("/clear");
    ui.assert_viewport_not_contains("/move");
}

#[test]
fn workspace_move_command_visible_with_capability() {
    let mut ui = TestUiBuilder::new().session(SessionCoreConfigBuilder::workspace_move).build();

    ui.key(key(KeyCode::Char('/')));
    assert!(ui.app().composer().has_completion());

    ui.assert_viewport_contains("/move");
}

#[test]
fn workspace_picker_opens_with_existing_workspaces() {
    let mut ui = TestUiBuilder::new().session(SessionCoreConfigBuilder::workspace_move).build();

    open_workspace_picker(
        &mut ui,
        vec![
            workspace_entry("/home/user/code/current", true),
            workspace_entry("/home/user/code/other", false),
            workspace_entry("/tmp/sandbox", false),
        ],
    );
    assert!(ui.app().has_modal());

    ui.assert_viewport_contains("/home/user/code/other");
    ui.assert_viewport_contains("/tmp/sandbox");
    ui.assert_viewport_not_contains("/home/user/code/current");
    ui.assert_viewport_contains("Create new workspace");
}

#[test]
fn workspace_picker_shows_empty_state_when_no_workspaces() {
    let mut ui = TestUiBuilder::new().session(SessionCoreConfigBuilder::workspace_move).build();

    open_workspace_picker(&mut ui, vec![workspace_entry("/home/user/code/current", true)]);
    assert!(ui.app().has_modal());

    ui.assert_viewport_not_contains("No other workspaces available");
}

#[test]
fn workspace_picker_esc_closes_and_resets_state() {
    let mut ui = TestUiBuilder::new().session(SessionCoreConfigBuilder::workspace_move).build();

    open_workspace_picker(
        &mut ui,
        vec![workspace_entry("/home/user/code/current", true), workspace_entry("/home/user/code/other", false)],
    );
    assert!(ui.app().has_modal());

    ui.key(key(KeyCode::Esc));
    assert!(matches!(ui.app().core().foreground(), ForegroundOperation::Idle));
    assert!(!ui.app().has_modal());
}

#[test]
fn workspace_picker_enter_selects_existing_workspace() {
    let mut ui = TestUiBuilder::new().session(SessionCoreConfigBuilder::workspace_move).build();

    move_to_other_workspace(&mut ui);

    assert!(!ui.app().has_modal());
    let (request, _) = ui.held::<MoveWorkspace>();
    assert_eq!(request.target, WorkspaceMoveTarget::Existing { path: "/home/user/code/other".into() });
}

#[test]
fn workspace_picker_enter_selects_create_new_and_shows_naming_mode() {
    let mut ui = TestUiBuilder::new().session(SessionCoreConfigBuilder::workspace_move).build();

    open_workspace_picker(&mut ui, vec![workspace_entry("/home/user/code/current", true)]);

    ui.assert_viewport_contains("Create new workspace");

    ui.key(key(KeyCode::Enter));
    ui.assert_viewport_contains("New workspace");
}

#[test]
fn workspace_name_prompt_owns_the_terminal_cursor() {
    let mut ui = TestUiBuilder::new().session(SessionCoreConfigBuilder::workspace_move).build();
    open_workspace_picker(&mut ui, vec![workspace_entry("/home/user/code/current", true)]);
    ui.key(key(KeyCode::Enter));

    ui.type_text("proj");
    ui.draw();

    let buffer = ui.backend().buffer();
    let row = row_containing(buffer, "Name: proj").expect("name prompt");
    let text = row_text(buffer, row);
    let column = text[..text.find("Name: proj").unwrap()].width() + "Name: proj".width();
    assert!(ui.backend().cursor_visible(), "the name prompt owns the cursor");
    assert_eq!(ui.backend().cursor_position(), Position::new(u16::try_from(column).unwrap(), row));
}

#[test]
fn workspace_name_scrolls_unicode_text_and_keeps_cursor_visible() {
    let mut ui = TestUiBuilder::new().session(SessionCoreConfigBuilder::workspace_move).dimensions(24, 20).build();
    open_workspace_picker(&mut ui, vec![workspace_entry("/workspace/current", true)]);
    ui.key(key(KeyCode::Enter));
    ui.type_text("a界bcdefghijklmnopqrstuvwxyz");
    ui.draw();

    let buffer = ui.backend().buffer();
    let row = row_containing(buffer, "Name:").expect("name prompt");
    let text = row_text(buffer, row);
    assert!(text.contains("xyz"), "{text}");
    assert!(!text.contains("a界"), "the input should scroll: {text}");
    let column = text[..text.find("xyz").unwrap()].width() + 3;
    assert!(ui.backend().cursor_visible());
    assert_eq!(ui.backend().cursor_position(), Position::new(u16::try_from(column).unwrap(), row));

    ui.key(key(KeyCode::Home));
    ui.key(key(KeyCode::Right));
    ui.key(key(KeyCode::Right));
    ui.draw();
    let text = row_text(ui.backend().buffer(), row);
    assert!(text.contains("Name: a界"), "{text}");
    let column = text[..text.find("a界").unwrap()].width() + 3;
    assert_eq!(ui.backend().cursor_position(), Position::new(u16::try_from(column).unwrap(), row));
}

#[test]
fn workspace_naming_new_esc_returns_to_list_mode() {
    let mut ui = TestUiBuilder::new().session(SessionCoreConfigBuilder::workspace_move).build();

    open_workspace_picker(&mut ui, vec![workspace_entry("/home/user/code/current", true)]);

    ui.key(key(KeyCode::Enter));
    ui.assert_viewport_contains("New workspace");

    ui.key(key(KeyCode::Esc));
    assert!(ui.app().has_modal());
    ui.assert_viewport_contains("Create new workspace");
}

#[test]
fn workspace_naming_new_enter_with_name_emits_move_target() {
    let mut ui = TestUiBuilder::new().session(SessionCoreConfigBuilder::workspace_move).build();

    open_workspace_picker(&mut ui, vec![workspace_entry("/home/user/code/current", true)]);

    ui.key(key(KeyCode::Enter));
    ui.paste("my-new-workspace");
    assert!(ui.app().composer().text().is_empty(), "paste must belong to the workspace editor");
    ui.key(key(KeyCode::Enter));

    assert!(matches!(ui.app().core().foreground(), ForegroundOperation::MovingWorkspace));
    assert!(!ui.app().has_modal());

    let (request, _) = ui.held::<MoveWorkspace>();
    assert_eq!(request.session_id, "test-session");
    assert_eq!(request.target, WorkspaceMoveTarget::New { name: "my-new-workspace".into() });
}

#[test]
fn workspace_picker_filtering_hides_non_matching() {
    let mut ui = TestUiBuilder::new().session(SessionCoreConfigBuilder::workspace_move).build();

    open_workspace_picker(
        &mut ui,
        vec![
            workspace_entry("/home/user/code/current", true),
            workspace_entry("/home/user/code/project-a", false),
            workspace_entry("/tmp/test", false),
        ],
    );

    ui.assert_viewport_contains("project-a");
    ui.assert_viewport_contains("/tmp/test");

    // Use a query that only matches one entry
    ui.key(key(KeyCode::Char('j')));
    ui.assert_viewport_contains("project-a");
    ui.assert_viewport_not_contains("/tmp/test");
    ui.assert_viewport_not_contains("Create new");
}

#[test]
fn workspace_picker_renders_on_narrow_terminal() {
    let mut ui = TestUiBuilder::new().session(SessionCoreConfigBuilder::workspace_move).build();

    open_workspace_picker(
        &mut ui,
        vec![workspace_entry("/home/user/code/current", true), workspace_entry("/home/user/code/project-a", false)],
    );

    ui.assert_viewport_contains("project-a");
}
