use super::support::*;

#[test]
fn submit_sends_prompt_and_clears_composer() {
    let (mut app, mut command_rx) = make_app();

    submit_prompt(&mut app, "hello agent");

    match command_rx.try_recv().unwrap() {
        PromptCommand::Prompt { session_id, text, content } => {
            assert_eq!(session_id.0.as_ref(), "test-session");
            assert_eq!(text, "hello agent");
            assert!(content.is_none());
        }
        other => panic!("expected Prompt command, got {other:?}"),
    }
    assert!(app.composer().is_empty());
}

#[test]
fn submit_is_ignored_while_prompt_in_flight() {
    let (mut app, mut command_rx) = make_app();
    submit_prompt(&mut app, "first");
    command_rx.try_recv().unwrap();

    submit_prompt(&mut app, "second");

    assert!(command_rx.try_recv().is_err());
    assert_eq!(app.composer().text(), "second");
}

#[test]
fn esc_cancels_only_while_busy() {
    let (mut app, mut command_rx) = make_app();

    app.on_key(key(KeyCode::Esc));
    assert!(command_rx.try_recv().is_err());

    submit_prompt(&mut app, "work");
    command_rx.try_recv().unwrap();
    app.on_key(key(KeyCode::Esc));

    assert!(matches!(command_rx.try_recv().unwrap(), PromptCommand::Cancel { .. }));
}

#[test]
fn double_ctrl_c_exits_and_first_press_clears_composer() {
    let (mut app, _command_rx) = make_app();
    type_text(&mut app, "draft");

    app.on_key(ctrl('c'));
    assert!(!app.exit_requested());
    assert!(app.composer().is_empty());
    assert!(app.exit_confirmation_active());

    app.on_key(ctrl('c'));
    assert!(app.exit_requested());
}

#[test]
fn ctrl_c_confirmation_disarms_after_window() {
    let (mut app, _command_rx) = make_app();

    app.on_key(ctrl('c'));
    app.on_tick(Instant::now() + Duration::from_secs(2));
    assert!(!app.exit_confirmation_active());

    app.on_key(ctrl('c'));
    assert!(!app.exit_requested());
}

#[test]
fn connection_closed_requests_exit() {
    let (mut app, _command_rx) = make_app();

    app.on_acp_event(AcpEvent::ConnectionClosed);

    assert!(app.exit_requested());
}

#[test]
fn alt_enter_inserts_newline_instead_of_submitting() {
    let (mut app, mut command_rx) = make_app();
    type_text(&mut app, "line one");

    app.on_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::ALT));
    type_text(&mut app, "line two");

    assert!(command_rx.try_recv().is_err());
    assert_eq!(app.composer().text(), "line one\nline two");
}

#[test]
fn context_clear_discards_conversation_retained_in_the_live_viewport() {
    let (mut app, _command_rx) = make_app();
    let mut terminal = make_terminal();
    let mut renderer = Presenter::new(&UiSettings::default());
    submit_prompt(&mut app, "old retained message");
    sync_terminal_with_renderer(&mut terminal, &mut app, &mut renderer).unwrap();
    assert!(buffer_text(&viewport_buffer(&mut terminal)).contains("old retained message"));

    app.on_acp_event(AcpEvent::ContextCleared(ContextClearedParams::default()));
    sync_terminal_with_renderer(&mut terminal, &mut app, &mut renderer).unwrap();

    let viewport = buffer_text(&viewport_buffer(&mut terminal));
    assert!(!viewport.contains("old retained message"), "{viewport}");
    assert!(viewport.contains("[wisp-next] Context cleared"), "{viewport}");
}

#[test]
fn fitting_user_message_remains_in_the_live_viewport() {
    let (mut app, _command_rx) = make_app();
    let mut terminal = make_terminal();
    let mut renderer = Presenter::new(&UiSettings::default());

    submit_prompt(&mut app, "hello viewport");
    sync_terminal_with_renderer(&mut terminal, &mut app, &mut renderer).unwrap();

    assert!(buffer_text(&viewport_buffer(&mut terminal)).contains("hello viewport"));
    assert!(!buffer_text(&history_buffer(&mut terminal)).contains("hello viewport"));
}

