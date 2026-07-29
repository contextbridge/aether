use super::support::*;

fn purge_count(app: &mut App) -> usize {
    let mut count = 0;
    while let Some(effect) = app.take_effect() {
        if matches!(effect, RuntimeEffect::PurgeScrollback) {
            count += 1;
        }
    }
    count
}

fn assert_command(rx: &mut UnboundedReceiver<PromptCommand>, expected: impl Fn(&PromptCommand) -> bool, label: &str) {
    let command = rx.try_recv().unwrap_or_else(|_| panic!("{label} should have sent a command"));
    assert!(expected(&command), "{label} sent {command:?}");
}

fn commit_overflowing_reply(
    app: &mut App,
    command_rx: &mut UnboundedReceiver<PromptCommand>,
    terminal: &mut Terminal<TestBackend>,
    presenter: &mut Presenter,
) {
    submit_prompt(app, "talk at length");
    assert_command(command_rx, |c| matches!(c, PromptCommand::Prompt { .. }), "the overflowing reply");
    let mut reply = String::new();
    for index in 0..30 {
        writeln!(reply, "overflow-line-{index}").unwrap();
        reply.push('\n');
    }
    app.on_acp_event(text_chunk(&format!("{reply}still streaming")));
    sync_terminal_with_renderer(terminal, app, presenter).unwrap();
    assert!(
        buffer_text(&history_buffer(terminal)).contains("overflow-line-0"),
        "precondition: a prior reply must have been committed to scrollback"
    );
    // Complete the turn so the app is idle and later commands (e.g. /move) are not blocked.
    app.on_acp_event(AcpEvent::PromptDone(acp::StopReason::EndTurn));
}

#[test]
fn purges_once_on_context_clear() {
    let (mut app, mut command_rx) = make_app();
    let mut terminal = make_terminal();
    let mut presenter = Presenter::new(&UiSettings::default());

    commit_overflowing_reply(&mut app, &mut command_rx, &mut terminal, &mut presenter);

    app.on_acp_event(AcpEvent::ContextCleared(ContextClearedParams::default()));

    assert_eq!(purge_count(&mut app), 1, "a context clear should purge native scrollback exactly once");
}

#[test]
fn clear_command_purges_only_when_the_new_session_lands() {
    let (mut app, mut command_rx) = make_app();
    let mut terminal = make_terminal();
    let mut presenter = Presenter::new(&UiSettings::default());

    commit_overflowing_reply(&mut app, &mut command_rx, &mut terminal, &mut presenter);

    type_text(&mut app, "/clear");
    app.on_key(key(KeyCode::Tab));
    assert_command(&mut command_rx, |c| matches!(c, PromptCommand::NewSession { .. }), "/clear");
    assert_eq!(purge_count(&mut app), 0, "requesting a new session must not purge before it lands");

    app.on_acp_event(new_session_created("fresh-session", Vec::new()));

    assert_eq!(purge_count(&mut app), 1, "a landed new session should purge native scrollback exactly once");
}

#[test]
fn session_switch_purges_once_and_the_loaded_landing_does_not_purge_again() {
    let (mut app, mut command_rx) = make_app();
    let mut terminal = make_terminal();
    let mut presenter = Presenter::new(&UiSettings::default());

    commit_overflowing_reply(&mut app, &mut command_rx, &mut terminal, &mut presenter);

    type_text(&mut app, "/resume");
    app.on_key(key(KeyCode::Tab));
    assert_command(&mut command_rx, |c| matches!(c, PromptCommand::ListSessions), "/resume");
    app.on_acp_event(sessions_listed(vec![session_info("other", "/tmp/elsewhere", "Other", "2025-01-01T00:00:00Z")]));
    app.on_key(key(KeyCode::Enter));
    assert_command(&mut command_rx, |c| matches!(c, PromptCommand::LoadSession { .. }), "resume");

    assert_eq!(purge_count(&mut app), 1, "requesting the session load should purge once");

    app.on_acp_event(session_loaded("other", Vec::new()));

    assert_eq!(purge_count(&mut app), 0, "the session landing must not purge a second time");
}

