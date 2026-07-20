use super::support::*;

#[test]
fn first_frame_never_inserts_scrollback() {
    let mut ui = TestUi::with_backend(RecordingBackend::new(40, 15));

    ui.draw();

    let events: Vec<BackendEvent> = ui.backend().events().to_vec();
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
fn resize_growth_preserves_native_history_without_duplication() {
    let mut ui = TestUi::with_backend(RecordingBackend::new(40, 15));
    ui.draw();

    // Conversation items remain owned by the model while the renderer commits
    // only the visible overflow into terminal-native history.
    ui.backend_mut().resize(40, 10);
    ui.submit("hello");
    let mut reply = String::new();
    for index in 0..30 {
        writeln!(reply, "overflow-line-{index}").unwrap();
        reply.push('\n');
    }
    ui.acp_event(text_chunk(&reply));
    ui.acp_event(AcpEvent::PromptDone(acp::StopReason::EndTurn));
    ui.draw();
    assert!(
        ui.app()
            .conversation_items()
            .iter()
            .any(|item| item.text().is_some_and(|text| text.contains("overflow-line-0"))),
        "precondition: the finished reply must be retained"
    );
    assert!(buffer_text(ui.backend().scrollback()).contains("overflow-line-0"));

    ui.backend_mut().clear_events();

    ui.backend_mut().resize(40, 15);
    ui.draw();

    let events: Vec<BackendEvent> = ui.backend().events().to_vec();
    assert!(events.iter().any(|event| *event == BackendEvent::ShowCursor), "the viewport must be drawn: {events:?}");
    assert!(
        !events.iter().any(|event| *event == BackendEvent::Scroll),
        "resize should not duplicate committed history: {events:?}"
    );
    assert_eq!(
        events.iter().filter(|event| **event == BackendEvent::ShowCursor).count(),
        1,
        "the frame should be drawn once, not once per side of the insertion: {events:?}"
    );
    assert!(
        ui.app()
            .conversation_items()
            .iter()
            .any(|item| item.text().is_some_and(|text| text.contains("overflow-line-0")))
    );
    assert!(buffer_text(ui.backend().scrollback()).contains("overflow-line-0"));
}

#[test]
fn repeated_draws_do_not_mutate_application_state() {
    let mut ui = TestUi::new();
    ui.submit("immutable");
    ui.acp_event(text_chunk("streaming reply"));

    let before_items: Vec<String> =
        ui.app().conversation_items().iter().map(|item| item.text().unwrap_or_default().to_string()).collect();
    let before_composer = ui.app().composer().text().to_string();
    let before_waiting = ui.app().waiting_for_response();

    ui.draw();
    ui.draw();

    let after_items: Vec<String> =
        ui.app().conversation_items().iter().map(|item| item.text().unwrap_or_default().to_string()).collect();
    assert_eq!(after_items, before_items);
    assert_eq!(ui.app().composer().text(), before_composer);
    assert_eq!(ui.app().waiting_for_response(), before_waiting);
}

#[test]
fn redrawing_without_state_change_preserves_the_visible_rows() {
    let mut ui = TestUi::new();
    ui.submit("idempotent");
    ui.acp_event(text_chunk("streaming reply"));
    ui.draw();

    let viewport: Vec<String> = ui.viewport_text().lines().map(str::to_string).collect();
    let conversation: Vec<String> = ui.conversation_text().lines().map(str::to_string).collect();
    ui.draw();
    ui.assert_viewport(&viewport);
    ui.assert_conversation(&conversation);
}

#[test]
fn completed_turn_settles_into_a_stable_viewport() {
    let mut ui = TestUi::with_dimensions(40, 15);
    ui.submit("hi");
    ui.acp_event(text_chunk("final answer"));
    ui.acp_event(AcpEvent::PromptDone(acp::StopReason::EndTurn));
    ui.draw();
    ui.draw();

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
    let mut ui = TestUi::new();
    ui.submit("hello");
    let mut reply = String::new();
    for index in 0..30 {
        writeln!(reply, "overflow-line-{index}").unwrap();
        reply.push('\n');
    }
    ui.acp_event(text_chunk(&format!("{reply}still-streaming")));
    ui.draw();

    assert!(buffer_text(ui.backend().scrollback()).contains("overflow-line-0"));
    ui.assert_history_not_contains("still-streaming");
    ui.assert_viewport_contains("still-streaming");
    ui.assert_viewport_not_contains("overflow-line-0");
    let conversation = ui.conversation_text();
    assert!(conversation.find("overflow-line-0").unwrap() < conversation.find("still-streaming").unwrap());
    ui.assert_conversation_contains("overflow-line-29");
    ui.assert_conversation_not_contains("never-rendered");
}

#[test]
fn resize_growth_keeps_canonical_history_visible() {
    let mut ui = TestUi::new();
    ui.draw();

    ui.resize(40, 10);
    ui.submit("hello");
    let mut reply = String::new();
    for index in 0..30 {
        writeln!(reply, "queued-line-{index}").unwrap();
        reply.push('\n');
    }
    ui.acp_event(text_chunk(&reply));
    ui.acp_event(AcpEvent::PromptDone(acp::StopReason::EndTurn));
    ui.draw();

    ui.assert_history_contains("queued-line-0");
    assert!(
        ui.app().conversation_items().iter().any(|item| item.text().is_some_and(|text| text.contains("queued-line-0"))),
        "precondition: the finished reply must be retained"
    );

    ui.resize(40, 15);
    ui.draw();

    ui.assert_history_contains("queued-line-0");
    assert!(
        ui.app().conversation_items().iter().any(|item| item.text().is_some_and(|text| text.contains("queued-line-0")))
    );
    ui.assert_conversation_contains("queued-line-29");
    assert_eq!(ui.history_text().matches("queued-line-0").count(), 1);
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