#[test]
fn completed_stream_lines_remain_live_until_they_overflow() {
    let (mut app, _command_rx) = make_app();
    let mut terminal = make_terminal();
    let mut renderer = Presenter::new(&UiSettings::default());
    submit_prompt(&mut app, "hi");

    let mut completed = String::new();
    for index in 0..20 {
        writeln!(completed, "line-{index}\n").unwrap();
    }
    app.on_acp_event(text_chunk(&format!("{completed}partial")));
    sync_terminal_with_renderer(&mut terminal, &mut app, &mut renderer).unwrap();

    let scrollback = buffer_text(&history_buffer(&mut terminal));
    let viewport = buffer_text(&viewport_buffer(&mut terminal));
    assert!(scrollback.contains("line-0"));
    assert!(!scrollback.contains("partial"));
    assert!(viewport.contains("line-19"));
    assert!(viewport.contains("partial"));
}

#[test]
fn completed_streaming_text_remains_adjacent_to_the_composer_once() {
    let (mut app, _command_rx) = make_app();
    let mut terminal = make_terminal();
    let mut renderer = Presenter::new(&UiSettings::default());
    submit_prompt(&mut app, "hi");

    app.on_acp_event(text_chunk("streamed "));
    app.on_acp_event(text_chunk("answer"));
    sync_terminal_with_renderer(&mut terminal, &mut app, &mut renderer).unwrap();

    assert!(buffer_text(&viewport_buffer(&mut terminal)).contains("streamed answer"));
    assert!(!buffer_text(&history_buffer(&mut terminal)).contains("streamed answer"));

    app.on_acp_event(AcpEvent::PromptDone(acp::StopReason::EndTurn));
    sync_terminal_with_renderer(&mut terminal, &mut app, &mut renderer).unwrap();
    sync_terminal_with_renderer(&mut terminal, &mut app, &mut renderer).unwrap();

    let conversation = buffer_text(&conversation_buffer(&mut terminal));
    assert_eq!(conversation.matches("streamed answer").count(), 1);
    assert!(buffer_text(&viewport_buffer(&mut terminal)).contains("streamed answer"));
}

#[test]
fn streamed_markdown_blocks_keep_blank_separators() {
    let (mut app, _command_rx) = make_app();
    let mut terminal = make_terminal_tall();
    let mut renderer = Presenter::new(&UiSettings::default());
    submit_prompt(&mut app, "format this");

    for chunk in ["### Root\n", "\n", "Keep this paragraph separate.\n", "\n", "- first item\n"] {
        app.on_acp_event(text_chunk(chunk));
    }
    app.on_acp_event(AcpEvent::PromptDone(acp::StopReason::EndTurn));
    sync_terminal_with_renderer(&mut terminal, &mut app, &mut renderer).unwrap();

    let conversation = conversation_buffer(&mut terminal);
    let heading = row_containing(&conversation, "### Root").expect("heading should render");
    let paragraph = row_containing(&conversation, "Keep this paragraph separate.").expect("paragraph should render");
    let item = row_containing(&conversation, "first item").expect("list item should render");

    assert_eq!(paragraph - heading, 2, "heading and paragraph need one blank row");
    assert_eq!(item - paragraph, 2, "paragraph and list need one blank row");
}

#[test]
fn running_tool_holds_later_content_out_of_committed_history() {
    let (mut app, _command_rx) = make_app();
    let mut terminal = make_terminal();
    let mut renderer = Presenter::new(&UiSettings::default());
    submit_prompt(&mut app, "run a tool");

    app.on_acp_event(tool_call("tool-1", "Reading main.rs"));
    app.on_acp_event(text_chunk("tool output summary"));
    sync_terminal_with_renderer(&mut terminal, &mut app, &mut renderer).unwrap();

    let viewport = buffer_text(&viewport_buffer(&mut terminal));
    assert!(viewport.contains("Reading main.rs"));
    assert!(!buffer_text(&history_buffer(&mut terminal)).contains("Reading main.rs"));

    app.on_acp_event(tool_completed("tool-1"));
    app.on_acp_event(AcpEvent::PromptDone(acp::StopReason::EndTurn));
    sync_terminal_with_renderer(&mut terminal, &mut app, &mut renderer).unwrap();

    let conversation = buffer_text(&conversation_buffer(&mut terminal));
    assert!(conversation.contains("Reading main.rs"));
    assert!(conversation.contains("tool output summary"));
    assert!(app.pending_items().is_empty());
}

