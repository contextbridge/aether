use super::support::*;

#[test]
fn first_frame_never_inserts_scrollback() {
    let (mut app, _command_rx) = make_app();
    let mut terminal = Terminal::with_options(
        RecordingBackend::new(40, 15),
        TerminalOptions { viewport: Viewport::Inline(inline_viewport_height(15)) },
    )
    .unwrap();
    let mut renderer = Renderer::new(&UiSettings::default());

    renderer.draw(&mut terminal, &mut app).unwrap();

    let events = &terminal.backend().events;
    assert_eq!(
        events.iter().filter(|event| **event == BackendEvent::Scroll).count(),
        0,
        "an empty first frame must not insert scrollback: {events:?}"
    );
    assert_eq!(
        events.iter().filter(|event| **event == BackendEvent::ShowCursor).count(),
        1,
        "the first frame must still draw the viewport exactly once: {events:?}"
    );
}

#[test]
fn resize_growth_inserts_pending_history_before_the_viewport_draws() {
    let (mut app, _command_rx) = make_app();
    let mut terminal = Terminal::with_options(
        RecordingBackend::new(40, 15),
        TerminalOptions { viewport: Viewport::Inline(inline_viewport_height(15)) },
    )
    .unwrap();
    let mut renderer = Renderer::new(&UiSettings::default());
    renderer.draw(&mut terminal, &mut app).unwrap();

    // Shrunk below the scrollback reserve, the finished reply stays queued
    // instead of overflowing into the terminal's own scrollback.
    terminal.backend_mut().resize(40, 10);
    submit_prompt(&mut app, "hello");
    let mut reply = String::new();
    for index in 0..30 {
        writeln!(reply, "overflow-line-{index}").unwrap();
        reply.push('\n');
    }
    app.on_acp_event(text_chunk(&reply));
    app.on_acp_event(AcpEvent::PromptDone(acp::StopReason::EndTurn));
    renderer.draw(&mut terminal, &mut app).unwrap();
    assert!(!app.pending_items().is_empty(), "precondition: the finished reply must still be pending");
    assert!(!buffer_text(terminal.backend().scrollback()).contains("overflow-line-0"));

    terminal.backend_mut().events.clear();

    terminal.backend_mut().resize(40, 15);
    renderer.draw(&mut terminal, &mut app).unwrap();

    let events = &terminal.backend().events;
    let draw = events.iter().position(|event| *event == BackendEvent::ShowCursor).expect("the viewport must be drawn");
    let insert = events.iter().position(|event| *event == BackendEvent::Scroll).expect("history must be inserted");
    assert!(insert < draw, "expected history insertion before the viewport draw: {events:?}");
    assert_eq!(
        events.iter().filter(|event| **event == BackendEvent::ShowCursor).count(),
        1,
        "the frame should be drawn once, not once per side of the insertion: {events:?}"
    );
    assert!(app.pending_items().is_empty());
    assert!(buffer_text(terminal.backend().scrollback()).contains("overflow-line-0"));
}

#[test]
fn redrawing_without_state_change_preserves_the_visible_rows() {
    let (mut app, _command_rx) = make_app();
    let mut ui = TestUi::new(make_terminal());
    submit_prompt(&mut app, "idempotent");
    app.on_acp_event(text_chunk("streaming reply"));
    ui.draw(&mut app);

    let viewport: Vec<String> = ui.viewport_text().lines().map(str::to_string).collect();
    let conversation: Vec<String> = ui.conversation_text().lines().map(str::to_string).collect();
    ui.draw(&mut app);
    ui.assert_viewport(&viewport);
    ui.assert_conversation(&conversation);
}

#[test]
fn completed_turn_settles_into_a_stable_viewport() {
    let (mut app, _command_rx) = make_app();
    let mut ui = TestUi::with_dimensions(40, 15);
    submit_prompt(&mut app, "hi");
    app.on_acp_event(text_chunk("final answer"));
    app.on_acp_event(AcpEvent::PromptDone(acp::StopReason::EndTurn));
    ui.draw(&mut app);
    ui.draw(&mut app);

    let rows = [
        "",
        "  hi",
        "",
        "",
        "  final answer",
        "",
        "",
        "",
        "",
        "────────────────────────────────────────",
        ">",
        "────────────────────────────────────────",
        "  ~/code/demo · main              aether",
    ];
    ui.assert_viewport(&rows);
    ui.assert_conversation(&rows);
}

#[test]
fn scoped_buffers_partition_the_committed_and_live_conversation() {
    let (mut app, _command_rx) = make_app();
    let mut ui = TestUi::new(make_terminal());
    submit_prompt(&mut app, "hello");
    let mut reply = String::new();
    for index in 0..30 {
        writeln!(reply, "overflow-line-{index}").unwrap();
        reply.push('\n');
    }
    app.on_acp_event(text_chunk(&format!("{reply}still-streaming")));
    ui.draw(&mut app);

    ui.assert_history_contains("overflow-line-0");
    ui.assert_history_not_contains("still-streaming");
    ui.assert_viewport_contains("still-streaming");
    ui.assert_viewport_not_contains("overflow-line-0");
    let conversation = ui.conversation_text();
    assert!(conversation.find("overflow-line-0").unwrap() < conversation.find("still-streaming").unwrap());
    ui.assert_conversation_contains("overflow-line-29");
    ui.assert_conversation_not_contains("never-rendered");
}

#[test]
fn resize_growth_commits_history_that_the_small_viewport_queued() {
    let (mut app, _command_rx) = make_app();
    let mut ui = TestUi::new(make_terminal());
    ui.draw(&mut app);

    ui.resize(40, 10);
    submit_prompt(&mut app, "hello");
    let mut reply = String::new();
    for index in 0..30 {
        writeln!(reply, "queued-line-{index}").unwrap();
        reply.push('\n');
    }
    app.on_acp_event(text_chunk(&reply));
    app.on_acp_event(AcpEvent::PromptDone(acp::StopReason::EndTurn));
    ui.draw(&mut app);

    ui.assert_history_not_contains("queued-line-0");
    assert!(!app.pending_items().is_empty(), "precondition: without scrollback room the reply stays queued");

    ui.resize(40, 15);
    ui.draw(&mut app);

    ui.assert_history_contains("queued-line-0");
    assert!(app.pending_items().is_empty());

    let mut expected_history = vec![String::new(); 4];
    expected_history[3] = "  hello".to_string();
    expected_history.push(String::new());
    expected_history.push(String::new());
    for index in 0..26 {
        expected_history.push(format!("  queued-line-{index}"));
        expected_history.push(String::new());
    }
    expected_history.pop();
    ui.assert_history(&expected_history);
}

#[test]
#[should_panic(expected = "line 0 mismatch")]
fn assert_buffer_eq_reports_the_first_mismatched_line() {
    let mut buffer = Buffer::empty(ratatui::layout::Rect::new(0, 0, 3, 1));
    buffer.set_string(0, 0, "abc", ratatui::style::Style::default());
    assert_buffer_eq(&buffer, &["abd"]);
}

#[test]
fn assert_buffer_eq_ignores_styles_and_trailing_spaces() {
    let mut buffer = Buffer::empty(ratatui::layout::Rect::new(0, 0, 5, 2));
    buffer.set_string(0, 0, "abc", ratatui::style::Style::default().fg(Color::Red).add_modifier(Modifier::BOLD));
    assert_buffer_eq(&buffer, &["abc", ""]);
}