#[test]
fn successful_workspace_move_purges_once() {
    let (mut app, mut command_rx) = make_app_with_workspace_move();
    let mut terminal = make_terminal();
    let mut presenter = Presenter::new(&UiSettings::default());

    commit_overflowing_reply(&mut app, &mut command_rx, &mut terminal, &mut presenter);

    type_text(&mut app, "/move");
    app.on_key(key(KeyCode::Tab));
    assert_command(&mut command_rx, |c| matches!(c, PromptCommand::ListWorkspaces(_)), "/move");
    app.on_acp_event(workspaces_listed(vec![
        workspace_entry("/home/user/code/current", true),
        workspace_entry("/home/user/code/other", false),
    ]));
    app.on_key(key(KeyCode::Enter));
    assert_command(&mut command_rx, |c| matches!(c, PromptCommand::MoveWorkspace(_)), "move");
    app.on_acp_event(workspace_moved("/home/user/code/other"));

    assert_eq!(purge_count(&mut app), 1, "a successful workspace move should purge native scrollback exactly once");
}

/// The purge is unconditional: even a reset with no committed content must clear
/// scrollback a prior app instance may have left behind.
#[test]
fn resets_purge_even_with_no_committed_content() {
    let (mut app, _command_rx) = make_app();

    app.on_acp_event(AcpEvent::ContextCleared(ContextClearedParams::default()));

    assert_eq!(purge_count(&mut app), 1, "a reset should purge native scrollback even with no committed content");
}

#[test]
fn an_ordinary_render_does_not_purge() {
    let (mut app, mut command_rx) = make_app();
    let mut terminal = make_terminal();
    let mut presenter = Presenter::new(&UiSettings::default());

    commit_overflowing_reply(&mut app, &mut command_rx, &mut terminal, &mut presenter);
    sync_terminal_with_renderer(&mut terminal, &mut app, &mut presenter).unwrap();

    assert_eq!(purge_count(&mut app), 0, "an ordinary render must not purge native scrollback");
}

#[test]
fn a_new_session_request_that_fails_does_not_purge() {
    let (mut app, fail_signal, mut command_rx) = AppBuilder::new().build_failable();

    fail_signal.store(true, Ordering::Relaxed);
    type_text(&mut app, "/clear");
    app.on_key(key(KeyCode::Tab));
    assert!(command_rx.try_recv().is_err(), "the new-session send should have failed");

    assert_eq!(purge_count(&mut app), 0, "a rejected new-session request must not purge");
}

#[test]
fn a_workspace_listing_failure_does_not_purge() {
    let (mut app, mut command_rx) = make_app_with_workspace_move();

    type_text(&mut app, "/move");
    app.on_key(key(KeyCode::Tab));
    assert_command(&mut command_rx, |c| matches!(c, PromptCommand::ListWorkspaces(_)), "/move");

    app.on_acp_event(workspace_list_failed("network error"));

    assert_eq!(purge_count(&mut app), 0, "a failed workspace listing must not purge");
}

#[test]
fn a_workspace_move_failure_does_not_purge() {
    let (mut app, mut command_rx) = make_app_with_workspace_move();

    type_text(&mut app, "/move");
    app.on_key(key(KeyCode::Tab));
    assert_command(&mut command_rx, |c| matches!(c, PromptCommand::ListWorkspaces(_)), "/move");
    app.on_acp_event(workspaces_listed(vec![
        workspace_entry("/home/user/code/current", true),
        workspace_entry("/home/user/code/other", false),
    ]));
    app.on_key(key(KeyCode::Enter));
    assert_command(&mut command_rx, |c| matches!(c, PromptCommand::MoveWorkspace(_)), "move");

    app.on_acp_event(workspace_move_failed("permission denied"));

    assert_eq!(purge_count(&mut app), 0, "a failed workspace move must not purge");
}