#[test]
fn cancelled_prompt_marks_running_tool_as_error() {
    let (mut app, _command_rx) = make_app();
    submit_prompt(&mut app, "run a tool");
    app.on_acp_event(tool_call("tool-1", "Slow tool"));

    app.on_acp_event(AcpEvent::PromptDone(acp::StopReason::Cancelled));

    let items = app.drain_finalized();
    let cancelled = items
        .iter()
        .any(|item| matches!(item, HistoryItem::Tool { status: ToolStatus::Error(cause), .. } if cause == "cancelled"));
    assert!(cancelled, "expected cancelled tool in {items:?}");
}

fn tool_call_with_raw(id: &str, title: &str, raw_input: serde_json::Value) -> AcpEvent {
    session_update(acp::SessionUpdate::ToolCall(acp::ToolCall::new(id.to_string(), title).raw_input(raw_input)))
}

fn tool_call_update_with_raw(id: &str, raw_input: serde_json::Value) -> AcpEvent {
    session_update(acp::SessionUpdate::ToolCallUpdate(acp::ToolCallUpdate::new(
        id.to_string(),
        acp::ToolCallUpdateFields::new().raw_input(raw_input),
    )))
}

#[test]
fn streamed_raw_input_fragments_accumulate_and_show_after_completion() {
    let (mut app, _command_rx) = make_app();
    let mut terminal = make_terminal_with_width(80);
    submit_prompt(&mut app, "run tool");
    app.on_acp_event(tool_call("tool-1", "Edit file"));
    app.on_acp_event(tool_call_update_with_raw("tool-1", serde_json::Value::String("first ".to_string())));
    app.on_acp_event(tool_call_update_with_raw("tool-1", serde_json::Value::String("second".to_string())));
    app.on_acp_event(tool_completed_status("tool-1"));

    sync_terminal(&mut terminal, &mut app).unwrap();

    let viewport = buffer_text(&viewport_buffer(&mut terminal));
    assert!(viewport.contains("first second"), "streamed fragments must appear: {viewport}");
}

fn tool_call_update_with_display_value(id: &str, display_value: &str) -> AcpEvent {
    let mut meta = serde_json::Map::new();
    meta.insert("display_value".to_string(), serde_json::Value::String(display_value.to_string()));
    session_update(acp::SessionUpdate::ToolCallUpdate(
        acp::ToolCallUpdate::new(id.to_string(), acp::ToolCallUpdateFields::new()).meta(meta),
    ))
}

fn tool_completed_status(id: &str) -> AcpEvent {
    session_update(acp::SessionUpdate::ToolCallUpdate(acp::ToolCallUpdate::new(
        id.to_string(),
        acp::ToolCallUpdateFields::new().status(acp::ToolCallStatus::Completed),
    )))
}

fn tool_failed_status(id: &str) -> AcpEvent {
    session_update(acp::SessionUpdate::ToolCallUpdate(acp::ToolCallUpdate::new(
        id.to_string(),
        acp::ToolCallUpdateFields::new().status(acp::ToolCallStatus::Failed),
    )))
}

#[test]
fn running_tool_hides_raw_arguments() {
    let (mut app, _command_rx) = make_app();
    let mut terminal = make_terminal();
    submit_prompt(&mut app, "run tool");
    let raw = serde_json::json!({"path": "/src/main.rs"});
    app.on_acp_event(tool_call_with_raw("tool-1", "Read file", raw));

    sync_terminal(&mut terminal, &mut app).unwrap();

    let viewport = buffer_text(&viewport_buffer(&mut terminal));
    assert!(viewport.contains("Read file"), "title must be visible: {viewport}");
    assert!(!viewport.contains("/src/main.rs"), "raw args must be hidden while running: {viewport}");
}

#[test]
fn completed_tool_shows_raw_arguments() {
    let (mut app, _command_rx) = make_app();
    let mut terminal = make_terminal_with_width(80);
    submit_prompt(&mut app, "run tool");
    let raw = serde_json::json!({"path": "/src/main.rs"});
    app.on_acp_event(tool_call_with_raw("tool-1", "Read file", raw));
    app.on_acp_event(tool_completed_status("tool-1"));

    sync_terminal(&mut terminal, &mut app).unwrap();

    let viewport = buffer_text(&viewport_buffer(&mut terminal));
    assert!(viewport.contains("/src/main.rs"), "raw args must be visible after completion: {viewport}");
}

#[test]
fn display_value_overrides_raw_arguments_in_rendered_output() {
    let (mut app, _command_rx) = make_app();
    let mut terminal = make_terminal_with_width(80);
    submit_prompt(&mut app, "run tool");
    let raw = serde_json::json!({"path": "/src/main.rs"});
    app.on_acp_event(tool_call_with_raw("tool-1", "Read file", raw));
    app.on_acp_event(tool_call_update_with_display_value("tool-1", "42 lines read"));
    app.on_acp_event(tool_completed_status("tool-1"));

    sync_terminal(&mut terminal, &mut app).unwrap();

    let viewport = buffer_text(&viewport_buffer(&mut terminal));
    assert!(viewport.contains("42 lines read"), "display_value must be visible: {viewport}");
    assert!(!viewport.contains("/src/main.rs"), "raw args must be hidden when display_value is set: {viewport}");
}

#[test]
fn error_cause_is_visible_in_rendered_output() {
    let (mut app, _command_rx) = make_app();
    let mut terminal = make_terminal_with_width(80);
    submit_prompt(&mut app, "run tool");
    app.on_acp_event(tool_call("tool-1", "Failing tool"));
    app.on_acp_event(tool_failed_status("tool-1"));

    sync_terminal(&mut terminal, &mut app).unwrap();

    let viewport = buffer_text(&viewport_buffer(&mut terminal));
    assert!(viewport.contains("failed"), "error cause must be visible: {viewport}");
    assert!(!viewport.contains("(failed)"), "error cause must NOT have parentheses: {viewport}");
}

#[test]
fn truncation_adds_visible_ellipsis_for_long_arguments() {
    let (mut app, _command_rx) = make_app();
    let mut terminal = make_terminal_with_width(250);
    submit_prompt(&mut app, "run tool");
    let long = "x".repeat(300);
    app.on_acp_event(tool_call_with_raw("tool-1", "Long arg", serde_json::Value::String(long)));
    app.on_acp_event(tool_completed_status("tool-1"));

    sync_terminal(&mut terminal, &mut app).unwrap();

    let viewport = buffer_text(&viewport_buffer(&mut terminal));
    assert!(viewport.contains('…'), "truncated args must show ellipsis: {viewport}");
}

#[test]
fn truncation_keeps_short_arguments_unchanged() {
    let (mut app, _command_rx) = make_app();
    let mut terminal = make_terminal_with_width(250);
    submit_prompt(&mut app, "run tool");
    let short = "hello world";
    app.on_acp_event(tool_call_with_raw("tool-1", "Short arg", serde_json::Value::String(short.to_string())));
    app.on_acp_event(tool_completed_status("tool-1"));

    sync_terminal(&mut terminal, &mut app).unwrap();

    let viewport = buffer_text(&viewport_buffer(&mut terminal));
    assert!(viewport.contains(short), "short args must appear in full: {viewport}");
    assert!(!viewport.contains('…'), "short args must NOT have ellipsis: {viewport}");
}

#[test]
fn truncation_is_unicode_safe_no_split_characters() {
    let (mut app, _command_rx) = make_app();
    let mut terminal = make_terminal_with_width(250);
    submit_prompt(&mut app, "run tool");
    let prefix = "a".repeat(195);
    let unicode_arg = format!("{prefix}こんにちは世界");
    app.on_acp_event(tool_call_with_raw("tool-1", "Unicode tool", serde_json::Value::String(unicode_arg)));
    app.on_acp_event(tool_completed_status("tool-1"));

    sync_terminal(&mut terminal, &mut app).unwrap();

    let viewport = buffer_text(&viewport_buffer(&mut terminal));
    assert!(viewport.contains('…'), "truncated Unicode args must show ellipsis: {viewport}");
    assert!(viewport.contains('ん'), "char-based truncation must preserve leading multi-byte chars: {viewport}");
    assert!(!viewport.contains('ち'), "truncation must stop at char boundary (199 chars): {viewport}");
}

#[test]
fn char_based_truncation_preserves_multi_byte_under_200_chars() {
    let (mut app, _command_rx) = make_app();
    let mut terminal = make_terminal_with_width(250);
    submit_prompt(&mut app, "run tool");
    // 100 ASCII chars + 50 × '界' (3 bytes each) = 150 chars, but 100 + 150 = 250 bytes
    // This is under 200 chars so it must NOT be truncated even though it exceeds 200 bytes.
    let half = "a".repeat(100);
    let unicode_half = "界".repeat(50);
    let arg = format!("{half}{unicode_half}");
    assert!(arg.len() > 200, "arg must exceed 200 bytes for this test");
    assert!(arg.chars().count() < 200, "arg must be under 200 chars (with leading space)");
    app.on_acp_event(tool_call_with_raw("tool-1", "Unicode tool", serde_json::Value::String(arg)));
    app.on_acp_event(tool_completed_status("tool-1"));

    sync_terminal(&mut terminal, &mut app).unwrap();

    let viewport = buffer_text(&viewport_buffer(&mut terminal));
    assert!(!viewport.contains('…'), "under 200 chars must not be truncated: {viewport}");
    assert!(viewport.contains('界'), "multi-byte chars must appear in full: {viewport}");
}

#[test]
fn truncation_boundary_exactly_200_bytes_no_ellipsis() {
    let (mut app, _command_rx) = make_app();
    let mut terminal = make_terminal_with_width(250);
    submit_prompt(&mut app, "run tool");
    let exactly_199 = "x".repeat(199);
    app.on_acp_event(tool_call_with_raw("tool-1", "199-char arg", serde_json::Value::String(exactly_199)));
    app.on_acp_event(tool_completed_status("tool-1"));

    sync_terminal(&mut terminal, &mut app).unwrap();

    let viewport = buffer_text(&viewport_buffer(&mut terminal));
    assert!(!viewport.contains('…'), "199-char args must not be truncated (fit in 200 with space): {viewport}");
}

#[test]
fn truncation_preserved_in_scrollback() {
    let (mut app, _command_rx) = make_app();
    let mut terminal = make_terminal_with_dimensions(250, 15);
    let mut renderer = Presenter::new(&UiSettings::default());
    submit_prompt(&mut app, "run tool");
    let long = "x".repeat(300);
    app.on_acp_event(tool_call_with_raw("tool-1", "Long arg", serde_json::Value::String(long)));
    app.on_acp_event(tool_completed_status("tool-1"));
    app.on_acp_event(AcpEvent::PromptDone(acp::StopReason::EndTurn));

    sync_terminal_with_renderer(&mut terminal, &mut app, &mut renderer).unwrap();
    sync_terminal_with_renderer(&mut terminal, &mut app, &mut renderer).unwrap();

    let conversation = buffer_text(&conversation_buffer(&mut terminal));
    assert!(conversation.contains('…'), "truncation must survive drain to scrollback: {conversation}");
}

#[test]
fn tool_arguments_preserved_in_scrollback_exactly_once() {
    let (mut app, _command_rx) = make_app();
    let mut terminal = make_terminal();
    let mut renderer = Presenter::new(&UiSettings::default());
    let raw = serde_json::json!({"path": "/src/main.rs"});
    submit_prompt(&mut app, "run tool");
    app.on_acp_event(tool_call_with_raw("tool-1", "Read file", raw));
    app.on_acp_event(tool_completed_status("tool-1"));
    app.on_acp_event(AcpEvent::PromptDone(acp::StopReason::EndTurn));

    sync_terminal_with_renderer(&mut terminal, &mut app, &mut renderer).unwrap();
    sync_terminal_with_renderer(&mut terminal, &mut app, &mut renderer).unwrap();

    let conversation = buffer_text(&conversation_buffer(&mut terminal));
    let occurrences = conversation.matches("/src/main.rs").count();
    assert_eq!(occurrences, 1, "tool args must appear exactly once in conversation: {conversation}");
    assert!(conversation.contains("Read file"), "title must appear: {conversation}");
}

#[test]
fn diff_not_rendered_while_tool_is_running() {
    let (mut app, _command_rx) = make_app();
    let mut terminal = make_terminal();
    submit_prompt(&mut app, "run tool");
    app.on_acp_event(tool_call("tool-1", "Edit file"));

    let diff = acp::Diff::new("src/main.rs", "new content").old_text("old content");
    let update = acp::ToolCallUpdate::new(
        "tool-1".to_string(),
        acp::ToolCallUpdateFields::new().content(vec![acp::ToolCallContent::Diff(diff)]),
    );
    app.on_acp_event(session_update(acp::SessionUpdate::ToolCallUpdate(update)));

    sync_terminal(&mut terminal, &mut app).unwrap();

    let viewport = buffer_text(&viewport_buffer(&mut terminal));
    assert!(viewport.contains("Edit file"), "title must be visible: {viewport}");
    assert!(!viewport.contains("old content"), "diff must NOT render while running: {viewport}");
    assert!(!viewport.contains("new content"), "diff must NOT render while running: {viewport}");
}

#[test]
fn diff_not_rendered_after_failed_status() {
    let (mut app, _command_rx) = make_app();
    let mut terminal = make_terminal();
    let mut renderer = Presenter::new(&UiSettings::default());
    submit_prompt(&mut app, "run tool");
    app.on_acp_event(tool_call("tool-1", "Edit file"));

    let diff = acp::Diff::new("src/main.rs", "new content").old_text("old content");
    let update = acp::ToolCallUpdate::new(
        "tool-1".to_string(),
        acp::ToolCallUpdateFields::new()
            .content(vec![acp::ToolCallContent::Diff(diff)])
            .status(acp::ToolCallStatus::Failed),
    );
    app.on_acp_event(session_update(acp::SessionUpdate::ToolCallUpdate(update)));
    app.on_acp_event(AcpEvent::PromptDone(acp::StopReason::EndTurn));

    sync_terminal_with_renderer(&mut terminal, &mut app, &mut renderer).unwrap();
    sync_terminal_with_renderer(&mut terminal, &mut app, &mut renderer).unwrap();

    let conversation = buffer_text(&conversation_buffer(&mut terminal));
    assert!(!conversation.contains("old content"), "diff must NOT render after failure: {conversation}");
    assert!(!conversation.contains("new content"), "diff must NOT render after failure: {conversation}");
    assert!(conversation.contains("failed"), "error cause must be visible: {conversation}");
}

#[test]
fn short_streaming_message_renders_above_progress_indicator_and_composer() {
    let (mut app, _command_rx) = make_app();
    let mut terminal = make_terminal();
    submit_prompt(&mut app, "prompt");
    app.on_acp_event(text_chunk("short answer"));

    sync_terminal(&mut terminal, &mut app).unwrap();

    let viewport = viewport_buffer(&mut terminal);
    let prompt_row = row_containing(&viewport, "prompt").unwrap();
    let message_row = row_containing(&viewport, "short answer").unwrap();
    let spinner_row = row_containing(&viewport, "⠋").unwrap();
    let composer_row = row_containing(&viewport, "> ").unwrap();
    assert!(prompt_row < message_row);
    assert!(
        message_row < spinner_row && spinner_row < composer_row,
        "progress indicator must sit between the message and the composer \
         (message {message_row}, spinner {spinner_row}, composer {composer_row})"
    );
}

#[test]
fn progress_indicator_renders_below_streaming_message() {
    let (mut app, _command_rx) = make_app();
    let mut terminal = make_terminal_with_width(80);
    submit_prompt(&mut app, "prompt");
    app.on_acp_event(text_chunk("streamed answer"));

    sync_terminal(&mut terminal, &mut app).unwrap();

    let viewport = viewport_buffer(&mut terminal);
    let message_row = row_containing(&viewport, "streamed answer").expect("streamed message should be visible");
    let spinner_row = row_containing(&viewport, "⠋").expect("progress spinner should be visible");
    assert!(
        message_row < spinner_row,
        "progress indicator must render below the streaming message \
         (message row {message_row}, spinner row {spinner_row})"
    );
}

#[test]
fn composer_echo_and_status_line_render_in_viewport() {
    let (mut app, _command_rx) = make_app();
    let mut terminal = make_terminal();

    type_text(&mut app, "typing");
    sync_terminal(&mut terminal, &mut app).unwrap();

    let viewport = buffer_text(&viewport_buffer(&mut terminal));
    assert!(viewport.contains("> typing"));
    assert!(viewport.contains("~/code/demo · main"));
    assert!(viewport.contains("aether"));
}
